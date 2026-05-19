use farscry_core::{VaspDelta, VaspOutput};
use serde_json::Value;

use crate::helpers::{mcp_error, tool_result_error, tool_result_text};
use crate::types::JsonRpcRequest;
use crate::{McpServer, PipelineOps};

impl<P: PipelineOps> McpServer<P> {
    pub(crate) async fn dispatch(&self, msg: &Value) -> Option<Value> {
        let method = msg.get("method")?.as_str()?;

        let id_opt = msg.get("id");
        let is_notification = id_opt.is_none() || id_opt.map(Value::is_null).unwrap_or(false);
        if is_notification {
            return None;
        }
        let id = id_opt.cloned().unwrap_or(Value::Null);
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        let result: Result<Value, Value> = match method {
            "initialize" => self.mcp_initialize(&params),
            "tools/list" => self.mcp_tools_list(),
            "tools/call" => self.mcp_tools_call(&params).await,
            "farscry_extract" | "farscry_diff" => {
                let req = JsonRpcRequest {
                    method: method.to_string(),
                    params,
                    id: id.clone(),
                };
                let resp = self.handle_request(&req).await;
                match (resp.result, resp.error) {
                    (Some(r), _) => Ok(r),
                    (_, Some(e)) => Err(serde_json::json!({"code": e.code, "message": e.message})),
                    _ => Ok(Value::Null),
                }
            }
            other => Err(serde_json::json!({
                "code": -32601,
                "message": format!("Method not found: {other}")
            })),
        };

        Some(match result {
            Ok(r) => serde_json::json!({"jsonrpc": "2.0", "result": r, "id": id}),
            Err(e) => serde_json::json!({"jsonrpc": "2.0", "error": e, "id": id}),
        })
    }

