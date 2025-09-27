use anyhow::{ Context, Result };
use chrono::{ DateTime, Duration, FixedOffset, Utc };
use log::{ error, info };
use notify::{ Config, RecommendedWatcher, RecursiveMode, Watcher };
use regex::Regex;
use std::collections::HashMap;
use std::fs::File;
use std::io::{ BufRead, BufReader, Seek, SeekFrom };
use std::net::IpAddr;
use std::path::Path;
use tokio::sync::mpsc::{ self, Receiver, Sender };
use crate::models::Alert;

struct FailureTracker {
    attempts: HashMap<String, Vec<DateTime<Utc>>>,
    max_attempts: usize,
    time_window: Duration,
}

impl FailureTracker {
    fn new(max_attempts: usize, time_window_seconds: i64) -> Self {
        FailureTracker {
            attempts: HashMap::new(),
            max_attempts,
            time_window: Duration::seconds(time_window_seconds),
        }
    }

    fn record_and_check(&mut self, ip: &str) -> bool {
        let now = Utc::now();
        let recent_failures = self.attempts.entry(ip.to_string()).or_default();
        recent_failures.retain(|&timestamp| now - timestamp < self.time_window);
        recent_failures.push(now);
        if recent_failures.len() >= self.max_attempts {
            recent_failures.clear();
            true
        } else {
            false
        }
    }
}

pub async fn watch_log_file() -> Result<Receiver<Alert>> {
    let (tx, rx) = mpsc::channel(100);
    let log_path = std::env::var("LOG_PATH").context("LOG_PATH must be set for Slave mode")?;

    tokio::spawn(async move {
        info!("Informer starting to watch log file: {}", log_path);
        if let Err(e) = run_watcher(&log_path, tx).await {
            error!("Log watcher task failed: {:?}", e);
        }
    });

    Ok(rx)
}

async fn run_watcher(log_path: &str, tx: Sender<Alert>) -> Result<()> {
    let ssh_success_re = Regex::new(r"sshd.*Accepted (password|publickey) for (\S+) from (\S+)")?;
    let sudo_re = Regex::new(r"sudo:.*COMMAND=(.+)")?;
    let new_user_re = Regex::new(r"(useradd|adduser).*new user")?;
    let new_group_re = Regex::new(r"(groupadd|addgroup).*new group")?;

    let path = Path::new(log_path);
    let mut file = File::open(path).context(format!("Failed to open log file: {}", log_path))?;
    file.seek(SeekFrom::End(0))?;
    let mut reader = BufReader::new(file);

    let (fs_tx, mut fs_rx) = mpsc::channel(32);
    let mut watcher = RecommendedWatcher::new(move |res| {
        if let Ok(event) = res {
            let _ = fs_tx.blocking_send(event);
        }
    }, Config::default())?;

    watcher.watch(path, RecursiveMode::NonRecursive)?;

    let mut line = String::new();
    loop {
        if fs_rx.recv().await.is_some() {
            while reader.read_line(&mut line)? > 0 {
                let mut alert_opt: Option<Alert> = None;

                if ssh_success_re.is_match(&line) {
                    alert_opt = Some(create_alert("SSH_LOGIN", &line));
                } else if sudo_re.is_match(&line) {
                    alert_opt = Some(create_alert("SUDO", &line));
                } else if new_user_re.is_match(&line) {
                    alert_opt = Some(create_alert("NEW_USER_CREATED", &line));
                } else if new_group_re.is_match(&line) {
                    alert_opt = Some(create_alert("NEW_GROUP_CREATED", &line));
                }

                if let Some(alert) = alert_opt {
                    if let Err(e) = tx.send(alert).await {
                        error!("Failed to send alert to channel: {}", e);
                    }
                }
                line.clear();
            }
        }
    }
}

fn create_alert(event_type: &str, log_line: &str) -> Alert {
    Alert {
        slave_alias: std::env::var("SERVER_ALIAS").unwrap_or_else(|_| "unknown".to_string()),
        event_type: event_type.to_string(),

        timestamp: Utc::now().with_timezone(&FixedOffset::east_opt(0).unwrap()).to_string(),
        log_line: log_line.trim().to_string(),
        server_ip: get_server_ip().unwrap_or_else(|_| "unknown".to_string()),
        hostname: hostname
            ::get()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".to_string()),
    }
}

pub fn get_server_ip() -> Result<String> {
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
