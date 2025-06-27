use std::fs::{ File, OpenOptions };
use std::io::{ BufRead, BufReader, Seek, SeekFrom, Write };
use chrono::{ FixedOffset, Utc };
use regex::Regex;
use reqwest::Client;
use dotenv::dotenv;
use std::env;
use teloxide::{ Bot, prelude::* };
use std::net::IpAddr;
use notify::{ Config, RecommendedWatcher, RecursiveMode, Watcher };
use tokio::sync::mpsc;
use std::path::Path;
#[cfg(unix)]
use anyhow::{ Context, Result };
use log::{ error, info };

#[derive(Debug)]
enum AppError {
    Io(()),
    Notify(()),
}

impl From<std::io::Error> for AppError {
    fn from(_err: std::io::Error) -> Self {
        AppError::Io(())
    }
}

impl From<notify::Error> for AppError {
    fn from(_err: notify::Error) -> Self {
        AppError::Notify(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    pretty_env_logger::init();
    info!(
        "Starting log watcher at {}",
        Utc::now().with_timezone(&FixedOffset::east_opt(7 * 3600).unwrap())
    );

    let telegram_bot_token = env
        ::var("TELEGRAM_BOT_TOKEN")
        .context("Failed to read TELEGRAM_BOT_TOKEN")?;
    if telegram_bot_token.is_empty() {
        return Err(anyhow::anyhow!("TELEGRAM_BOT_TOKEN is empty"));
    }
    let whitelisted_chat_ids: Vec<i64> = env
        ::var("WHITELISTED_CHAT_IDS")
        .context("Failed to read WHITELISTED_CHAT_IDS")?
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().parse().context("Invalid chat ID in WHITELISTED_CHAT_IDS"))
        .collect::<Result<Vec<i64>>>()?;
    let channel_id = env::var("CHANNEL_ID").context("Failed to read CHANNEL_ID")?;
    if channel_id.is_empty() {
        return Err(anyhow::anyhow!("CHANNEL_ID is empty"));
    }
    let topic_id = env::var("TOPIC_ID").context("Failed to read TOPIC_ID")?;
    if topic_id.is_empty() {
        return Err(anyhow::anyhow!("TOPIC_ID is empty"));
    }
    let log_path = env::var("LOG_PATH").context("Failed to read LOG_PATH")?;
    if log_path.is_empty() {
        return Err(anyhow::anyhow!("LOG_PATH is empty"));
    }
    let command_log_path = env::var("COMMAND_LOG_PATH").unwrap_or("commands.log".to_string());
    info!("Using command log path: {}", command_log_path);

    let hostname = get_hostname().context("Failed to get hostname")?;
    let server_ip = get_server_ip().unwrap_or_else(|e| {
        error!("Failed to get server IP: {:?}", e);
        "Unknown".to_string()
    });

    let client = Client::new();
    let bot = Bot::new(&telegram_bot_token);
    let ssh_re = Regex::new(r"sshd.*Accepted (password|publickey) for (\S+) from (\S+)").context(
        "Failed to compile SSH regex"
    )?;
    let sudo_re = Regex::new(r"sudo:.*COMMAND=(.+)").context("Failed to compile sudo regex")?;

    let bot_clone = bot.clone();
    let whitelisted_chat_ids_ref = whitelisted_chat_ids.clone();
    let channel_id_ref = channel_id.clone();
    let topic_id_ref = topic_id.clone();
    let command_log_path_ref = command_log_path.clone();
    tokio::spawn(async move {
        teloxide::repl(bot_clone, move |bot: Bot, msg: Message| {
            let whitelisted_chat_ids = whitelisted_chat_ids_ref.clone();
            let channel_id = channel_id_ref.clone();
            let topic_id = topic_id_ref.clone();
            let command_log_path = command_log_path_ref.clone();
            async move {
                let data = msg.text().unwrap_or("");
                if data == "/status" || data == "/init" {
                    let timestamp = Utc::now()
                        .with_timezone(&FixedOffset::east_opt(7 * 3600).unwrap())
                        .format("%Y-%m-%d %H:%M %Z")
                        .to_string();
                    let username = msg.from
                        .as_ref()
                        .map(|user| user.username.clone().unwrap_or(user.first_name.clone()))
                        .unwrap_or("Unknown".to_string());
                    let sanitized_data = sanitize_input(data);
                    let log_entry = format!("{}|{}|{}\n", timestamp, username, sanitized_data);
                    if let Err(e) = append_to_command_log(&command_log_path, &log_entry).await {
                        error!(
                            "Failed to log command to {}: {:?}\n  Username: {}\n  Command: {}",
                            command_log_path,
                            e,
                            username,
                            sanitized_data
                        );
                    } else {
                        info!(
                            "Successfully logged command to {}: {}|{}|{}",
                            command_log_path,
                            timestamp,
                            username,
                            sanitized_data
                        );
                    }

                    match data {
                        "/status" => {
                            let timestamp = Utc::now()
                                .with_timezone(&FixedOffset::east_opt(7 * 3600).unwrap())
                                .format("%Y-%m-%d %H:%M %Z")
                                .to_string();
                            let response =
                                format!("Server Timestamp: {}\nStatus: Healthy", timestamp);
                            if let Err(e) = bot.send_message(msg.chat.id, response).await {
                                error!(
                                    "Failed to send /status response to chat {}: {:?}",
                                    msg.chat.id,
                                    e
                                );
                            }
                        }
                        "/init" => {
                            let response = format!(
                                "User IDs: {}\nChannel ID: {}\nTopic ID: {}",
                                whitelisted_chat_ids
                                    .iter()
                                    .map(|id| id.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", "),
                                channel_id,
                                topic_id
                            );
                            if let Err(e) = bot.send_message(msg.chat.id, response).await {
                                error!(
                                    "Failed to send /init response to chat {}: {:?}",
                                    msg.chat.id,
                                    e
                                );
                            }
                        }
                        _ => {}
                    }
                }
                Ok(())
            }
        }).await;
    });

    let (tx, mut rx) = mpsc::channel(32);
    let mut watcher = RecommendedWatcher::new(move |res| {
        if let Ok(event) = res {
            let _ = tx.blocking_send(event);
        } else if let Err(e) = res {
            error!("File watcher error: {:?}", e);
        }
    }, Config::default()).context("Failed to create file watcher")?;
    watcher
        .watch(Path::new(&log_path), RecursiveMode::NonRecursive)
        .context(format!("Failed to watch log file: {}", log_path))?;

    let mut file = File::open(&log_path).context(format!("Failed to open log file: {}", log_path))?;
    file
        .seek(SeekFrom::End(0))
        .context(format!("Failed to seek to end of log file: {}", log_path))?;
    let mut reader = BufReader::new(file);

    let mut line = String::new();
    loop {
        if rx.recv().await.is_some() {
            while
                reader
                    .read_line(&mut line)
                    .context(format!("Failed to read line from log file: {}", log_path))? > 0
            {
                let timestamp = Utc::now()
                    .with_timezone(&FixedOffset::east_opt(7 * 3600).unwrap())
                    .format("%Y-%m-%d %H:%M %Z")
                    .to_string();
                let sanitized_line = sanitize_input(&line);

                if let Some(captures) = ssh_re.captures(&sanitized_line) {
                    let auth_method = captures.get(1).map_or("", |m| m.as_str());
                    let user = captures.get(2).map_or("", |m| m.as_str());
                    let ip = captures.get(3).map_or("", |m| m.as_str());

                    let message = format!(
                        "----\nTimestamp: {}\nIP: {}\nHost: {}\nEvent: SSH_LOGIN\nMessage: Valid ssh login from {} using {} with {}\n----",
                        timestamp,
                        server_ip,
                        hostname,
                        ip,
                        user,
                        auth_method
                    );
                    send_telegram_message(
                        &client,
                        &message,
                        &telegram_bot_token,
                        &whitelisted_chat_ids,
                        &channel_id,
                        &topic_id
                    ).await;
                } else if sudo_re.is_match(&sanitized_line) {
                    let message = format!(
                        "----\nTimestamp: {}\nIP: {}\nHost: {}\nEvent: SUDO\nMessage: Sudo command executed: {}\n----",
                        timestamp,
                        server_ip,
                        hostname,
                        sanitized_line.trim()
                    );
                    send_telegram_message(
                        &client,
                        &message,
                        &telegram_bot_token,
                        &whitelisted_chat_ids,
                        &channel_id,
                        &topic_id
                    ).await;
                }

                line.clear();
            }
            let new_file = File::open(&log_path).context(
                format!("Failed to reopen log file after rotation: {}", log_path)
            )?;
            reader = BufReader::new(new_file);
            reader
                .seek(SeekFrom::End(0))
                .context(
                    format!("Failed to seek to end of log file after rotation: {}", log_path)
                )?;
        }
    }
}

async fn send_telegram_message(
    client: &Client,
    message: &str,
    bot_token: &str,
    chat_ids: &[i64],
    channel_id: &str,
    topic_id: &str
) {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);

