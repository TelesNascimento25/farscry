use std::sync::Arc;

use crate::store::A11yStore;

pub async fn watch_and_store(store: Arc<A11yStore>) {
    #[cfg(target_os = "linux")]
    linux::run(store).await;

    #[cfg(not(target_os = "linux"))]
    {
        let _ = store;
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::{
        store::A11yStore,
        types::{A11yNode, A11ySnapshot},
    };
    use farscry_core::StateId;

    pub async fn run(store: Arc<A11yStore>) {
        match try_connect_and_watch(store).await {
            Ok(()) => {}
            Err(e) => {
                eprintln!("[farscry:a11y] watcher stopped: {e}");
            }
        }
    }

    async fn try_connect_and_watch(store: Arc<A11yStore>) -> Result<(), Box<dyn std::error::Error>> {
        let conn = match atspi::Connection::open().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[farscry:a11y] AT-SPI unavailable, running without a11y enrichment: {e}");
                return Ok(());
            }
        };

        conn.register_event::<atspi::events::object::StateChangedEvent>().await?;
        conn.register_event::<atspi::events::focus::FocusEvent>().await?;

        eprintln!("[farscry:a11y] watching AT-SPI events");

        let mut stream = conn.event_stream();
        use futures_lite::StreamExt;

        let mut last_scrape_ms: i64 = 0;
        let throttle_ms: i64 = 500;

        loop {
            match tokio::time::timeout(Duration::from_millis(600), stream.next()).await {
                Ok(Some(Ok(_event))) => {
                    let now_ms = now_ms();
                    if now_ms - last_scrape_ms < throttle_ms {
                        continue;
                    }
                    last_scrape_ms = now_ms;

                    if let Ok(snap) = scrape_focused(&conn, now_ms).await {
                        store.insert(&snap).await.ok();
                    }
                }
                Ok(Some(Err(e))) => {
                    eprintln!("[farscry:a11y] event error: {e}");
                }
                Ok(None) => break,
                Err(_timeout) => {}
            }
        }

        Ok(())
    }

    async fn scrape_focused(
        conn: &atspi::Connection,
        captured_at_ms: i64,
    ) -> Result<A11ySnapshot, Box<dyn std::error::Error>> {
        use atspi::proxy::accessible::AccessibleProxy;

        let desktop = AccessibleProxy::new(conn.connection()).await?;
        let app_name = desktop.name().await.unwrap_or_else(|_| "unknown".into());

        let mut nodes = Vec::new();
        scrape_node(&desktop, &mut nodes, None, 0).await;

        Ok(A11ySnapshot {
            state_id: StateId::from_bits(0),
            captured_at_ms,
            app_name,
            nodes,
        })
    }

    async fn scrape_node(
        proxy: &atspi::proxy::accessible::AccessibleProxy<'_>,
        out: &mut Vec<A11yNode>,
        parent_id: Option<i64>,
        depth: usize,
    ) {
        if depth > 8 || out.len() > 500 {
            return;
        }
        use atspi::proxy::accessible::AccessibleProxy;
        use atspi::proxy::component::ComponentProxy;

        let role = proxy.get_role().await.map(|r| format!("{r:?}")).unwrap_or_default();
        let name = proxy.name().await.unwrap_or_default();
        let description = proxy.description().await.unwrap_or_default();
        let states: Vec<String> = proxy
            .get_state()
            .await
            .map(|s| {
                let sv: atspi::StateSet = s;
                format!("{sv:?}")
                    .split('|')
                    .map(|s| s.trim().to_string())
                    .collect()
            })
            .unwrap_or_default();

        let (x, y, w, h) = if let Ok(comp) = ComponentProxy::builder(proxy.connection())
            .destination(proxy.destination().clone())
            .unwrap_or_else(|_| ComponentProxy::builder(proxy.connection()))
            .path(proxy.path().clone())
            .build()
            .await
        {
            comp.get_extents(atspi::CoordType::Screen)
                .await
                .map(|(x, y, w, h)| (x, y, w, h))
                .unwrap_or((0, 0, 0, 0))
        } else {
            (0, 0, 0, 0)
        };

        let seq = out.len() as i32;
        out.push(A11yNode {
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
        for i in 0..child_count.min(30) {
            if let Ok(child) = proxy.get_child_at_index(i).await {
                if let Ok(child_proxy) = AccessibleProxy::builder(proxy.connection())
                    .destination(child.name)
                    .unwrap_or_else(|_| {
                        AccessibleProxy::builder(proxy.connection())
                    })
                    .path(child.path)
                    .build()
                    .await
                {
                    Box::pin(scrape_node(&child_proxy, out, Some(my_id), depth + 1)).await;
                }
            }
        }
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }
}
