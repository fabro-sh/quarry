#![cfg(feature = "lib-documents")]
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for HTTP and CRDT fixtures"
)]

use anyhow::Context as _;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

mod common;

use common::{document_test_app, json_request, response_json};

async fn block_test_app() -> (tempfile::TempDir, axum::Router, quarry_storage::QuarryStore) {
    let (root, app, store) = document_test_app().await;
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries",
            serde_json::json!({"slug": "blocks"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    (root, app, store)
}

#[cfg(feature = "tmp-documents")]
async fn get_tmp_block_tree(app: &axum::Router, secret: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/blocks"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

fn assert_json_timestamp(value: &Value) {
    let timestamp = value.as_str().expect("timestamp should be a string");
    chrono::DateTime::parse_from_rfc3339(timestamp).expect("timestamp should parse as RFC 3339");
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn rest_api_supports_tmp_documents_ttl_versions_and_promotion() -> anyhow::Result<()> {
    let (_root, app, _store) = document_test_app().await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "draft one",
                "content_type": "text/markdown",
                "metadata": {"title": "Scratch"}
            }),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .context("created tmp document should expose a valid ETag")?
        .to_string();
    let created: Value = response_json(response).await;
    let secret = created["document"]["path"]
        .as_str()
        .context("created tmp document should expose a secret path")?
        .to_string();
    let document_id = created["document"]["id"]
        .as_str()
        .context("created tmp document should expose an id")?
        .to_string();
    assert_eq!(secret.len(), 32);
    assert!(
        secret
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
    assert_eq!(created["document"]["library_id"], Value::Null);
    assert_json_timestamp(&created["document"]["expires_at"]);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/tmp/documents")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], etag);
    assert_eq!(
        response.headers()["x-quarry-document-id"],
        document_id.as_str()
    );
    assert!(
        response.headers()["x-quarry-expires-at"]
            .to_str()
            .context("tmp document should expose a valid expiry header")?
            .starts_with("20")
    );
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await?,
        "draft one"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/tmp/documents/scratch/note.txt")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/tmp/documents/{secret}/share"),
            serde_json::json!({"role": "editor"}),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::IF_MATCH, etag)
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("draft two"))
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let updated: Value = response_json(response).await;
    let updated_version = updated["version"]["id"]
        .as_str()
        .context("updated tmp document should expose a version id")?
        .to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/versions/raw"))
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let versions: Value = response_json(response).await;
    assert_eq!(
        versions
            .as_array()
            .context("raw versions response should be an array")?
            .len(),
        2
    );
    let created_version_id = created["version"]["id"]
        .as_str()
        .context("created tmp document should expose a version id")?;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/tmp/documents/{secret}/versions/{created_version_id}"
                ))
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let first: Value = response_json(response).await;
    assert_eq!(first["content"], "draft one");

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/tmp/documents/{secret}/ttl"),
            serde_json::json!({"expires_at":"2099-01-01T00:00:00Z"}),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let ttl: Value = response_json(response).await;
    assert_eq!(ttl["expires_at"], "2099-01-01T00:00:00Z");

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/tmp/documents/{secret}/ttl"),
            serde_json::json!({"expires_at": null}),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries",
            serde_json::json!({"slug":"promoted"}),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/tmp/documents/{secret}/promote"),
            serde_json::json!({
                "library": "promoted",
                "path": "notes/promoted.txt",
                "if_match": updated_version
            }),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/promoted/documents/notes/promoted.txt")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-quarry-document-id"], document_id);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await?,
        "draft two\n"
    );

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/promoted/documents/notes/promoted.txt/versions/raw")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let promoted_versions: Value = response_json(response).await;
    assert_eq!(
        promoted_versions
            .as_array()
            .context("promoted versions response should be an array")?
            .len(),
        2
    );
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_markdown_put_replaces_materialized_blocks_and_preserves_ttl() -> anyhow::Result<()> {
    let (_root, app, store) = block_test_app().await;
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "# Original\n\nOld body.\n",
                "content_type": "text/markdown",
                "expires_at": "2099-01-01T00:00:00Z"
            }),
        ))
        .await
        .context("create tmp markdown document through REST")?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = response_json(response).await;
    let secret = created["document"]["path"]
        .as_str()
        .context("tmp create response should include document path")?
        .to_string();

    let before = get_tmp_block_tree(&app, &secret).await;
    assert_eq!(
        before["blocks"]
            .as_array()
            .context("tmp block tree should include blocks before PUT")?
            .len(),
        2
    );
    assert_eq!(before["blocks"][0]["text"], "Original");
    assert_eq!(before["blocks"][1]["text"], "Old body.");
    let clock = before["document_clock"]
        .as_str()
        .context("tmp block tree should include document clock")?
        .to_string();
    let expires_before = store
        .head_tmp_document(&secret)
        .await
        .context("read tmp document head before PUT")?
        .expires_at;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "text/markdown")
                .header(header::IF_MATCH, format!("\"{clock}\""))
                .body(Body::from("# Uploaded\n\nNew body.\n"))
                .context("build tmp markdown PUT request")?,
        )
        .await
        .context("replace tmp markdown document through REST")?;
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .context("tmp markdown PUT response ETag should be valid header text")?
        .to_string();
    let outcome = response_json(response).await;
    assert_eq!(
        etag,
        format!(
            "\"{}\"",
            outcome["version"]["id"]
                .as_str()
                .context("tmp markdown PUT response should include version id")?
        )
    );
    // The tmp PUT reply carries the merge verdict too — agents on tmp docs
    // (the usual case) must be able to tell "applied" from "parked in review".
    assert_eq!(outcome["changed"], true, "unexpected PUT reply: {outcome}");
    assert_eq!(outcome["conflicts"], 0);
    assert_eq!(
        store
            .head_tmp_document(&secret)
            .await
            .context("read tmp document head after PUT")?
            .expires_at,
        expires_before
    );

    let after = get_tmp_block_tree(&app, &secret).await;
    let blocks = after["blocks"]
        .as_array()
        .context("tmp block tree should include blocks after PUT")?;
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["text"], "Uploaded");
    assert_eq!(blocks[1]["text"], "New body.");

    let document = store
        .get_tmp_document(&secret)
        .await
        .context("read tmp document after markdown PUT")?;
    assert_eq!(
        String::from_utf8(document.content).context("tmp document content should be utf-8")?,
        "# Uploaded\n\nNew body.\n"
    );
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_put_requires_markdown_content_type() -> anyhow::Result<()> {
    let (_root, app, store) = block_test_app().await;
    let secret = "0123456789abcdef0123456789abcdef";

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::IF_NONE_MATCH, "*")
                .body(Body::from("# Draft\n"))
                .context("build tmp PUT request without content type")?,
        )
        .await
        .context("send tmp PUT request without content type")?;
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body["message"],
        "tmp writes require Content-Type: text/markdown"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::IF_NONE_MATCH, "*")
                .body(Body::from("# Draft\n"))
                .context("build tmp PUT request with form content type")?,
        )
        .await
        .context("send tmp PUT request with form content type")?;
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body["message"],
        "unsupported media type: tmp documents are Markdown-only; unsupported content type application/x-www-form-urlencoded"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::IF_NONE_MATCH, "*")
                .body(Body::from("# Draft\n"))
                .context("build tmp PUT request with json content type")?,
        )
        .await
        .context("send tmp PUT request with json content type")?;
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body["message"],
        "unsupported media type: tmp documents are Markdown-only; unsupported content type application/json"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "application/markdown; charset=utf-8")
                .header(header::IF_NONE_MATCH, "*")
                .header(
                    "x-quarry-metadata",
                    r#"{"content_type":"text/plain","title":"kept"}"#,
                )
                .body(Body::from("# Draft\n"))
                .context("build tmp PUT request with markdown content type")?,
        )
        .await
        .context("send tmp PUT request with markdown content type")?;
    assert_eq!(response.status(), StatusCode::OK);
    let document = store
        .get_tmp_document(secret)
        .await
        .context("read tmp document created by markdown PUT")?;
    assert_eq!(document.version.content_type, "application/markdown");
    assert_eq!(
        document.version.metadata,
        serde_json::json!({"content_type": "application/markdown", "title": "kept"})
    );
    let blocks = get_tmp_block_tree(&app, secret).await;
    assert_eq!(blocks["blocks"][0]["text"], "Draft");
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_documents_expose_version_history_diff_and_restore() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({"content": "# Plan\n\nAlpha.\n", "content_type": "text/markdown"}),
        ))
        .await
        .context("create tmp document")?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = response_json(response).await;
    let secret = created["document"]["path"]
        .as_str()
        .context("tmp create response should include the secret path")?
        .to_string();
    let first_version = created["version"]["id"]
        .as_str()
        .context("tmp create response should include the version id")?
        .to_string();

    let tree = get_tmp_block_tree(&app, &secret).await;
    let clock = tree["document_clock"]
        .as_str()
        .context("tmp block tree should include the document clock")?
        .to_string();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "text/markdown")
                .header(header::IF_MATCH, format!("\"{clock}\""))
                .body(Body::from("# Plan\n\nBravo.\n"))
                .context("build tmp rewrite request")?,
        )
        .await
        .context("rewrite tmp document")?;
    assert_eq!(response.status(), StatusCode::OK);

    // The aggregated history (DocumentHistoryEntry groups) is served for tmp.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/versions"))
                .body(Body::empty())
                .context("build tmp versions request")?,
        )
        .await
        .context("list tmp version history")?;
    assert_eq!(response.status(), StatusCode::OK);
    let history = response_json(response).await;
    let entries = history
        .as_array()
        .context("tmp version history should be an array")?;
    assert!(
        entries.len() >= 2,
        "expected history entries for create and rewrite: {history}"
    );
    assert!(entries[0]["latest_version_id"].is_string());

    // Diffing the first version against the current head shows the rewrite.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/tmp/documents/{secret}/versions/{first_version}/diff"
                ))
                .body(Body::empty())
                .context("build tmp diff request")?,
        )
        .await
        .context("diff tmp version")?;
    assert_eq!(response.status(), StatusCode::OK);
    let diff = response_json(response).await;
    assert_eq!(diff["base_version_id"], first_version.as_str());
    let unified = diff["unified_diff"]
        .as_str()
        .context("tmp diff should include a unified diff")?;
    assert!(unified.contains("-Alpha."), "unexpected diff: {unified}");
    assert!(unified.contains("+Bravo."), "unexpected diff: {unified}");

    // Restore is a whole-file write through the reconciler (tmp documents
    // are always BlockDocuments); the document returns to the old content.
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/tmp/documents/{secret}/versions/{first_version}/restore"),
            serde_json::json!({}),
        ))
        .await
        .context("restore tmp version")?;
    assert_eq!(response.status(), StatusCode::OK);
    let restored = response_json(response).await;
    assert!(restored["version"]["id"].is_string());

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .body(Body::empty())
                .context("build tmp read request")?,
        )
        .await
        .context("read restored tmp document")?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .context("read restored body")?;
    assert_eq!(String::from_utf8_lossy(&body), "# Plan\n\nAlpha.\n");
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_create_and_put_reject_oversized_markdown() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    let oversized = "a".repeat(quarry_storage::TMP_DOCUMENT_MARKDOWN_MAX_BYTES + 1);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": oversized,
                "content_type": "text/markdown",
            }),
        ))
        .await
        .context("create oversized tmp markdown document through REST")?;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "# Draft\n",
                "content_type": "text/markdown",
                "expires_at": "2099-01-01T00:00:00Z"
            }),
        ))
        .await
        .context("create tmp markdown document before oversized PUT")?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .context("tmp create response ETag should be valid header text")?
        .to_string();
    let created = response_json(response).await;
    let secret = created["document"]["path"]
        .as_str()
        .context("tmp create response should include document path")?
        .to_string();

    let oversized = "a".repeat(quarry_storage::TMP_DOCUMENT_MARKDOWN_MAX_BYTES + 1);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "text/markdown")
                .header(header::IF_MATCH, etag)
                .body(Body::from(oversized))
                .context("build oversized tmp markdown PUT request")?,
        )
        .await
        .context("send oversized tmp markdown PUT request")?;
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["code"], "PAYLOAD_TOO_LARGE");
    assert_eq!(body["retryable"], false);
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_markdown_put_rejects_non_markdown_content_type() -> anyhow::Result<()> {
    let (_root, app, store) = block_test_app().await;
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "# Draft\n\nBody.\n",
                "content_type": "text/markdown",
                "expires_at": "2099-01-01T00:00:00Z"
            }),
        ))
        .await
        .context("create tmp markdown document before non-markdown PUT")?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .context("tmp create response ETag should be valid header text")?
        .to_string();
    let created = response_json(response).await;
    let secret = created["document"]["path"]
        .as_str()
        .context("tmp create response should include document path")?
        .to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "text/plain")
                .header(header::IF_MATCH, etag.clone())
                .body(Body::from("raw body"))
                .context("build non-markdown tmp PUT request")?,
        )
        .await
        .context("send non-markdown tmp PUT request")?;
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body["message"],
        "unsupported media type: tmp documents are Markdown-only; unsupported content type text/plain"
    );

    let document = store
        .get_tmp_document(&secret)
        .await
        .context("read tmp document after rejected non-markdown PUT")?;
    assert_eq!(document.version.content_type, "text/markdown");
    assert_eq!(
        String::from_utf8(document.content).context("tmp document content should remain UTF-8")?,
        "# Draft\n\nBody.\n"
    );
    let blocks = get_tmp_block_tree(&app, &secret).await;
    assert_eq!(blocks["blocks"][0]["text"], "Draft");
    let latest_etag = format!(
        "\"{}\"",
        store
            .head_tmp_document(&secret)
            .await
            .context("read tmp document head after rejected non-markdown PUT")?
            .head_version_id
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/tmp/documents/{secret}"))
                .header(header::CONTENT_TYPE, "text/plain")
                .header(header::IF_MATCH, latest_etag)
                .header("x-quarry-allow-document-kind-change", "true")
                .body(Body::from("raw body"))
                .context("build non-markdown tmp PUT request with kind-change opt-in")?,
        )
        .await
        .context("send non-markdown tmp PUT request with kind-change opt-in")?;
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body["message"],
        "unsupported media type: tmp documents are Markdown-only; unsupported content type text/plain"
    );
    let document = store
        .get_tmp_document(&secret)
        .await
        .context("read tmp document after rejected kind-change opt-in PUT")?;
    assert_eq!(document.version.content_type, "text/markdown");
    assert_eq!(
        String::from_utf8(document.content).context("tmp document content should remain UTF-8")?,
        "# Draft\n\nBody.\n"
    );
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_create_rejects_non_markdown_content_type() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({
                "content": "draft one",
                "content_type": "text/plain",
                "expires_at": "2099-01-01T00:00:00Z"
            }),
        ))
        .await
        .context("create tmp document with non-markdown content type")?;
    let status = response.status();
    let body = response_json(response).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body["message"],
        "unsupported media type: tmp documents are Markdown-only; unsupported content type text/plain"
    );
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_agent_prompt_returns_connect_instructions() -> anyhow::Result<()> {
    let (_root, app, _store) = document_test_app().await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/tmp/documents",
            serde_json::json!({"content": "hello", "content_type": "text/markdown"}),
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: Value = response_json(response).await;
    let secret = created["document"]["path"]
        .as_str()
        .context("created tmp document should expose a secret path")?
        .to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/tmp/documents/{secret}/agent-prompt"))
                .header(header::HOST, "quarry.example.com")
                .header("x-forwarded-proto", "https")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/plain; charset=utf-8"
    );
    let body = to_bytes(response.into_body(), usize::MAX).await?;
    let prompt = String::from_utf8(body.to_vec())?;
    assert!(prompt.contains(&format!("https://quarry.example.com/tmp/{secret}")));
    assert!(prompt.contains("Scope: tmp document"));
    assert!(prompt.contains("Tmp document URLs are bearer capabilities"));
    assert!(prompt.contains(&format!(
        "POST https://quarry.example.com/v1/tmp/documents/{secret}/presence"
    )));
    assert!(prompt.contains("Connected in Quarry and ready."));
    Ok(())
}

#[cfg(feature = "tmp-documents")]
#[tokio::test]
async fn tmp_agent_prompt_unknown_secret_is_not_found() -> anyhow::Result<()> {
    let (_root, app, _store) = document_test_app().await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/tmp/documents/00000000000000000000000000000000/agent-prompt")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    Ok(())
}
