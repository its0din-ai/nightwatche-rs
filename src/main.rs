use anyhow::{ Context, Result };
use log::{ error, info };
use tokio::sync::mpsc;

mod bot;
mod config;
mod connector;
mod informer;
mod models;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logger and load configuration
    pretty_env_logger::init();
    let config = config::load().context("Failed to load configuration")?;

    // Delegate to the appropriate run mode
    let result = match config.app_mode {
        config::AppMode::Master => run_master(config).await,
        config::AppMode::Slave => run_slave(config).await,
    };

    if let Err(e) = result {
        error!("Application exited with a critical error: {:?}", e);
    }

    Ok(())
}

async fn run_master(config: config::Config) -> Result<()> {
    info!("Starting in MASTER mode.");
    let bot_token = config.telegram_bot_token.context(
        "TELEGRAM_BOT_TOKEN is required for MASTER mode"
    )?;
    let chat_id_str = config.telegram_chat_id.context(
        "TELEGRAM_CHAT_ID is required for MASTER mode"
    )?;
    let alert_channel_id_str = config.alert_channel_id.context(
        "ALERT_CHANNEL_ID is required for MASTER mode"
    )?;
    // FIX: Get the topic ID from the config
    let topic_id = config.telegram_topic_id;

    // Channel for alerts from the connector to the bot logic
    let (alert_tx, alert_rx) = mpsc::channel(100);

    // Start the internal server to listen for alerts from slaves
    let server_handle = tokio::spawn(
        connector::start_server(
            config.slave_listen_addr.clone(),
            config.internal_api_key.clone(),
            alert_tx
        )
    );

    // Start the Telegram bot
    let bot_handle = tokio::spawn(
        bot::run(
            bot_token,
            chat_id_str,
            alert_channel_id_str,
            topic_id, // <-- FIX: Pass topic_id to the bot's run function
            alert_rx,
            config.slaves,
            config.internal_api_key
        )
    );

    tokio::select! {
        res = server_handle => error!("Connector server exited: {:?}", res),
        res = bot_handle => error!("Telegram bot exited: {:?}", res),
    }

    Err(anyhow::anyhow!("A critical Master component has shut down."))
}

async fn run_slave(config: config::Config) -> Result<()> {
    info!("Starting in SLAVE mode.");
    let master_endpoint = config.master_api_endpoint.context(
        "MASTER_API_ENDPOINT is required for SLAVE mode"
    )?;

    // This channel is unused in the slave but required by the connector's function signature
    let (alert_tx, _) = mpsc::channel(1);

    // Start the internal server to listen for commands from the master
    let server_handle = tokio::spawn(
        connector::start_server(
            config.slave_listen_addr.clone(),
            config.internal_api_key.clone(),
            alert_tx
        )
    );

    // Start the log watcher
    let mut alert_receiver = informer::watch_log_file().await?;

    let alert_forwarder_handle = tokio::spawn(async move {
        loop {
            if let Some(alert) = alert_receiver.recv().await {
                info!("Informer detected event: {:?}, sending to master.", alert.event_type);
                if
                    let Err(e) = connector::send_alert_to_master(
                        &master_endpoint,
                        alert,
                        &config.internal_api_key
                    ).await
                {
                    error!("Failed to send alert to master: {}", e);
                }
            }
        }
    });

    tokio::select! {
        res = server_handle => error!("Connector server exited: {:?}", res),
        res = alert_forwarder_handle => error!("Alert forwarder exited: {:?}", res),
    }

    Err(anyhow::anyhow!("A critical Slave component has shut down."))
}
