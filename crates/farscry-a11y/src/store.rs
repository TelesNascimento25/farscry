use std::{path::PathBuf, sync::Arc, time::Instant};

use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};

use crate::types::{A11yError, A11yNode, A11yQueryParams, A11yQueryResult, A11ySnapshot};

const SCHEMA_STMTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS snapshots (
        id               INTEGER PRIMARY KEY AUTOINCREMENT,
        state_id_hex     TEXT    NOT NULL,
        state_id_bits    INTEGER NOT NULL,
        app_name         TEXT    NOT NULL DEFAULT '',
        captured_at_ms   INTEGER NOT NULL,
        node_count       INTEGER NOT NULL DEFAULT 0
    )",
    "CREATE TABLE IF NOT EXISTS a11y_nodes (
        id               INTEGER PRIMARY KEY AUTOINCREMENT,
        snapshot_id      INTEGER NOT NULL,
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
    )",
    "CREATE INDEX IF NOT EXISTS idx_snap_state_bits ON snapshots(state_id_bits)",
    "CREATE INDEX IF NOT EXISTS idx_snap_app        ON snapshots(app_name)",
    "CREATE INDEX IF NOT EXISTS idx_snap_ts         ON snapshots(captured_at_ms)",
    "CREATE INDEX IF NOT EXISTS idx_nodes_snapshot  ON a11y_nodes(snapshot_id)",
    "CREATE INDEX IF NOT EXISTS idx_nodes_role      ON a11y_nodes(role)",
    "CREATE INDEX IF NOT EXISTS idx_nodes_name      ON a11y_nodes(name)",
];

#[derive(Clone)]
pub struct A11yStore {
    pool: Arc<SqlitePool>,
}

impl A11yStore {
    pub async fn open(db_path: &PathBuf) -> Result<Self, A11yError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await?;
        for stmt in SCHEMA_STMTS {
            sqlx::query(stmt).execute(&pool).await?;
        }
        Ok(Self { pool: Arc::new(pool) })
    }

    pub async fn insert(&self, snapshot: &A11ySnapshot) -> Result<i64, A11yError> {
        let state_bits = snapshot.state_id.to_bits() as i64;
        let state_hex = format!("{:016x}", state_bits);
        let node_count = snapshot.nodes.len() as i64;

        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO snapshots (state_id_hex, state_id_bits, app_name, captured_at_ms, node_count)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&state_hex)
        .bind(state_bits)
        .bind(&snapshot.app_name)
        .bind(snapshot.captured_at_ms)
        .bind(node_count)
        .execute(&mut *tx)
        .await?;

        let snap_id: i64 =
            sqlx::query_scalar("SELECT last_insert_rowid()")
                .fetch_one(&mut *tx)
                .await?;

        for (seq, node) in snapshot.nodes.iter().enumerate() {
            let states = serde_json::to_string(&node.states).unwrap_or_else(|_| "[]".into());
            sqlx::query(
                "INSERT INTO a11y_nodes
                 (snapshot_id, role, name, description, x, y, width, height, states_json, parent_node_id, sequence)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(snap_id)
            .bind(&node.role)
            .bind(&node.name)
            .bind(&node.description)
            .bind(node.x)
            .bind(node.y)
            .bind(node.width)
            .bind(node.height)
            .bind(&states)
            .bind(node.parent_row_id)
            .bind(seq as i32)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        self.evict_old().await?;
        Ok(snap_id)
    }

    pub async fn query(&self, params: &A11yQueryParams) -> Result<A11yQueryResult, A11yError> {
        let t0 = Instant::now();
        let limit = params.limit.unwrap_or(50).min(500) as i64;
        let role_filter = params.role.clone();
        let name_filter = params.name_contains.as_ref().map(|n| format!("%{n}%"));
        let app_filter = params.app_name.clone();
        let state_id_filter = params.state_id_hex.clone();

        let role_pat = role_filter.as_deref().unwrap_or("%");
        let name_pat = name_filter.as_deref().unwrap_or("%");
        let app_pat = app_filter.as_deref().unwrap_or("%");
        let state_pat = state_id_filter.as_deref().unwrap_or("%");

        let rows = sqlx::query(
            "SELECT n.id, n.snapshot_id, n.role, n.name, n.description,
                    n.x, n.y, n.width, n.height, n.states_json, n.parent_node_id, n.sequence
             FROM a11y_nodes n
             JOIN snapshots s ON s.id = n.snapshot_id
             WHERE n.role LIKE ?
               AND n.name LIKE ?
               AND s.app_name LIKE ?
               AND s.state_id_hex LIKE ?
             ORDER BY s.captured_at_ms DESC
             LIMIT ?",
        )
        .bind(role_pat)
        .bind(name_pat)
        .bind(app_pat)
        .bind(state_pat)
        .bind(limit)
        .fetch_all(&*self.pool)
        .await?;

        let nodes: Vec<A11yNode> = rows
            .iter()
            .map(|r| {
                let states_json: String = r.get("states_json");
                let states: Vec<String> = serde_json::from_str(&states_json).unwrap_or_default();
                A11yNode {
                    row_id: r.get("id"),
                    snapshot_id: r.get("snapshot_id"),
                    role: r.get("role"),
                    name: r.get("name"),
                    description: r.get("description"),
                    x: r.get("x"),
                    y: r.get("y"),
                    width: r.get("width"),
                    height: r.get("height"),
                    states,
                    parent_row_id: r.get("parent_node_id"),
                    sequence: r.get("sequence"),
                }
            })
            .collect();

        let snapshot_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM snapshots")
            .fetch_one(&*self.pool)
            .await
            .unwrap_or(0);

        Ok(A11yQueryResult {
            nodes,
            snapshot_count: snapshot_count as usize,
            query_ms: t0.elapsed().as_millis() as u64,
        })
    }

    async fn evict_old(&self) -> Result<(), A11yError> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let cutoff_ms = now_ms - 86_400_000;
        sqlx::query("DELETE FROM snapshots WHERE captured_at_ms < ?")
            .bind(cutoff_ms)
            .execute(&*self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use farscry_core::StateId;

    #[tokio::test]
    async fn test_insert_and_query() {
        let tmp = std::env::temp_dir().join("farscry_a11y_test.db");
        let _ = std::fs::remove_file(&tmp);

        let store = A11yStore::open(&tmp).await.unwrap();

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let snap = A11ySnapshot {
            state_id: StateId::from_bits(0xdeadbeef),
            captured_at_ms: now_ms,
            app_name: "test_app".into(),
            nodes: vec![A11yNode {
                row_id: 0,
                snapshot_id: 0,
                role: "push button".into(),
                name: "Save".into(),
                description: String::new(),
                x: 640,
                y: 480,
                width: 80,
                height: 30,
                states: vec!["enabled".into()],
                parent_row_id: None,
                sequence: 0,
            }],
        };

        store.insert(&snap).await.unwrap();

        let all = store.query(&A11yQueryParams::default()).await.unwrap();
        assert_eq!(all.nodes.len(), 1, "all query returned {} nodes", all.nodes.len());

        let filtered = store
            .query(&A11yQueryParams {
                role: Some("push button".into()),
                name_contains: Some("Sa".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(filtered.nodes.len(), 1);
        assert_eq!(filtered.nodes[0].name, "Save");
        assert_eq!(filtered.nodes[0].x, 640);
        assert_eq!(filtered.nodes[0].y, 480);

        let _ = std::fs::remove_file(&tmp);
    }
}
