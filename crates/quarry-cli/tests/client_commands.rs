#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP fixtures and server setup"
)]

use quarry_storage::{QuarryStore, StoreConfig};

/// Binds an ephemeral-port quarry server backed by a fresh temp store and
/// returns its `http://127.0.0.1:PORT` origin. The temp dir is returned so the
/// caller keeps it alive for the test's duration.
async fn spawn_server() -> (tempfile::TempDir, String) {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = quarry_server::router(store);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (root, format!("http://{addr}"))
}

fn secret_from_prompt(prompt: &str, server: &str) -> String {
    let marker = format!("{server}/tmp/");
    prompt
        .split(&marker)
        .nth(1)
        .expect("prompt should contain the tmp locator URL")
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect()
}

#[tokio::test]
async fn create_tmp_document_creates_empty_tmp_document_and_returns_prompt() {
    let (_root, server) = spawn_server().await;

    let prompt = quarry_cli::create_tmp_document(&server, None)
        .await
        .unwrap();

    assert!(prompt.contains(&format!("{server}/tmp/")));
    assert!(prompt.contains("Scope: tmp document"));
    assert!(prompt.contains("Connected in Quarry and ready."));

    let secret = secret_from_prompt(&prompt, &server);
    assert_eq!(secret.len(), 32);
    let rendered = reqwest::get(format!("{server}/v1/tmp/documents/{secret}"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(rendered.trim().is_empty(), "new document should be empty");
}

#[tokio::test]
async fn open_seeds_tmp_document_from_markdown() {
    let (_root, server) = spawn_server().await;

    let prompt =
        quarry_cli::create_tmp_document(&server, Some("# Draft\n\nHello world\n".to_string()))
            .await
            .unwrap();

    let secret = secret_from_prompt(&prompt, &server);
    let rendered = reqwest::get(format!("{server}/v1/tmp/documents/{secret}"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(rendered.contains("Draft"));
    assert!(rendered.contains("Hello world"));
}

#[tokio::test]
async fn unreachable_server_reports_a_helpful_error() {
    // Nothing is listening on this port.
    let error = quarry_cli::create_tmp_document("http://127.0.0.1:1", None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("could not reach quarry server"),
        "unexpected error: {error}"
    );
}
