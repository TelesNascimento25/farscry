use farscry_core::StateId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A11yNode {
    pub row_id: i64,
    pub snapshot_id: i64,
    pub role: String,
    pub name: String,
    pub description: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub states: Vec<String>,
    pub parent_row_id: Option<i64>,
    pub sequence: i32,
}

#[derive(Debug, Clone)]
pub struct A11ySnapshot {
    pub state_id: StateId,
    pub captured_at_ms: i64,
    pub app_name: String,
    pub nodes: Vec<A11yNode>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct A11yQueryParams {
    pub role: Option<String>,
    pub name_contains: Option<String>,
    pub app_name: Option<String>,
    pub state_id_hex: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A11yQueryResult {
    pub nodes: Vec<A11yNode>,
    pub snapshot_count: usize,
    pub query_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum A11yError {
    #[error("AT-SPI not available: {0}")]
    Unavailable(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("no snapshot found for state_id {0}")]
    NotFound(String),
}