    fn mcp_initialize(&self, _params: &Value) -> Result<Value, Value> {
        Ok(serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "farscry", "version": "0.1.0" }
        }))
    }

    fn mcp_tools_list(&self) -> Result<Value, Value> {
        Ok(serde_json::json!({
            "tools": [
                Self::extract_tool_schema(),
                Self::diff_tool_schema(),
                Self::mark_action_tool_schema()
            ]
        }))
    }

    fn extract_tool_schema() -> Value {
        serde_json::json!({
            "name": "farscry_extract",
            "description": "Converts one or more screenshots into VASP structured context. Returns typed, coordinate-rich UI elements with positions and affordances. Pass image_path for a single image or image_paths for multiple.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "image_path": {
                        "type": "string",
                        "description": "Absolute path to a single image file (PNG, JPG, WebP, GIF, TIFF)"
                    },
                    "image_paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Absolute paths to multiple image files - processed in parallel, outputs separated by ---"
                    },
                    "lang": {
                        "type": "string",
                        "description": "OCR language code, e.g. 'eng', 'por', 'chi_sim'",
                        "default": "eng"
                    },
                    "affordances": {
                        "type": "boolean",
                        "description": "Include affordances (click/type targets) in output",
                        "default": true
                    },
                    "after_action": {
                        "type": "boolean",
                        "description": "Set to true when called immediately after a computer-use action (click, type, keypress). Records an action marker in the session file so silent failures can be measured precisely. Silent failure = action produced no visual change.",
                        "default": false
                    }
                }
            }
        })
    }

    fn mark_action_tool_schema() -> Value {
        serde_json::json!({
            "name": "farscry_mark_action",
            "description": "Write an action marker to the current session. Call this BEFORE taking a computer-use action (click, type, keypress). farscry will compare the visual state before and after to detect silent failures (AER). No parameters required.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        })
    }

    fn diff_tool_schema() -> Value {
        serde_json::json!({
            "name": "farscry_diff",
            "description": "Returns semantic delta between two screenshots - appeared, changed, removed elements. Saves tokens vs re-sending full images to vision APIs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "before": {
                        "type": "string",
                        "description": "Absolute path to the before-action image"
                    },
                    "after": {
                        "type": "string",
                        "description": "Absolute path to the after-action image"
                    }
                },
                "required": ["before", "after"]
            }
        })
    }

    pub(crate) async fn mcp_tools_call(&self, params: &Value) -> Result<Value, Value> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| mcp_error(-32602, "Missing tool name in params"))?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        match name {
            "farscry_extract" => self.handle_mcp_extract_tool(&arguments).await,
            "farscry_diff" => self.handle_mcp_diff_tool(&arguments).await,
            "farscry_mark_action" => self.handle_mcp_mark_action().await,
            other => Err(mcp_error(-32602, &format!("Unknown tool: {other}"))),
        }
    }

    async fn handle_mcp_extract_tool(&self, arguments: &Value) -> Result<Value, Value> {
        let multi_paths: Vec<String> = arguments
            .get("image_paths")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if !multi_paths.is_empty() {
            return self.handle_mcp_extract_multi(multi_paths).await;
        }
        let image_path = arguments
            .get("image_path")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                mcp_error(
                    -32602,
                    "Provide image_path (single) or image_paths (multiple)",
                )
            })?
            .to_string();
        let after_action = arguments
            .get("after_action")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (img_w, img_h) = image::image_dimensions(&image_path).unwrap_or((1920, 1080));
        let image_path_for_fmt = image_path.clone();
        let pipeline = self.pipeline.clone();
        let result = tokio::task::spawn_blocking(move || {
            pipeline
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .process(&image_path, after_action)
        })
        .await
        .map_err(|e| mcp_error(-32000, &format!("Spawn error: {e}")))?;
        match result {
            Ok(output) => {
                let auto_diff = self.compute_auto_diff(&output).await;
                *self.last_state.lock().unwrap_or_else(|p| p.into_inner()) = Some(output.clone());
                let vasp_text = farscry_formatter::VaspFormatter::format_vasp(
                    &output,
                    &image_path_for_fmt,
                    img_w,
                    img_h,
                );
                let text = match auto_diff {
                    Some(ref d) => format!(
                        "{vasp_text}\n=== diff from previous state ===\n{}",
                        farscry_formatter::VaspFormatter::format_diff(d)
                    ),
                    None => vasp_text,
                };
                Ok(tool_result_text(&text))
            }
            Err(e) => Ok(tool_result_error(&format!("farscry_extract failed: {e}"))),
        }
    }

    async fn handle_mcp_extract_multi(&self, multi_paths: Vec<String>) -> Result<Value, Value> {
        let mut tasks = Vec::new();
        for path in multi_paths {
            let pipeline = self.pipeline.clone();
            let task = tokio::task::spawn_blocking(move || {
                let dims = image::image_dimensions(&path).unwrap_or((1920, 1080));
                let result = pipeline
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .process(&path, false);
                (path, dims, result)
            });
            tasks.push(task);
        }
        let mut combined = String::new();
        for (i, task) in tasks.into_iter().enumerate() {
            let (path, (img_w, img_h), result) = task
                .await
                .map_err(|e| mcp_error(-32000, &format!("Spawn error: {e}")))?;
            if i > 0 {
                combined.push_str("\n---\n");
            }
            match result {
                Ok(output) => {
                    combined.push_str(&farscry_formatter::VaspFormatter::format_vasp(
                        &output, &path, img_w, img_h,
                    ));
                }
                Err(e) => combined.push_str(&format!("Error processing {path}: {e}\n")),
            }
        }
        Ok(tool_result_text(&combined))
    }

    async fn handle_mcp_mark_action(&self) -> Result<Value, Value> {
        self.pipeline
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .mark_action();
        Ok(crate::helpers::tool_result_text(
            "action_marker: written\nfarscry will compare visual state before and after this action.",
        ))
    }

    async fn handle_mcp_diff_tool(&self, arguments: &Value) -> Result<Value, Value> {
        let before_path = arguments
            .get("before")
            .and_then(Value::as_str)
            .ok_or_else(|| mcp_error(-32602, "Missing required argument: before"))?
            .to_string();
        let after_path = arguments
            .get("after")
            .and_then(Value::as_str)
            .ok_or_else(|| mcp_error(-32602, "Missing required argument: after"))?
            .to_string();
        let before_dims = image::image_dimensions(&before_path).ok();
        let after_dims = image::image_dimensions(&after_path).ok();
        let pipeline1 = self.pipeline.clone();
        let pipeline2 = self.pipeline.clone();
        let (before_result, after_result) = tokio::join!(
            tokio::task::spawn_blocking(move || {
                pipeline1
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .process(&before_path, false)
            }),
            tokio::task::spawn_blocking(move || {
                pipeline2
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .process(&after_path, false)
            })
        );
        let before = before_result
            .map_err(|e| mcp_error(-32000, &format!("Spawn error: {e}")))?
            .map_err(|e| mcp_error(-32000, &format!("before failed: {e}")))?;
        let after = after_result
            .map_err(|e| mcp_error(-32000, &format!("Spawn error: {e}")))?
            .map_err(|e| mcp_error(-32000, &format!("after failed: {e}")))?;
        let pipeline3 = self.pipeline.clone();
        let delta = tokio::task::spawn_blocking(move || {
            pipeline3.lock().unwrap_or_else(|p| p.into_inner()).diff(
                &before,
                &after,
                before_dims,
                after_dims,
            )
        })
        .await
        .map_err(|e| mcp_error(-32000, &format!("Spawn error: {e}")))?;
        Ok(tool_result_text(
            &farscry_formatter::VaspFormatter::format_diff(&delta),
        ))
    }

    pub(crate) async fn compute_auto_diff(&self, current: &VaspOutput) -> Option<VaspDelta> {
        let last = self
            .last_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()?;
        let pipeline = self.pipeline.clone();
        let current_clone = current.clone();
        tokio::task::spawn_blocking(move || {
            pipeline.lock().unwrap_or_else(|p| p.into_inner()).diff(
                &last,
                &current_clone,
                None,
                None,
            )
        })
        .await
        .ok()
    }
}
