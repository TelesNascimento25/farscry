use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;

use serde_json::Value;

use crate::helpers::{error_response, send_line};
use crate::{McpServer, PipelineOps};

impl<P: PipelineOps> McpServer<P> {
    pub fn new(pipeline: P) -> Self {
        Self {
            pipeline: Arc::new(Mutex::new(pipeline)),
            last_state: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(unix)]
    pub async fn serve_unix_with(socket_path: &Path, pipeline: P) -> Result<(), String> {
        if socket_path.exists() {
            std::fs::remove_file(socket_path)
                .map_err(|e| format!("Failed to remove socket: {e}"))?;
        }
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {e}"))?;
        }
        let listener =
            UnixListener::bind(socket_path).map_err(|e| format!("Failed to bind socket: {e}"))?;
        let server = McpServer::new(pipeline);
        eprintln!(
            "[farscry] MCP server listening on {}",
            socket_path.display()
        );
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let server_clone = server.clone();
                    tokio::spawn(async move {
                        if let Err(e) = server_clone.handle_unix_session(stream).await {
                            eprintln!("[farscry] session error: {e}");
                        }
                    });
                }
                Err(e) => eprintln!("[farscry] accept error: {e}"),
            }
        }
    }

    pub async fn serve_tcp_with(port: u16, pipeline: P) -> Result<(), String> {
        let addr = format!("127.0.0.1:{port}");
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("Failed to bind TCP: {e}"))?;
        let server = McpServer::new(pipeline);
        eprintln!("[farscry] MCP server listening on {addr}");
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let server_clone = server.clone();
                    tokio::spawn(async move {
                        if let Err(e) = server_clone.handle_tcp_session(stream).await {
                            eprintln!("[farscry] session error: {e}");
                        }
                    });
                }
                Err(e) => eprintln!("[farscry] accept error: {e}"),
            }
        }
    }

    #[cfg(unix)]
    pub(crate) async fn handle_unix_session(
        &self,
        stream: tokio::net::UnixStream,
    ) -> Result<(), String> {
        let (reader, writer) = tokio::io::split(stream);
        self.run_session(BufReader::new(reader), writer).await
    }

    pub(crate) async fn handle_tcp_session(
        &self,
        stream: tokio::net::TcpStream,
    ) -> Result<(), String> {
        let (reader, writer) = tokio::io::split(stream);
        self.run_session(BufReader::new(reader), writer).await
    }

    pub(crate) async fn run_session<R, W>(
        &self,
        mut reader: BufReader<R>,
        mut writer: W,
    ) -> Result<(), String>
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader
                .read_line(&mut line)
                .await
                .map_err(|e| format!("read error: {e}"))?;
            if n == 0 {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let msg: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[farscry] JSON parse error: {e}");
                    let err_resp = error_response(Value::Null, -32700, "Parse error");
                    send_line(&mut writer, &err_resp).await?;
                    continue;
                }
            };
            if let Some(response) = self.dispatch(&msg).await {
                send_line(&mut writer, &response).await?;
            }
        }
        Ok(())
    }
}
