mod helpers;
mod jsonrpc;
mod protocol;
mod transport;
pub mod types;

pub use types::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};

use farscry_core::{VaspDelta, VaspOutput};
use std::sync::{Arc, Mutex};

pub trait PipelineOps: Clone + Send + 'static {
    fn process(&self, image_path: &str, after_action: bool) -> Result<VaspOutput, String>;

    fn diff(
        &self,
        before: &VaspOutput,
        after: &VaspOutput,
        before_dims: Option<(u32, u32)>,
        after_dims: Option<(u32, u32)>,
    ) -> VaspDelta;
}

#[derive(Clone)]
pub struct McpServer<P> {
    pipeline: Arc<Mutex<P>>,
    last_state: Arc<Mutex<Option<VaspOutput>>>,
}

#[cfg(test)]
#[derive(Clone, Default)]
pub struct MockPipeline;

#[cfg(test)]
use farscry_core::{Confidence, ElementType, ScreenType, StateId, UiElement};

#[cfg(test)]
impl PipelineOps for MockPipeline {
    fn process(&self, _image_path: &str, _after_action: bool) -> Result<VaspOutput, String> {
        Ok(VaspOutput::new(
            StateId::from_bits(0x123456789ABCDEF0),
            ScreenType::Ui,
            Confidence::High,
            "eng",
            "Mock extraction",
            vec![UiElement {
                text: "Test Element".to_string(),
                element_type: ElementType::Label,
                cx: 100.0,
                cy: 100.0,
                w: 100.0,
                h: 30.0,
                enabled: None,
                value: None,
            }],
            vec![],
        ))
    }

    fn diff(
        &self,
        before: &VaspOutput,
        after: &VaspOutput,
        _before_dims: Option<(u32, u32)>,
        _after_dims: Option<(u32, u32)>,
    ) -> VaspDelta {
        VaspDelta {
            vasp_version: "1.0".to_string(),
            diff_from: before.state_id,
            diff_to: after.state_id,
            context_similarity: 1.0,
            context_changed: false,
            agent_context: "Mock diff".to_string(),
            entries: vec![],
            tokens_saved: Some(100),
        }
    }
}

#[cfg(test)]
impl McpServer<MockPipeline> {
    #[cfg(unix)]
    pub async fn serve_unix(socket_path: &std::path::Path) -> Result<(), String> {
        McpServer::<MockPipeline>::serve_unix_with(socket_path, MockPipeline).await
    }

    pub async fn serve_tcp(port: u16) -> Result<(), String> {
        McpServer::<MockPipeline>::serve_tcp_with(port, MockPipeline).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DiffParams, ExtractParams};

