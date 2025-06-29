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

    if config.telegram_chat_ids.is_empty() {
        return Err(anyhow::anyhow!("TELEGRAM_CHAT_ID is required for MASTER mode"));
    }
    if config.alert_targets.is_empty() {
        return Err(anyhow::anyhow!("ALERT_TARGETS is required for MASTER mode"));
    }

    let (alert_tx, alert_rx) = mpsc::channel(100);

    info!("Master is also starting in slave mode to monitor local logs.");
    let local_alert_tx = alert_tx.clone();

    let master_alias_for_informer = std::env
        ::var("SERVER_ALIAS")
        .unwrap_or_else(|_| "master".to_string());

    tokio::spawn(async move {
        match informer::watch_log_file().await {
            Ok(mut alert_receiver) => {
                info!("Local log informer started successfully for master.");
                loop {
                    if let Some(mut alert) = alert_receiver.recv().await {
                        info!(
                            "Master's informer detected event: {:?}, forwarding to bot.",
                            alert.event_type
                        );
                        alert.slave_alias = master_alias_for_informer.clone();
                        if let Err(e) = local_alert_tx.send(alert).await {
                            error!("Failed to send local alert from master to bot channel: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to start local log informer for master: {:?}", e);
            }
        }
    });

    let server_handle = tokio::spawn(
        connector::start_server(
            config.listen_addr.clone(),
            config.internal_api_key.clone(),
            alert_tx.clone()
        )
    );

    let bot_handle = tokio::spawn(
        bot::run(
            bot_token,
            config.telegram_chat_ids,
            config.alert_targets,
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

    let (alert_tx, _) = mpsc::channel(1);

    let server_handle = tokio::spawn(
        connector::start_server(
            config.listen_addr.clone(),
            config.internal_api_key.clone(),
            alert_tx
        )
    );

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
