use anyhow::Result;
use chrono::Utc;
use farscry_core::vasf::{VasfFrame, VasfWriter};
use farscry_core::{Pipeline, StateId, VaspDelta, VaspOutput};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn serve_mcp(
    mcp: bool,
    port: Option<u16>,
    record: Option<PathBuf>,
    hamming_threshold: u8,
) -> Result<()> {
    if !mcp {
        anyhow::bail!("Only MCP mode is currently supported");
    }
    let pipeline = crate::pipeline::get_or_build_pipeline()
        .map_err(|e| anyhow::anyhow!("Pipeline init failed: {e}"))?;
    let record_path = resolve_record_path(record, hamming_threshold).await?;
    let recorder = record_path
        .map(|p| SessionRecorder::new(p, hamming_threshold))
        .transpose()
        .map_err(|e| anyhow::anyhow!("Failed to create recorder: {e}"))?;
    let recorder_arc: Arc<Mutex<Option<SessionRecorder>>> = Arc::new(Mutex::new(recorder));
    let adapter = RecordingAdapter::new(pipeline, recorder_arc.clone());
    run_server(port, adapter).await;
    finalize_recorder(recorder_arc);
    Ok(())
}

async fn run_server<P: farscry_mcp::PipelineOps>(port: Option<u16>, adapter: P) {
    let task: tokio::task::JoinHandle<Result<(), String>> = if let Some(p) = port {
        tokio::spawn(async move { farscry_mcp::McpServer::serve_tcp_with(p, adapter).await })
    } else {
        #[cfg(unix)]
        {
            let socket_path = default_socket_path();
            tokio::spawn(async move {
                farscry_mcp::McpServer::serve_unix_with(&socket_path, adapter).await
            })
        }
        #[cfg(not(unix))]
        {
            eprintln!("[farscry] Unix sockets not supported on Windows. Use --port.");
            return;
        }
    };
    #[cfg(unix)]
    let sigterm = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        r = task => { if let Err(e) = r { eprintln!("[farscry] server error: {e}"); } }
        _ = tokio::signal::ctrl_c() => { eprintln!("[farscry] shutting down"); }
        _ = sigterm => { eprintln!("[farscry] shutting down (SIGTERM)"); }
    }
}

fn finalize_recorder(recorder_arc: Arc<Mutex<Option<SessionRecorder>>>) {
    if let Ok(mut guard) = recorder_arc.lock() {
        if let Some(mut rec) = guard.take() {
            rec.print_summary();
            let _ = rec.finalize();
        }
    }
}

async fn resolve_record_path(
    explicit: Option<PathBuf>,
    hamming_threshold: u8,
) -> Result<Option<PathBuf>> {
    if let Some(p) = explicit {
        return Ok(Some(p));
    }
    let cfg = crate::config::read_farscry_config();
    if let Some(sessions) = &cfg.sessions {
        if sessions.record == Some(true) {
            let dir = sessions
                .output_dir
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(crate::util::sessions_dir);
            std::fs::create_dir_all(&dir)?;
            let path = dir.join(timestamp_filename());
            eprintln!("[farscry] session recording: auto (from config)");
            eprintln!("[farscry] recording to {}", path.display());
            return Ok(Some(path));
        }
    }
    let _ = hamming_threshold;
    tokio::task::spawn_blocking(prompt_record_session)
        .await
        .map_err(|e| anyhow::anyhow!("prompt task: {e}"))
        .and_then(|r| r)
}

fn prompt_record_session() -> Result<Option<PathBuf>> {
    use std::io::Write;
    let dir = crate::util::sessions_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(timestamp_filename());
    eprint!("[farscry] session observability: disabled\nEnable recording? (y/N): ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if line.trim().eq_ignore_ascii_case("y") {
        eprintln!("[farscry] recording to {}", path.display());
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

pub fn default_socket_path_pub() -> std::path::PathBuf {
    default_socket_path()
}

fn default_socket_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".farscry")
        .join("mcp.sock")
}

fn timestamp_filename() -> String {
    let now = Utc::now();
    format!("{}.vasf", now.format("%Y-%m-%d-%H%M"))
}

struct SessionRecorder {
    writer: VasfWriter,
    last_state_id: Option<StateId>,
    pending_action_state_id: Option<StateId>,
    hamming_threshold: u8,
    output_path: PathBuf,
    start_time: i64,
}

impl SessionRecorder {
    fn new(output_path: PathBuf, hamming_threshold: u8) -> std::io::Result<Self> {
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let writer = VasfWriter::create(&output_path)?;
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        Ok(Self {
            writer,
            last_state_id: None,
            pending_action_state_id: None,
            hamming_threshold,
            output_path,
            start_time,
        })
    }