    #[test]
    fn test_mock_pipeline_process() {
        let pipeline = MockPipeline;
        let result = pipeline.process("test.png");
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.ui_tree.len(), 1);
        assert_eq!(output.ui_tree[0].text, "Test Element");
    }

    #[test]
    fn test_mock_pipeline_diff() {
        let pipeline = MockPipeline;
        let before = pipeline.process("before.png").unwrap();
        let after = pipeline.process("after.png").unwrap();
        let delta = pipeline.diff(&before, &after, None, None);
        assert_eq!(delta.context_similarity, 1.0);
        assert!(!delta.context_changed);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_server_starts_unix() {
        let temp_dir = tempfile::tempdir().unwrap();
        let socket_path = temp_dir.path().join("test.sock");

        let socket_path_clone = socket_path.clone();
        tokio::spawn(async move { McpServer::serve_unix(&socket_path_clone).await });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        assert!(socket_path.exists());
    }

    #[test]
    fn test_json_rpc_request_parse() {
        let json = r#"{"jsonrpc":"2.0","method":"farscry_extract","params":{"image_path":"test.png"},"id":1}"#;
        let request: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.method, "farscry_extract");
    }

    #[test]
    fn test_extract_params_parse() {
        let json = r#"{"image_path":"test.png","lang":"eng","affordances":true}"#;
        let params: ExtractParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.image_path, "test.png");
    }

    #[test]
    fn test_diff_params_parse() {
        let json = r#"{"before":"before.png","after":"after.png"}"#;
        let params: DiffParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.before, "before.png");
        assert_eq!(params.after, "after.png");
    }

    #[tokio::test]
    async fn test_handle_extract() {
        let server = McpServer::new(MockPipeline);
        let request = JsonRpcRequest {
            method: "farscry_extract".to_string(),
            params: serde_json::json!({"image_path": "test.png"}),
            id: serde_json::json!(1),
        };
        let response = server.handle_request(&request).await;
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[tokio::test]
    async fn test_handle_diff() {
        let server = McpServer::new(MockPipeline);
        let request = JsonRpcRequest {
            method: "farscry_diff".to_string(),
            params: serde_json::json!({"before": "before.png", "after": "after.png"}),
            id: serde_json::json!(1),
        };
        let response = server.handle_request(&request).await;
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[tokio::test]
    async fn test_concurrent_requests() {
        let server = McpServer::new(MockPipeline);
        let server_clone = server.clone();

        let req1 = JsonRpcRequest {
            method: "farscry_extract".to_string(),
            params: serde_json::json!({"image_path": "test1.png"}),
            id: serde_json::json!(1),
        };
        let req2 = JsonRpcRequest {
            method: "farscry_extract".to_string(),
            params: serde_json::json!({"image_path": "test2.png"}),
            id: serde_json::json!(2),
        };

        let (r1, r2) = tokio::join!(
            server.handle_request(&req1),
            server_clone.handle_request(&req2)
        );
        assert!(r1.result.is_some());
        assert!(r2.result.is_some());
    }

    #[tokio::test]
    async fn test_mcp_initialize() {
        let server = McpServer::new(MockPipeline);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test-client", "version": "1.0"}
            },
            "id": 1
        });

        let response = server.dispatch(&msg).await.unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);
        let result = &response["result"];
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "farscry");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn test_mcp_notifications_initialized_no_response() {
        let server = McpServer::new(MockPipeline);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });

        let response = server.dispatch(&msg).await;
        assert!(
            response.is_none(),
            "notifications must not produce a response"
        );
    }

    #[tokio::test]
    async fn test_mcp_tools_list() {
        let server = McpServer::new(MockPipeline);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tools/list",
            "params": {},
            "id": 2
        });

        let response = server.dispatch(&msg).await.unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        let tools = &response["result"]["tools"];
        assert!(tools.is_array());
        let tools_arr = tools.as_array().unwrap();
        assert_eq!(tools_arr.len(), 2);

        let tool_names: Vec<&str> = tools_arr
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(tool_names.contains(&"farscry_extract"));
        assert!(tool_names.contains(&"farscry_diff"));

        for tool in tools_arr {
            assert!(
                tool["inputSchema"].is_object(),
                "tool must have inputSchema"
            );
            assert!(
                tool["description"].is_string(),
                "tool must have description"
            );
        }
    }

    #[tokio::test]
    async fn test_mcp_tools_call_extract() {
        let server = McpServer::new(MockPipeline);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {
                "name": "farscry_extract",
                "arguments": {"image_path": "test.png"}
            },
            "id": 3
        });

        let response = server.dispatch(&msg).await.unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 3);
        let result = &response["result"];
        assert!(result["content"].is_array());
        assert_eq!(result["content"][0]["type"], "text");
        assert!(result["content"][0]["text"].is_string());
        assert_eq!(result["isError"], false);
    }

    #[tokio::test]
    async fn test_mcp_tools_call_diff() {
        let server = McpServer::new(MockPipeline);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {
                "name": "farscry_diff",
                "arguments": {"before": "before.png", "after": "after.png"}
            },
            "id": 4
        });

        let response = server.dispatch(&msg).await.unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        let result = &response["result"];
        assert_eq!(result["isError"], false);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("farscry diff"));
    }

    #[tokio::test]
    async fn test_mcp_tools_call_unknown_tool() {
        let server = McpServer::new(MockPipeline);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {
                "name": "nonexistent_tool",
                "arguments": {}
            },
            "id": 5
        });

        let response = server.dispatch(&msg).await.unwrap();
        assert!(response["error"].is_object());
    }

    #[tokio::test]
    async fn test_mcp_unknown_method() {
        let server = McpServer::new(MockPipeline);
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "unknown/method",
            "params": {},
            "id": 6
        });

        let response = server.dispatch(&msg).await.unwrap();
        assert!(response["error"].is_object());
        assert_eq!(response["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn test_full_mcp_handshake_sequence() {
        let server = McpServer::new(MockPipeline);

        let init = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name":"test","version":"1"}},
            "id": 1
        });
        let r = server.dispatch(&init).await.unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2024-11-05");

        let notif = serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert!(server.dispatch(&notif).await.is_none());

        let list = serde_json::json!({"jsonrpc":"2.0","method":"tools/list","params":{},"id":2});
        let r = server.dispatch(&list).await.unwrap();
        assert!(r["result"]["tools"].is_array());

        let call = serde_json::json!({
            "jsonrpc":"2.0","method":"tools/call",
            "params":{"name":"farscry_extract","arguments":{"image_path":"test.png"}},
            "id":3
        });
        let r = server.dispatch(&call).await.unwrap();
        assert_eq!(r["result"]["isError"], false);
    }
}
