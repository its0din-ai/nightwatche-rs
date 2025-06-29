use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;

pub struct Config {
    pub app_mode: AppMode,
    pub telegram_bot_token: Option<String>,
    pub telegram_chat_id: Option<String>,
    pub telegram_topic_id: Option<i32>, // <-- FIX: Added topic ID field
    pub master_api_endpoint: Option<String>,
    pub internal_api_key: String,
    pub slave_listen_addr: String,
    pub slaves: HashMap<String, String>,
}

pub enum AppMode {
    Master,
    Slave,
}

#[derive(Deserialize)]
struct ServersConfig {
    slaves: HashMap<String, String>,
}

pub fn load() -> Result<Config> {
    dotenv::dotenv().ok();

    let app_mode_str = env::var("APP_MODE").context("APP_MODE must be set (MASTER or SLAVE)")?;
    let app_mode = match app_mode_str.as_str() {
        "MASTER" => AppMode::Master,
        "SLAVE" => AppMode::Slave,
        _ => return Err(anyhow::anyhow!("Invalid APP_MODE: {}", app_mode_str)),
    };

    // FIX: Parse the TOPIC_ID from environment. It's optional.
    let telegram_topic_id = env::var("TOPIC_ID").ok().and_then(|id| id.parse::<i32>().ok());


    let internal_api_key = env::var("INTERNAL_API_KEY").context("INTERNAL_API_KEY must be set")?;

    let mut config = Config {
        app_mode,
        telegram_bot_token: env::var("TELEGRAM_BOT_TOKEN").ok(),
        telegram_chat_id: env::var("TELEGRAM_CHAT_ID").ok(),
        telegram_topic_id, // <-- FIX: Assign parsed topic ID
        master_api_endpoint: env::var("MASTER_API_ENDPOINT").ok(),
        internal_api_key,
        slave_listen_addr: env::var("SLAVE_LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string()),
        slaves: HashMap::new(),
    };

    if let AppMode::Master = config.app_mode {
        let servers_config_str = fs::read_to_string("servers.json")
            .context("Could not find or read servers.json for MASTER mode")?;
        let servers_config: ServersConfig = serde_json::from_str(&servers_config_str)
            .context("Failed to parse servers.json")?;
        config.slaves = servers_config.slaves;
    }

    Ok(config)
}