use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use chrono::{FixedOffset, Utc};
use regex::Regex;
use reqwest::Client;
use dotenv::dotenv;
use std::env;
use teloxide::{Bot, prelude::*};
use std::net::IpAddr;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use std::path::Path;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

// Custom error type to handle both io::Error and notify::Error
#[derive(Debug)]
enum AppError {
    Io(std::io::Error),
    Notify(notify::Error),
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Io(err)
    }
}

impl From<notify::Error> for AppError {
    fn from(err: notify::Error) -> Self {
        AppError::Notify(err)
    }
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
    dotenv().ok();
    pretty_env_logger::init();
    log::info!("Starting log watcher at {}", Utc::now().with_timezone(&FixedOffset::east_opt(7 * 3600).unwrap()));

    // Validate environment variables
    let telegram_bot_token = env::var("TELEGRAM_BOT_TOKEN")
        .map_err(|_| AppError::Io(io::Error::new(io::ErrorKind::InvalidData, "TELEGRAM_BOT_TOKEN must be set")))?;
    if telegram_bot_token.is_empty() {
        return Err(AppError::Io(io::Error::new(io::ErrorKind::InvalidData, "TELEGRAM_BOT_TOKEN is empty")));
    }
    let whitelisted_chat_ids: Vec<i64> = env::var("WHITELISTED_CHAT_IDS")
        .map_err(|_| AppError::Io(io::Error::new(io::ErrorKind::InvalidData, "WHITELISTED_CHAT_IDS must be set")))?
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().parse().expect("Invalid chat ID"))
        .collect();
    let channel_id = env::var("CHANNEL_ID")
        .map_err(|_| AppError::Io(io::Error::new(io::ErrorKind::InvalidData, "CHANNEL_ID must be set")))?;
    if channel_id.is_empty() {
        return Err(AppError::Io(io::Error::new(io::ErrorKind::InvalidData, "CHANNEL_ID is empty")));
    }
    let topic_id = env::var("TOPIC_ID")
        .map_err(|_| AppError::Io(io::Error::new(io::ErrorKind::InvalidData, "TOPIC_ID must be set")))?;
    if topic_id.is_empty() {
        return Err(AppError::Io(io::Error::new(io::ErrorKind::InvalidData, "TOPIC_ID is empty")));
    }
    let log_path = env::var("LOG_PATH")
        .map_err(|_| AppError::Io(io::Error::new(io::ErrorKind::InvalidData, "LOG_PATH must be set")))?;
    if log_path.is_empty() {
        return Err(AppError::Io(io::Error::new(io::ErrorKind::InvalidData, "LOG_PATH is empty")));
    }
    let command_log_path = env::var("COMMAND_LOG_PATH").unwrap_or("commands.log".to_string());

    let hostname = get_hostname()?;
    let server_ip = get_server_ip().unwrap_or_else(|_| "Unknown".to_string());

    let client = Client::new();
    let bot = Bot::new(&telegram_bot_token);
    let ssh_re = Regex::new(r"sshd.*Accepted (password|publickey) for (\S+) from (\S+)").unwrap();
    let sudo_re = Regex::new(r"sudo:.*COMMAND=(.+)").unwrap();

    // Start Telegram bot in a separate task
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
                    // Log command to commands.log
                    let timestamp = Utc::now()
                        .with_timezone(&FixedOffset::east_opt(7 * 3600).unwrap())
                        .format("%Y-%m-%d %H:%M %Z")
                        .to_string();
                    let username = msg.from.as_ref()
                        .map(|user| user.username.clone().unwrap_or(user.first_name.clone()))
                        .unwrap_or("Unknown".to_string());
                    let sanitized_data = sanitize_input(data);
                    let log_entry = format!("{}|{}|{}\n", timestamp, username, sanitized_data);
                    if let Err(e) = append_to_command_log(&command_log_path, &log_entry).await {
                        eprintln!("Failed to log command: {}", e);
                    }

                    match data {
                        "/status" => {
                            let timestamp = Utc::now()
                                .with_timezone(&FixedOffset::east_opt(7 * 3600).unwrap())
                                .format("%Y-%m-%d %H:%M %Z")
                                .to_string();
                            let response = format!("Server Timestamp: {}\nStatus: Healthy", timestamp);
                            bot.send_message(msg.chat.id, response).await?;
                        }
                        "/init" => {
                            let response = format!(
                                "User IDs: {}\nChannel ID: {}\nTopic ID: {}",
                                whitelisted_chat_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", "),
                                channel_id,
                                topic_id
                            );
                            bot.send_message(msg.chat.id, response).await?;
                        }
                        _ => {}
                    }
                }
                Ok(())
            }
        }).await;
    });

    // Set up file watcher
    let (tx, mut rx) = mpsc::channel(32);
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            if let Ok(event) = res {
                let _ = tx.blocking_send(event);
            }
        },
        Config::default(),
    )?;
    watcher.watch(Path::new(&log_path), RecursiveMode::NonRecursive)?;

    // Open log file and seek to end
    let mut file = File::open(&log_path)?;
    file.seek(SeekFrom::End(0))?;
    let mut reader = BufReader::new(file);

    let mut line = String::new();
    loop {
        // Wait for file changes
        if rx.recv().await.is_some() {
            while reader.read_line(&mut line)? > 0 {
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
                        timestamp, server_ip, hostname, ip, user, auth_method
                    );
                    send_telegram_message(&client, &message, &telegram_bot_token, &whitelisted_chat_ids, &channel_id, &topic_id).await;
                } else if sudo_re.is_match(&sanitized_line) {
                    let message = format!(
                        "----\nTimestamp: {}\nIP: {}\nHost: {}\nEvent: SUDO\nMessage: Sudo command executed: {}\n----",
                        timestamp, server_ip, hostname, sanitized_line.trim()
                    );
                    send_telegram_message(&client, &message, &telegram_bot_token, &whitelisted_chat_ids, &channel_id, &topic_id).await;
                }

                line.clear();
            }
            // Re-open file in case of log rotation
            let new_file = File::open(&log_path)?;
            reader = BufReader::new(new_file);
            reader.seek(SeekFrom::End(0))?;
        }
    }
}

