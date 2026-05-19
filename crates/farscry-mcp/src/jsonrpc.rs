use serde_json::Value;

use crate::types::{DiffParams, ExtractParams, JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use crate::{McpServer, PipelineOps};

impl<P: PipelineOps> McpServer<P> {
    pub async fn handle_request(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let result = match request.method.as_str() {
            "farscry_extract" => self.handle_extract(request).await,
            "farscry_diff" => self.handle_diff(request).await,
            other => Err(format!("Unknown method: {other}")),
        };
        match result {
            Ok(result) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: Some(Value::String(result)),
                error: None,
                id: request.id.clone(),
            },
            Err(error) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: None,
                error: Some(JsonRpcError {
                    code: -32000,
                    message: error,
                }),
                id: request.id.clone(),
            },
        }
    }

    async fn handle_extract(&self, request: &JsonRpcRequest) -> Result<String, String> {
        let params: ExtractParams = serde_json::from_value(request.params.clone())
            .map_err(|e| format!("Invalid params: {e}"))?;
        let image_path = params.image_path;
        let (img_w, img_h) = image::image_dimensions(&image_path).unwrap_or((1920, 1080));
        let image_path_for_fmt = image_path.clone();
        let pipeline = self.pipeline.clone();
        let result = tokio::task::spawn_blocking(move || {
            pipeline
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .process(&image_path, false)
        })
        .await
        .map_err(|e| format!("Task error: {e}"))??;
        let auto_diff = self.compute_auto_diff(&result).await;
        *self.last_state.lock().unwrap_or_else(|p| p.into_inner()) = Some(result.clone());
        let vasp_text = farscry_formatter::VaspFormatter::format_vasp(
            &result,
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
        Ok(text)
    }

    async fn handle_diff(&self, request: &JsonRpcRequest) -> Result<String, String> {
        let params: DiffParams = serde_json::from_value(request.params.clone())
            .map_err(|e| format!("Invalid params: {e}"))?;
        let before_path = params.before;
        let after_path = params.after;
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
        let before = before_result.map_err(|e| format!("Task error: {e}"))??;
        let after = after_result.map_err(|e| format!("Task error: {e}"))??;
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
        .map_err(|e| format!("Task error: {e}"))?;
        Ok(farscry_formatter::VaspFormatter::format_diff(&delta))
    }
}
