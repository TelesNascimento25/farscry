use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub method: String,
    pub params: Value,
    pub id: Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
    pub id: Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ExtractParams {
    pub image_path: String,
    #[serde(default)]
    pub after_action: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DiffParams {
    pub before: String,
    pub after: String,
}