    fn do_mark_action(&mut self) -> Option<StateId> {
        self.pending_action_state_id = self.last_state_id;
        let _ = self.writer.append_action_marker();
        self.last_state_id
    }

    fn consume_action_effect(&mut self, new_state: StateId) -> Option<farscry_mcp::ActionEffect> {
        let before = self.pending_action_state_id.take()?;
        Some(if before == new_state {
            farscry_mcp::ActionEffect::SilentFailure {
                before,
                after: new_state,
            }
        } else {
            farscry_mcp::ActionEffect::Changed {
                before,
                after: new_state,
            }
        })
    }

    fn record(&mut self, vasp: &VaspOutput, image_path: &str, force: bool) {
        self.writer.total_input += 1;
        let is_new = self
            .last_state_id
            .map(|last| vasp.state_id.hamming(last) > self.hamming_threshold)
            .unwrap_or(true);
        if !is_new && !force {
            return;
        }
        let (w, h) = image::image_dimensions(image_path).unwrap_or((1920, 1080));
        let vasp_text = farscry_formatter::VaspFormatter::format_vasp(vasp, image_path, w, h);
        let frame = VasfFrame {
            state_id: vasp.state_id,
            timestamp: crate::util::now_ms(),
            vasp_data: vasp_text.into_bytes(),
            delta_data: None,
        };
        let _ = self.writer.append_frame(&frame);
        self.last_state_id = Some(vasp.state_id);
    }

    fn print_summary(&self) {
        let unique = self.writer.frame_count;
        let total = self.writer.total_input;
        let dupes = total.saturating_sub(unique);
        let pct = (dupes * 100).checked_div(total).unwrap_or(0);
        let tokens_raw = total as u64 * 2765;
        let tokens_vasf = unique as u64 * 200;
        let ratio = tokens_raw.checked_div(tokens_vasf).unwrap_or(0);
        eprintln!("[farscry] session complete");
        eprintln!("[farscry] unique states: {unique} of {total} frames ({pct}% deduplicated)");
        eprintln!("[farscry] tokens without farscry: ~{tokens_raw}");
        eprintln!("[farscry] tokens with farscry: ~{tokens_vasf}");
        eprintln!("[farscry] reduction: ~{ratio}x");
        eprintln!("[farscry] saved: {}", self.output_path.display());
        eprintln!(
            "[farscry] replay: farscry timeline {}",
            self.output_path.display()
        );
        let _ = self.start_time;
    }

    fn finalize(&mut self) -> std::io::Result<()> {
        self.writer.finalize()
    }
}

#[derive(Clone)]
struct RecordingAdapter {
    pipeline: Arc<Pipeline>,
    recorder: Arc<Mutex<Option<SessionRecorder>>>,
    last_effect: Arc<Mutex<Option<farscry_mcp::ActionEffect>>>,
}

impl RecordingAdapter {
    fn new(pipeline: Arc<Pipeline>, recorder: Arc<Mutex<Option<SessionRecorder>>>) -> Self {
        Self {
            pipeline,
            recorder,
            last_effect: Arc::new(Mutex::new(None)),
        }
    }
}

impl farscry_mcp::PipelineOps for RecordingAdapter {
    fn mark_action(&self) -> Option<farscry_core::StateId> {
        if let Ok(mut guard) = self.recorder.lock() {
            if let Some(rec) = guard.as_mut() {
                return rec.do_mark_action();
            }
        }
        None
    }

    fn last_action_effect(&self) -> Option<farscry_mcp::ActionEffect> {
        self.last_effect.lock().ok().and_then(|mut g| g.take())
    }

    fn process(&self, image_path: &str, after_action: bool) -> Result<VaspOutput, String> {
        if after_action {
            self.mark_action();
        }
        let img = image::open(image_path).map_err(|e| format!("cannot open image: {e}"))?;
        let output = self.pipeline.process(img).map_err(|e| e.to_string())?;
        if let Ok(mut guard) = self.recorder.lock() {
            if let Some(rec) = guard.as_mut() {
                let effect = rec.consume_action_effect(output.state_id);
                if let Some(eff) = effect {
                    if let Ok(mut eg) = self.last_effect.lock() {
                        *eg = Some(eff);
                    }
                }
                rec.record(&output, image_path, after_action);
            }
        }
        Ok(output)
    }

    fn diff(
        &self,
        before: &VaspOutput,
        after: &VaspOutput,
        before_dims: Option<(u32, u32)>,
        after_dims: Option<(u32, u32)>,
    ) -> VaspDelta {
        use farscry_core::DiffEngine;
        farscry_diff::DiffEngineImpl.diff(before, after, before_dims, after_dims)
    }
}
