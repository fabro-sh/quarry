#![allow(clippy::unwrap_used, reason = "tests use unwrap for shutdown fixtures")]

use anyhow::Context;
use quarry_server::serve_with_shutdown;
#[cfg(feature = "lib-documents")]
use quarry_server::{app_state, serve_state_with_shutdown};
use quarry_storage::{QuarryStore, StoreConfig};
#[cfg(feature = "lib-documents")]
use tokio::io::AsyncWriteExt;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn serve_with_shutdown_exits_and_releases_store_lock() -> anyhow::Result<()> {
    let root = tempfile::tempdir().context("create shutdown tempdir")?;
    let lock_path = root.path().join("quarry.lock");
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .context("open shutdown test store")?;
    assert!(lock_path.exists());

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve_with_shutdown(
        store,
        "127.0.0.1:0".parse().context("parse loopback bind addr")?,
        async {
            let _ = shutdown_rx.await;
        },
    ));

    shutdown_tx
        .send(())
        .map_err(|_| anyhow::anyhow!("shutdown receiver dropped before signal"))?;
    timeout(Duration::from_secs(2), server)
        .await
        .context("server should exit within timeout")?
        .context("server task should join")?
        .context("server should exit cleanly")?;
    assert!(!lock_path.exists());
    Ok(())
}

#[tokio::test]
#[cfg(feature = "lib-documents")]
async fn serve_state_with_shutdown_abandons_pending_request_after_grace_period()
-> anyhow::Result<()> {
    let root = tempfile::tempdir().context("create drain tempdir")?;
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .context("open drain test store")?;
    store
        .create_library("drain")
        .await
        .context("create drain library")?;
    let state = app_state(store);

    let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind probe listener")?;
    let addr = probe.local_addr().context("read probe listener addr")?;
    drop(probe);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let mut server = tokio::spawn(serve_state_with_shutdown(state, addr, async {
        let _ = shutdown_rx.await;
    }));
    wait_for_server(addr).await?;

    let mut pending = tokio::net::TcpStream::connect(addr)
        .await
        .context("connect pending request socket")?;
    pending
        .write_all(
            b"PUT /v1/libraries/drain/documents/pending.md HTTP/1.1\r\n\
              Host: localhost\r\n\
              Content-Type: text/markdown\r\n\
              Content-Length: 100\r\n\
              \r\n\
              partial",
        )
        .await
        .context("write partial HTTP request")?;
    tokio::task::yield_now().await;

    shutdown_tx
        .send(())
        .map_err(|_| anyhow::anyhow!("shutdown receiver dropped before drain signal"))?;
    assert!(
        timeout(Duration::from_secs(1), &mut server).await.is_err(),
        "server should wait for active request drain before grace expires"
    );
    timeout(Duration::from_secs(6), &mut server)
        .await
        .context("server should abandon drain after grace period")?
        .context("server task should join")?
        .context("server should exit cleanly")?;
    Ok(())
}

#[cfg(feature = "lib-documents")]
async fn wait_for_server(addr: std::net::SocketAddr) -> anyhow::Result<()> {
    timeout(Duration::from_secs(2), async {
        loop {
            match tokio::net::TcpStream::connect(addr).await {
                Ok(_) => break,
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .context("server did not start listening")?;
    Ok(())
}
