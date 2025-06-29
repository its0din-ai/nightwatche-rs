use crate::models::{ Alert, Command, HealthResponse, ListResponse, WhoResponse };
use anyhow::Result;
use futures::future::join_all;
use log::error;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use teloxide::{
    prelude::*,
    types::{ Message, MessageId, ThreadId, ReplyParameters },
    utils::command::BotCommands,
};
use tokio::sync::mpsc::Receiver;
use tokio::time::timeout;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "These commands are supported:")]
enum TelegramCommand {
    #[command(description = "display this text.")]
    Help,
    #[command(description = "check service health. <all|server_name>")] Health(String),
    #[command(description = "see who is logged in. <server_name>")] Who(String),
    #[command(description = "list summary of all nodes.")]
    List,
}

pub async fn run(
    bot_token: String,
    chat_id_str: String,
    topic_id: Option<i32>,
    mut alert_receiver: Receiver<Alert>,
    slaves: HashMap<String, String>,
    api_key: String
) -> Result<()> {
    let bot = Bot::new(bot_token);
    let chat_id: i64 = chat_id_str.parse()?;
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

            let mut request = bot_clone.send_message(ChatId(chat_id), message);
            if let Some(id) = topic_id {
                request = request.message_thread_id(ThreadId(MessageId(id)));
            }

            if let Err(e) = request.await {
                error!("Failed to send alert to Telegram: {}", e);
            }
        }
    });

    let slaves_arc = Arc::new(slaves);
    let api_key_arc = Arc::new(api_key);

    let handler = Update::filter_message()
        .filter(move |msg: Message| {
            topic_id.map_or(true, |t_id| {
                msg.thread_id.map_or(false, |msg_t_id| msg_t_id.0.0 == t_id)
            })
        })
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
    match cmd {
        TelegramCommand::Help => {
            bot
                .send_message(msg.chat.id, TelegramCommand::descriptions().to_string())
                .message_thread_id(msg.thread_id.unwrap_or(ThreadId(MessageId(0))))
                .reply_parameters(ReplyParameters::new(msg.id)).await?;
        }
        TelegramCommand::Health(target) => {
            if target.to_lowercase() == "all" {
                let mut tasks = Vec::new();
                for (alias, addr) in slaves.clone() {
                    let key = api_key.to_string();
                    tasks.push(async move {
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
                    });
                }
                let results = join_all(tasks).await;
                bot
                    .send_message(
                        msg.chat.id,
                        format!("--- Health Report ---\n{}", results.join("\n"))
                    )
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
            if let Some(addr) = slaves.get(&alias) {
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
                tasks.push(async move {
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
                });
            }
            let results = join_all(tasks).await;
            bot
                .send_message(
                    msg.chat.id,
                    format!("--- Node Summary ---\n{}", results.join("\n\n"))
                )
                .message_thread_id(msg.thread_id.unwrap_or(ThreadId(MessageId(0))))
                .reply_parameters(ReplyParameters::new(msg.id)).await?;
        }
    }
    Ok(())
}
