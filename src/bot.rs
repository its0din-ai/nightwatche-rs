use crate::models::{ Alert, Command, HealthResponse, ListResponse, WhoResponse };
use anyhow::Result;
use futures::future::join_all;
use log::{ error, info};
use std::collections::HashMap;
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::time::Duration;
use teloxide::{
    prelude::*,
    types::{ Message, MessageId, ReplyParameters, ThreadId },
    utils::command::BotCommands,
};
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinError;
use tokio::time::timeout;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "These commands are supported:")]
enum TelegramCommand {
    #[command(description = "display this text.")]
    Help,
    #[command(description = "check service health. <all|server_name|master>")] Health(String),
    #[command(description = "see who is logged in. <server_name|master>")] Who(String),
    #[command(description = "list summary of all nodes.")]
    List,
}

pub async fn run(
    bot_token: String,
    whitelisted_chat_ids: Vec<i64>,
    alert_channel_ids: Vec<i64>,
    topic_ids: Vec<i32>,
    mut alert_receiver: Receiver<Alert>,
    slaves: HashMap<String, String>,
    api_key: String
) -> Result<()> {
    info!("Bot starting with {} whitelisted chat IDs.", whitelisted_chat_ids.len());
    let bot = Bot::new(bot_token);
    let bot_clone = bot.clone();

    tokio::spawn(async move {
        while let Some(alert) = alert_receiver.recv().await {
            let message = format!(
                "🚨 *New Alert from {}* 🚨\n\n*Host:* `{}`\n*IP:* `{}`\n*Event:* `{}`\n*Log:* `{}`",
                alert.slave_alias,
                alert.hostname,
                alert.server_ip,
                alert.event_type,
                alert.log_line
            );

            for &channel_id in &alert_channel_ids {
                if !topic_ids.is_empty() {
                    for &topic_id in &topic_ids {
                        let mut request = bot_clone.send_message(
                            ChatId(channel_id),
                            message.clone()
                        );
                        request = request.message_thread_id(ThreadId(MessageId(topic_id)));
                        if let Err(e) = request.await {
                            error!(
                                "Failed to send alert to channel {} topic {}: {}",
                                channel_id,
                                topic_id,
                                e
                            );
                        }
                    }
                } else {
                    if
                        let Err(e) = bot_clone.send_message(
                            ChatId(channel_id),
                            message.clone()
                        ).await
                    {
                        error!("Failed to send alert to channel {}: {}", channel_id, e);
                    }
                }
            }
        }
    });

    let slaves_arc = Arc::new(slaves);
    let api_key_arc = Arc::new(api_key);
    let whitelisted_chats_arc = Arc::new(whitelisted_chat_ids);

    let handler = Update::filter_message()
        .filter(move |msg: Message| { whitelisted_chats_arc.contains(&msg.chat.id.0) })
        .filter_command::<TelegramCommand>()
        .endpoint(
            |
                bot: Bot,
                msg: Message,
                cmd: TelegramCommand,
                slaves: Arc<HashMap<String, String>>,
                api_key: Arc<String>
            | async move {
                answer(bot, msg, cmd, &slaves, &api_key).await
            }
        );

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![slaves_arc, api_key_arc])
        .enable_ctrlc_handler()
        .build()
        .dispatch().await;

    Ok(())
}

