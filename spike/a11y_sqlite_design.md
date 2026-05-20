# AT-SPI + SQLite: Spike Design

**Status**: Draft for human review — do NOT implement yet
**Scope**: AT-SPI accessibility tree as primary UI data source, SQLite storage, new `farscry_query` MCP tool

---

## Prerequisites — Do This First

**daemon.rs uses `thread::spawn` sync throughout.** serve.rs is already tokio-native. Before any
new feature, migrate daemon.rs from manual threads to tokio tasks. Without this, adding
AT-SPI (event-driven via D-Bus, wants async) on top of sync threads is compounding debt.

Migration target — daemon.rs becomes:

```
tokio runtime
 ├── capture_task     (1 FPS, pHash + VASF write)
 ├── a11y_task        (event-driven AT-SPI, 500ms cadence)  ← new
 ├── socket_server    (Unix socket, existing MCP requests)
 └── mcp_http_server  (HTTP, if port is set)
```

RSS target after migration: <20MB with all tasks running.

---

## Architecture

```
farscry serve --mcp
│
├─ tokio::spawn ──────────────────────────────────────────────────┐
│    A11yWatcher (Linux only)                                      │
│    ├─ atspi::Connection::open()                                  │
│    ├─ EventStream: FocusEvents, ObjectEvents                     │
│    ├─ on_event: scrape full tree from root                       │
│    └─ A11yStore::insert(snapshot)  ──────────────────┐          │
│                                                       │          │
├─ McpServer<RecordingAdapter>                          │          │
│    ├─ a11y_store: Option<Arc<A11yStore>>  ◄───────────┘          │
│    │                                                             │
│    ├─ tools/call: farscry_extract                               │
│    │    ├─ [1] A11yStore::latest_for_state()  (Linux)           │
│    │    │    ├─ OK  → enrich VaspOutput + return                │
│    │    │    └─ Err → fallback ↓                                │
│    │    └─ [2] Pipeline::process(image)  (OCR VASP)             │
│    │                                                             │
│    └─ tools/call: farscry_query  (Linux only)                   │
│         └─ A11yStore::query(params) → Vec<A11yNode>             │
│
└─ SessionRecorder (VASF) — unchanged
```

```
Fallback Chain

farscry_extract(image_path)
  │
  ├─[primary, cfg(linux)]──► AT-SPI A11yStore::latest_for_state()
  │                               │
  │                         ┌─────┴──────┐
  │                       Hit          Miss / Err
  │                         │              │
  │                  enrich VASP        fallback
  │                    + return           │
  │                                       ▼
  └─[fallback, all platforms]──► OCR Pipeline::process(image)
                                      │
                                ┌─────┴──────┐
                              OK           Err
                                │              │
                           return VASP   VaspOutput { screen_type: Error }
```

---

## A11y Task Architecture — Zero Latency on Capture

AT-SPI is slow by design (D-Bus round-trips, 20-50ms per tree walk). It must never block
the capture_task. The correct separation:

```
capture_task (1 FPS)
  └── reads:  Arc<RwLock<Option<A11ySnapshot>>>   ← non-blocking read
  └── writes: VasfFrame enriched with latest snapshot (best-effort)

a11y_task (event-driven OR 500ms timer)
  └── walks AT-SPI tree
  └── writes: Arc<RwLock<Option<A11ySnapshot>>>
```

The capture_task takes a read lock and immediately moves on. If a11y has not written yet,
the frame is written without a11y data. Contract: **best-effort enrichment, never blocking**.

SSIM note: SSIM is O(W×H) — ~2M operations at 1080p. pHash is O(32×32) after downsample.
SSIM must never enter the runtime capture loop. Only valid use: offline corpus validation
(identify pHash false positives in VASF sessions). At runtime, pHash is the only hash.

---

## Cargo.toml Additions

`rusqlite` is replaced by `sqlx` — async-native, fits tokio without adapters.

Workspace root additions:

```toml
[workspace.dependencies]
sqlx = { version = "0.7", features = ["sqlite", "runtime-tokio"] }
libsqlite3-sys = { version = "0.27", features = ["bundled"] }

[target.'cfg(target_os = "linux")'.workspace.dependencies]
atspi = { version = "0.6", default-features = false, features = ["async-std"] }
zbus = { version = "4", default-features = false, features = ["tokio"] }
```

New crate `crates/farscry-a11y/Cargo.toml`:

```toml
[package]
name = "farscry-a11y"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish = false

[lib]
path = "src/lib.rs"

[dependencies]
farscry-core = { path = "../farscry-core" }
sqlx = { workspace = true }
libsqlite3-sys = { workspace = true }
serde = { workspace = true }
serde_json = "1"
thiserror = { workspace = true }
tokio = { version = "1", features = ["full"] }

[target.'cfg(target_os = "linux")'.dependencies]
atspi = { workspace = true }
zbus = { workspace = true }
```

`crates/farscry-mcp/Cargo.toml` addition:

```toml
[dependencies]
farscry-a11y = { path = "../farscry-a11y", optional = true }

[features]
a11y = ["farscry-a11y"]
```

---

## Data Structures

`crates/farscry-a11y/src/types.rs`:

```rust
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

#[derive(Debug, Clone)]
pub struct A11yStore {
    pub db_path: std::path::PathBuf,
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
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("No snapshot found for state_id {0}")]
    NotFound(String),
}
```

---

## SQLite Schema

