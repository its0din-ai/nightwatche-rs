use anyhow::Result;
use axum::{
    extract::{ State },
    http::{ header::{ AUTHORIZATION, CONTENT_TYPE }, HeaderMap, StatusCode },
    routing::post,
    Json,
    Router,
};
use log::{ info, warn };
use reqwest::{ Client };
use sha3::{ Digest, Sha3_512 };
use std::process::Command as StdCommand;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use crate::models::{ Alert, Command, HealthResponse, ListResponse, WhoResponse };

#[derive(Clone)]
struct AppState {
    api_key_hash: Arc<Vec<u8>>,
    alert_sender: mpsc::Sender<Alert>,
}

pub async fn send_command_to_slave<T: for<'de> serde::Deserialize<'de>>(
    slave_addr: &str,
    command: Command,
    api_key: &str
) -> Result<T> {
    let client = Client::new();
    let response = client
        .post(&format!("http://{}/command", slave_addr))
        .header(AUTHORIZATION, format!("Bearer {}", api_key))
        .header(CONTENT_TYPE, "application/json")
        .json(&command)
        .send().await?
        .json::<T>().await?;
    Ok(response)
}

pub async fn send_alert_to_master(
    master_endpoint: &str,
    alert: Alert,
    api_key: &str
) -> Result<()> {
    let client = Client::new();
    let response = client
        .post(master_endpoint)
        .header(AUTHORIZATION, format!("Bearer {}", api_key))
        .json(&alert)
        .send().await?;
    if !response.status().is_success() {
        return Err(anyhow::anyhow!("Failed to send alert to master: {}", response.status()));
    }
    Ok(())
}

pub async fn start_server(
    listen_addr: String,
    api_key: String,
    alert_sender: mpsc::Sender<Alert>
) -> Result<()> {
    let mut hasher = Sha3_512::new();
    hasher.update(api_key.as_bytes());
    let state = AppState {
        api_key_hash: Arc::new(hasher.finalize().to_vec()),
        alert_sender,
    };

    let app = Router::new()
        .route("/alert", post(handle_alert))
        .route("/command", post(handle_command))
        .with_state(state);

    info!("Internal connector server listening on {}", listen_addr);
    let listener = TcpListener::bind(listen_addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}

#[axum::debug_handler]
async fn handle_alert(
    State(state): State<AppState>,

    headers: HeaderMap,
    Json(alert): Json<Alert>
) -> Result<(), StatusCode> {
    authenticate(&headers, &state.api_key_hash)?;
    info!("Received authenticated alert from slave: {}", alert.slave_alias);
    state.alert_sender.send(alert).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(())
}

#[axum::debug_handler]
async fn handle_command(
    State(state): State<AppState>,

    headers: HeaderMap,
    Json(command): Json<Command>
) -> Result<Json<serde_json::Value>, StatusCode> {
    authenticate(&headers, &state.api_key_hash)?;
    info!("Received authenticated command: {:?}", command);
    let alias = std::env::var("SERVER_ALIAS").unwrap_or_else(|_| "unknown".to_string());
    match command {
        Command::Health =>
            Ok(
                Json(
                    serde_json
                        ::to_value(HealthResponse {
                            alias,
                            status: "Healthy".to_string(),
                        })
                        .unwrap()
                )
            ),
        Command::Who => {
            let output = StdCommand::new("who")
                .output()
                .map_or_else(
                    |e| format!("Error: {}", e),
                    |o| String::from_utf8_lossy(&o.stdout).to_string()
                );
            Ok(Json(serde_json::to_value(WhoResponse { alias, who_output: output }).unwrap()))
        }
        Command::List => {
            let uptime = StdCommand::new("uptime")
                .arg("-p")
                .output()
                .map_or_else(
                    |e| format!("Error: {}", e),
                    |o| String::from_utf8_lossy(&o.stdout).trim().to_string()
                );
            let resp = ListResponse {
                alias,
                hostname: hostname
                    ::get()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                ip: crate::informer::get_server_ip().unwrap_or_default(),
                uptime,
            };
            Ok(Json(serde_json::to_value(resp).unwrap()))
        }
    }
}

fn authenticate(headers: &HeaderMap, expected_hash: &[u8]) -> Result<(), StatusCode> {
    if let Some(auth_header) = headers.get(AUTHORIZATION).and_then(|value| value.to_str().ok()) {
        if let Some(key) = auth_header.strip_prefix("Bearer ") {
            let mut hasher = Sha3_512::new();
            hasher.update(key.as_bytes());
            if hasher.finalize().to_vec() == expected_hash {
                return Ok(());
            } else {
                warn!("Authentication failed: Invalid API Key provided.");
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }
    warn!("Authentication failed: Missing or invalid Authorization header.");
    Err(StatusCode::UNAUTHORIZED)
}
