use quarry_server::serve_with_shutdown;
#[cfg(feature = "lib-documents")]
use quarry_server::{app_state, serve_state_with_shutdown};
use quarry_storage::{QuarryStore, StoreConfig};
#[cfg(feature = "lib-documents")]
use tokio::io::AsyncWriteExt;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn serve_with_shutdown_exits_and_releases_store_lock() {
    let root = tempfile::tempdir().unwrap();
    let lock_path = root.path().join("quarry.lock");
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    assert!(lock_path.exists());

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve_with_shutdown(
        store,
        "127.0.0.1:0".parse().unwrap(),
        async {
            let _ = shutdown_rx.await;
        },
    ));

    shutdown_tx.send(()).unwrap();
    timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!lock_path.exists());
}

#[tokio::test]
#[cfg(feature = "lib-documents")]
async fn serve_state_with_shutdown_abandons_pending_request_after_grace_period() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    store.create_library("drain").await.unwrap();
    let state = app_state(store);

    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let mut server = tokio::spawn(serve_state_with_shutdown(state, addr, async {
        let _ = shutdown_rx.await;
    }));
    wait_for_server(addr).await;

    let mut pending = tokio::net::TcpStream::connect(addr).await.unwrap();
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
        .unwrap();
    tokio::task::yield_now().await;

    shutdown_tx.send(()).unwrap();
    assert!(
        timeout(Duration::from_secs(1), &mut server).await.is_err(),
        "server should wait for active request drain before grace expires"
    );
    timeout(Duration::from_secs(6), &mut server)
        .await
        .expect("server should abandon drain after grace period")
        .unwrap()
        .unwrap();
}

#[cfg(feature = "lib-documents")]
async fn wait_for_server(addr: std::net::SocketAddr) {
    timeout(Duration::from_secs(2), async {
        loop {
            match tokio::net::TcpStream::connect(addr).await {
                Ok(_) => break,
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .expect("server did not start listening");
}