async fn answer(
    bot: Bot,
    msg: Message,
    cmd: TelegramCommand,
    slaves: &HashMap<String, String>,
    api_key: &str
) -> ResponseResult<()> {
    let master_alias = std::env::var("SERVER_ALIAS").unwrap_or_else(|_| "master".to_string());

    match cmd {
        TelegramCommand::Help => {
            bot
                .send_message(msg.chat.id, TelegramCommand::descriptions().to_string())
                .message_thread_id(msg.thread_id.unwrap_or(ThreadId(MessageId(0))))
                .reply_parameters(ReplyParameters::new(msg.id)).await?;
        }
        TelegramCommand::Health(target) => {
            let target_lower = target.to_lowercase();
            if target_lower == "all" {
                let mut tasks = Vec::new();
                for (alias, addr) in slaves.clone() {
                    let key = api_key.to_string();
                    tasks.push(
                        tokio::spawn(async move {
                            let result = timeout(
                                Duration::from_secs(3),
                                crate::connector::send_command_to_slave::<HealthResponse>(
                                    &addr,
                                    Command::Health,
                                    &key
                                )
                            ).await;
                            match result {
                                Ok(Ok(resp)) => format!("✅ [{}] {}", resp.alias, resp.status),
                                _ => format!("❌ [{}] Timeout or Error", alias),
                            }
                        })
                    );
                }
                let m_alias = master_alias.clone();
                tasks.push(tokio::spawn(async move { format!("✅ [{}] Healthy", m_alias) }));

                // --- FIX: Correctly handle the Vec<Result<...>> ---
                let results: Vec<Result<String, JoinError>> = join_all(tasks).await;
                let report_lines: Vec<String> = results
                    .into_iter()
                    .map(|res| res.unwrap_or_else(|e| format!("Error joining task: {}", e)))
                    .collect();

                bot
                    .send_message(
                        msg.chat.id,
                        format!("--- Health Report ---\n{}", report_lines.join("\n"))
                    )
                    .message_thread_id(msg.thread_id.unwrap_or(ThreadId(MessageId(0))))
                    .reply_parameters(ReplyParameters::new(msg.id)).await?;
            } else if target_lower == "master" {
                let text = format!("✅ [{}] Healthy", master_alias);
                bot
                    .send_message(msg.chat.id, text)
                    .message_thread_id(msg.thread_id.unwrap_or(ThreadId(MessageId(0))))
                    .reply_parameters(ReplyParameters::new(msg.id)).await?;
            } else {
                let text = if let Some(addr) = slaves.get(&target) {
                    let resp = crate::connector::send_command_to_slave::<HealthResponse>(
                        addr,
                        Command::Health,
                        api_key
                    ).await;
                    match resp {
                        Ok(r) => format!("✅ [{}] {}", r.alias, r.status),
                        Err(e) => format!("❌ [{}] Error: {}", target, e),
                    }
                } else {
                    format!("Unknown server alias: {}", target)
                };
                bot
                    .send_message(msg.chat.id, text)
                    .message_thread_id(msg.thread_id.unwrap_or(ThreadId(MessageId(0))))
                    .reply_parameters(ReplyParameters::new(msg.id)).await?;
            }
        }
        TelegramCommand::Who(alias) => {
            let alias_lower = alias.to_lowercase();
            if alias_lower == "master" {
                let output = StdCommand::new("who")
                    .output()
                    .map_or_else(
                        |e| format!("Error executing command: {}", e),
                        |o| String::from_utf8_lossy(&o.stdout).to_string()
                    );
                bot
                    .send_message(
                        msg.chat.id,
                        format!("--- Who on {} ---\n`{}`", master_alias, output)
                    )
                    .message_thread_id(msg.thread_id.unwrap_or(ThreadId(MessageId(0))))
                    .reply_parameters(ReplyParameters::new(msg.id)).await?;
            } else if let Some(addr) = slaves.get(&alias) {
                match
                    crate::connector::send_command_to_slave::<WhoResponse>(
                        addr,
                        Command::Who,
                        api_key
                    ).await
                {
                    Ok(resp) => {
                        bot
                            .send_message(
                                msg.chat.id,
                                format!("--- Who on {} ---\n`{}`", resp.alias, resp.who_output)
                            )
                            .message_thread_id(msg.thread_id.unwrap_or(ThreadId(MessageId(0))))
                            .reply_parameters(ReplyParameters::new(msg.id)).await?;
                    }
                    Err(e) => {
                        bot
                            .send_message(msg.chat.id, format!("Error: {}", e))
                            .message_thread_id(msg.thread_id.unwrap_or(ThreadId(MessageId(0))))
                            .reply_parameters(ReplyParameters::new(msg.id)).await?;
                    }
                }
            } else {
                bot
                    .send_message(msg.chat.id, format!("Unknown server alias: {}", alias))
                    .message_thread_id(msg.thread_id.unwrap_or(ThreadId(MessageId(0))))
                    .reply_parameters(ReplyParameters::new(msg.id)).await?;
            }
        }
        TelegramCommand::List => {
            let mut tasks = Vec::new();
            for (alias, addr) in slaves.clone() {
                let key = api_key.to_string();
                tasks.push(
                    tokio::spawn(async move {
                        let result = timeout(
                            Duration::from_secs(3),
                            crate::connector::send_command_to_slave::<ListResponse>(
                                &addr,
                                Command::List,
                                &key
                            )
                        ).await;
                        match result {
                            Ok(Ok(resp)) =>
                                format!(
                                    "✅ *{}*\n  Host: `{}`\n  IP: `{}`\n  Uptime: `{}`",
                                    resp.alias,
                                    resp.hostname,
                                    resp.ip,
                                    resp.uptime
                                ),
                            _o => format!("❌ *{}*\n  Unreachable", alias),
                        }
                    })
                );
            }
            let m_alias = master_alias.clone();
            tasks.push(
                tokio::spawn(async move {
                    let uptime = StdCommand::new("uptime")
                        .arg("-p")
                        .output()
                        .map_or_else(
                            |e| format!("Error: {}", e),
                            |o| String::from_utf8_lossy(&o.stdout).trim().to_string()
                        );
                    let hostname = hostname
                        ::get()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let ip = crate::informer::get_server_ip().unwrap_or_default();
                    format!(
                        "✅ *{}* (Master)\n  Host: `{}`\n  IP: `{}`\n  Uptime: `{}`",
                        m_alias,
                        hostname,
                        ip,
                        uptime
                    )
                })
            );

            // --- FIX: Correctly handle the Vec<Result<...>> ---
            let results: Vec<Result<String, JoinError>> = join_all(tasks).await;
            let summary_lines: Vec<String> = results
                .into_iter()
                .map(|res| res.unwrap_or_else(|e| format!("Error joining task: {}", e)))
                .collect();

            bot
                .send_message(
                    msg.chat.id,
                    format!("--- Node Summary ---\n{}", summary_lines.join("\n\n"))
                )
                .message_thread_id(msg.thread_id.unwrap_or(ThreadId(MessageId(0))))
                .reply_parameters(ReplyParameters::new(msg.id)).await?;
        }
    }
    Ok(())
}
