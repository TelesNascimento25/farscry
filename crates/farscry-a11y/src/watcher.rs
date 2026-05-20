use std::sync::Arc;
use std::time::Duration;

use crate::{store::A11yStore, types::A11ySnapshot};
use farscry_core::StateId;

pub async fn watch_and_store(store: Arc<A11yStore>) {
    #[cfg(target_os = "linux")]
    linux::run(store).await;

    #[cfg(not(target_os = "linux"))]
    let _ = store;
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use atspi::{accessible::AccessibleProxy, component::ComponentProxy, Connection};
    use std::ops::Deref;

    pub async fn run(store: Arc<A11yStore>) {
        eprintln!("[farscry:a11y] starting AT-SPI polling");
        loop {
            if let Err(e) = poll_once(&store).await {
                eprintln!("[farscry:a11y] poll error: {e}");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn poll_once(store: &A11yStore) -> Result<(), Box<dyn std::error::Error>> {
        let conn = Connection::open().await?;
        let zbus_conn = conn.deref().connection();

        let captured_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let nodes = walk_desktop(zbus_conn).await;

        if nodes.is_empty() {
            return Ok(());
        }

        let snap = A11ySnapshot {
            state_id: StateId::from_bits(
                nodes.iter().fold(0u64, |acc, n| {
                    acc.wrapping_add(n.x as u64)
                        .wrapping_add(n.y as u64)
                        .wrapping_add(n.role.len() as u64)
                }),
            ),
            captured_at_ms,
            app_name: nodes
                .first()
                .map(|n| n.description.clone())
                .unwrap_or_default(),
            nodes,
        };

        store.insert(&snap).await?;
        Ok(())
    }

    async fn walk_desktop(conn: &zbus::Connection) -> Vec<crate::types::A11yNode> {
        let mut results = Vec::new();

        let root = match AccessibleProxy::builder(conn)
            .destination("org.a11y.atspi.Registry")
            .unwrap()
            .path("/org/a11y/atspi/accessible/root")
            .unwrap()
            .build()
            .await
        {
            Ok(a) => a,
            Err(_) => return results,
        };

        let child_count = root.child_count().await.unwrap_or(0);
        for i in 0..child_count.min(20) {
            let child = match root.get_child_at_index(i).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            let child_proxy = match AccessibleProxy::builder(conn)
                .destination(child.0.as_str())
                .unwrap()
                .path(child.1.as_str())
                .unwrap()
                .build()
                .await
            {
                Ok(p) => p,
                Err(_) => continue,
            };
            Box::pin(scrape(&child_proxy, conn, &mut results, None, 0)).await;
        }

        results
    }

    async fn scrape(
        proxy: &AccessibleProxy<'_>,
        conn: &zbus::Connection,
        out: &mut Vec<crate::types::A11yNode>,
        parent_id: Option<i64>,
        depth: usize,
    ) {
        if depth > 6 || out.len() > 300 {
            return;
        }

        let role = proxy
            .get_role()
            .await
            .map(|r| format!("{r:?}").to_lowercase())
            .unwrap_or_default();
        let name = proxy.name().await.unwrap_or_default();
        let description = proxy.description().await.unwrap_or_default();
        let states = proxy
            .get_state()
            .await
            .map(|s| vec![format!("{s:?}")])
            .unwrap_or_default();

        let dest = proxy.inner().destination().to_string();
        let path = proxy.inner().path().to_string();

        let (x, y, w, h) = if let Ok(comp) = ComponentProxy::builder(conn)
            .destination(dest.as_str())
            .unwrap()
            .path(path.as_str())
            .unwrap()
            .build()
            .await
        {
            comp.get_extents(atspi::CoordType::Screen)
                .await
                .unwrap_or((0, 0, 0, 0))
        } else {
            (0, 0, 0, 0)
        };

        let seq = out.len() as i32;
        out.push(crate::types::A11yNode {
            row_id: 0,
            snapshot_id: 0,
            role,
            name,
            description,
            x,
            y,
            width: w,
            height: h,
            states,
            parent_row_id: parent_id,
            sequence: seq,
        });

        let my_id = out.len() as i64 - 1;
        let child_count = proxy.child_count().await.unwrap_or(0);

        for i in 0..child_count.min(20) {
            let child = match proxy.get_child_at_index(i).await {
                Ok(c) => c,
                Err(_) => continue,
            };
            if let Ok(child_proxy) = AccessibleProxy::builder(conn)
                .destination(child.0.as_str())
                .unwrap()
                .path(child.1.as_str())
                .unwrap()
                .build()
                .await
            {
                Box::pin(scrape(&child_proxy, conn, out, Some(my_id), depth + 1)).await;
            }
        }
    }
}