async fn send_telegram_message(client: &Client, message: &str, bot_token: &str, chat_ids: &[i64], channel_id: &str, topic_id: &str) {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);

    for &chat_id in chat_ids {
        if chat_id != 0 {
            let params = [
                ("chat_id", chat_id.to_string()),
                ("text", message.to_string()),
                ("message_thread_id", topic_id.to_string()),
            ];

            if let Err(e) = client.post(&url).form(&params).send().await {
                eprintln!("Failed to send Telegram message to {}: {}", chat_id, e);
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
            eprintln!("Failed to send Telegram channel message to {}: {}", channel_id, e);
        }
    }
}

async fn append_to_command_log(log_path: &str, entry: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    #[cfg(unix)]
    std::fs::set_permissions(log_path, std::fs::Permissions::from_mode(0o600))?;
    file.write_all(entry.as_bytes())?;
    file.flush()?;
    Ok(())
}

fn sanitize_input(input: &str) -> String {
    input.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .replace('\n', " ")
        .trim()
        .to_string()
}

fn get_hostname() -> Result<String, AppError> {
    Ok(hostname::get().map_err(|e| AppError::Io(io::Error::new(io::ErrorKind::Other, e)))?.to_string_lossy().into_owned())
}

fn get_server_ip() -> Result<String, AppError> {
    let interfaces = if_addrs::get_if_addrs()?;
    for interface in interfaces {
        if !interface.is_loopback() {
            if let IpAddr::V4(ip) = interface.ip() {
                return Ok(ip.to_string());
            }
        }
    }
    Err(AppError::Io(io::Error::new(io::ErrorKind::NotFound, "No suitable IP found")))
}
