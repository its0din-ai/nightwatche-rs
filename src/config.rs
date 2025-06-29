use anyhow::{ anyhow, Context, Result };
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::str::FromStr;

pub struct Config {
    pub app_mode: AppMode,
    pub telegram_bot_token: Option<String>,
    pub telegram_chat_ids: Vec<i64>,
    pub alert_targets: Vec<TelegramTarget>,
    pub master_api_endpoint: Option<String>,
    pub internal_api_key: String,
    pub listen_addr: String,
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

#[derive(Clone, Debug)]
pub struct TelegramTarget {
    pub chat_id: i64,
    pub topic_id: Option<i32>,
}

fn get_telegram_targets(var_name: &str) -> Result<Vec<TelegramTarget>> {
    env::var(var_name).map_or(Ok(Vec::new()), |s| {
        s.split(',')
            .map(|item| {
                let mut parts = item.trim().split(':');
                let chat_id_str = parts.next().ok_or_else(|| anyhow!("Missing chat ID"))?;
                let chat_id = chat_id_str.parse::<i64>()?;
                let topic_id = parts
                    .next()
                    .map(|s| s.parse::<i32>())
                    .transpose()?;
                Ok(TelegramTarget { chat_id, topic_id })
            })
            .collect::<Result<Vec<TelegramTarget>>>()
            .context(format!("Failed to parse environment variable {}", var_name))
    })
}

fn get_env_vec<T>(var_name: &str) -> Result<Vec<T>>
    where T: FromStr, <T as FromStr>::Err: std::error::Error + Send + Sync + 'static
{
    env::var(var_name).map_or(Ok(Vec::new()), |s| {
        s.split(',')
            .map(|item| item.trim().parse::<T>())
            .collect::<Result<Vec<T>, _>>()
            .map_err(|e| anyhow!(e))
            .context(format!("Failed to parse environment variable {}", var_name))
    })
}

pub fn load() -> Result<Config> {
    dotenv::dotenv().ok();

    let app_mode_str = env::var("APP_MODE").context("APP_MODE must be set (MASTER or SLAVE)")?;
    let app_mode = match app_mode_str.as_str() {
        "MASTER" => AppMode::Master,
        "SLAVE" => AppMode::Slave,
        _ => {
            return Err(anyhow::anyhow!("Invalid APP_MODE: {}", app_mode_str));
        }
    };

    let internal_api_key = env::var("INTERNAL_API_KEY").context("INTERNAL_API_KEY must be set")?;

    let mut config = Config {
        app_mode,
        telegram_bot_token: env::var("TELEGRAM_BOT_TOKEN").ok(),
        telegram_chat_ids: get_env_vec("TELEGRAM_CHAT_ID")?,
        alert_targets: get_telegram_targets("ALERT_TARGETS")?,
        master_api_endpoint: env::var("MASTER_API_ENDPOINT").ok(),
        internal_api_key,
        listen_addr: env::var("LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:13001".to_string()),
        slaves: HashMap::new(),
    };

    if let AppMode::Master = config.app_mode {
        let servers_config_str = fs
            ::read_to_string("servers.json")
            .context("Could not find or read servers.json for MASTER mode")?;
        let servers_config: ServersConfig = serde_json
            ::from_str(&servers_config_str)
            .context("Failed to parse servers.json")?;
        config.slaves = servers_config.slaves;
    }

    Ok(config)
}
