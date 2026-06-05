use quarry_server::serve_with_shutdown;
use quarry_storage::{QuarryStore, StoreConfig};
use tokio::time::{timeout, Duration};

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
