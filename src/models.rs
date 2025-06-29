use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub slave_alias: String,
    pub event_type: String,
    pub timestamp: String,
    pub log_line: String,
    pub server_ip: String,
    pub hostname: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    Health,
    Who,
    List,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub alias: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhoResponse {
    pub alias: String,
    pub who_output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResponse {
    pub alias: String,
    pub hostname: String,
    pub ip: String,
    pub uptime: String,
}