```sql
CREATE TABLE IF NOT EXISTS snapshots (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    state_id_hex     TEXT    NOT NULL,
    state_id_bits    INTEGER NOT NULL,
    app_name         TEXT    NOT NULL DEFAULT '',
    captured_at_ms   INTEGER NOT NULL,
    node_count       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS a11y_nodes (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    snapshot_id      INTEGER NOT NULL
                     REFERENCES snapshots(id) ON DELETE CASCADE,
    role             TEXT    NOT NULL DEFAULT '',
    name             TEXT    NOT NULL DEFAULT '',
    description      TEXT    NOT NULL DEFAULT '',
    x                INTEGER NOT NULL DEFAULT 0,
    y                INTEGER NOT NULL DEFAULT 0,
    width            INTEGER NOT NULL DEFAULT 0,
    height           INTEGER NOT NULL DEFAULT 0,
    states_json      TEXT    NOT NULL DEFAULT '[]',
    parent_node_id   INTEGER,
    sequence         INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_snap_state_bits ON snapshots(state_id_bits);
CREATE INDEX IF NOT EXISTS idx_snap_app        ON snapshots(app_name);
CREATE INDEX IF NOT EXISTS idx_snap_ts         ON snapshots(captured_at_ms DESC);
CREATE INDEX IF NOT EXISTS idx_nodes_snapshot  ON a11y_nodes(snapshot_id);
CREATE INDEX IF NOT EXISTS idx_nodes_role      ON a11y_nodes(role);
CREATE INDEX IF NOT EXISTS idx_nodes_name      ON a11y_nodes(name);
```

---

## farscry_query MCP Tool

Tool schema (add to `mcp_tools_list`):

```rust
fn query_tool_schema() -> Value {
    serde_json::json!({
        "name": "farscry_query",
        "description": "Query the AT-SPI accessibility tree. Returns elements with exact OS-native coordinates. Linux only — returns empty on other platforms.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "role": { "type": "string" },
                "name_contains": { "type": "string" },
                "app_name": { "type": "string" },
                "state_id": { "type": "string" },
                "limit": { "type": "integer", "default": 50 }
            }
        }
    })
}
```

Handler (add to `mcp_tools_call`):

```rust
"farscry_query" => self.handle_mcp_query(&arguments).await,
```

```rust
async fn handle_mcp_query(&self, arguments: &Value) -> Result<Value, Value> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = arguments;
        return Ok(tool_result_text("farscry_query: Linux only"));
    }

    #[cfg(target_os = "linux")]
    {
        use farscry_a11y::types::A11yQueryParams;
        let params = A11yQueryParams {
            role: arguments.get("role").and_then(Value::as_str).map(str::to_string),
            name_contains: arguments.get("name_contains").and_then(Value::as_str).map(str::to_string),
            app_name: arguments.get("app_name").and_then(Value::as_str).map(str::to_string),
            state_id_hex: arguments.get("state_id").and_then(Value::as_str).map(str::to_string),
            limit: arguments.get("limit").and_then(Value::as_u64).map(|n| n as u32),
        };
        let store = match self.a11y_store.as_ref() {
            Some(s) => s.clone(),
            None => return Ok(tool_result_text("farscry_query: a11y store not initialized")),
        };
        let result = tokio::task::spawn_blocking(move || store.query(&params))
            .await
            .map_err(|e| mcp_error(-32000, &format!("spawn: {e}")))?
            .map_err(|e| mcp_error(-32000, &format!("query: {e}")))?;
        Ok(tool_result_text(&serde_json::to_string_pretty(&result).unwrap_or_default()))
    }
}
```

---

## serve.rs Integration

New field in `RecordingAdapter`:

```rust
#[derive(Clone)]
struct RecordingAdapter {
    pipeline: Arc<Pipeline>,
    recorder: Arc<Mutex<Option<SessionRecorder>>>,
    last_effect: Arc<Mutex<Option<farscry_mcp::ActionEffect>>>,
    consecutive_sf: Arc<Mutex<usize>>,
    #[cfg(target_os = "linux")]
    a11y_store: Option<Arc<farscry_a11y::A11yStore>>,
}
```

A11y watcher spawn in `serve_mcp`:

```rust
#[cfg(target_os = "linux")]
let a11y_store = {
    let db_path = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".farscry")
        .join("a11y.db");
    match farscry_a11y::A11yStore::open(&db_path) {
        Ok(store) => {
            let store_arc = Arc::new(store);
            let watcher = store_arc.clone();
            tokio::spawn(async move { farscry_a11y::watch_and_store(watcher).await });
            Some(store_arc)
        }
        Err(e) => {
            eprintln!("[farscry] a11y unavailable: {e}");
            None
        }
    }
};
```

---

## Blocker Matrix

| Item | Status | Notes |
|---|---|---|
| serve.rs uses tokio | CONFIRMED | async fn, tokio::spawn, tokio::select! |
| MCP supports new tools | CONFIRMED | trivial match arm addition |
| AT-SPI Linux-only | MANAGED | cfg gates everywhere |
| CI Windows/macOS | MANAGED | non-linux stub returns empty result |
| No comments in .rs | REQUIRED | enforced by CI pre-commit |
| rusqlite bundled | OK | cc already in build-deps |
| atspi + tokio compat | OK | zbus/tokio feature |
| 117 existing tests | WATCH | McpServer::new() signature change risk |
| AT-SPI in OSWorld Docker | UNKNOWN | verify at-spi2-core before investing |

---

## Open Questions

1. Does OSWorld Docker have `at-spi2-core` running? Determines if the integration is testable immediately.
2. Enrich VaspOutput or replace? Design above enriches (adds enabled/states). Replacing OCR entirely on Linux is an option.
3. Which StateId for indexing AT-SPI snapshots? Current proposal: phash of co-captured screenshot. Alternative: hash of AT-SPI tree itself.
4. SQLite TTL: 24h proposed. OSWorld sessions may need full session retention.
5. `McpServer::new()` backward compat: builder pattern or `new_with_a11y()` to avoid breaking external users.