    for &chat_id in chat_ids {
        if chat_id != 0 {
            let params = [
                ("chat_id", chat_id.to_string()),
                ("text", message.to_string()),
                ("message_thread_id", topic_id.to_string()),
            ];

            if let Err(e) = client.post(&url).form(&params).send().await {
                error!("Failed to send Telegram message to chat {}: {:?}", chat_id, e);
            }
        }
    }

    if !channel_id.is_empty() && topic_id.parse::<i64>().is_ok() {
        let channel_params = [
            ("chat_id", channel_id.to_string()),
            ("text", message.to_string()),
            ("message_thread_id", topic_id.to_string()),
        ];

        if let Err(e) = client.post(&url).form(&channel_params).send().await {
            error!("Failed to send Telegram channel message to {}: {:?}", channel_id, e);
        }
    }
}

async fn append_to_command_log(log_path: &str, entry: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .context(format!("Failed to open command log file: {}", log_path))?;
    file.write_all(entry.as_bytes())
        .context(format!("Failed to write to command log file: {}", log_path))?;
    file.flush()
        .context(format!("Failed to flush command log file: {}", log_path))?;
    Ok(())
}

fn sanitize_input(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .replace('\n', " ")
        .trim()
        .to_string()
}

fn get_hostname() -> Result<String> {
    Ok(hostname::get().context("Failed to get hostname")?.to_string_lossy().into_owned())
}

fn get_server_ip() -> Result<String> {
    let interfaces = if_addrs::get_if_addrs().context("Failed to get network interfaces")?;
    for interface in interfaces {
        if !interface.is_loopback() {
            if let IpAddr::V4(ip) = interface.ip() {
                return Ok(ip.to_string());
            }
        }
    }
    Err(anyhow::anyhow!("No suitable IP found"))
}
