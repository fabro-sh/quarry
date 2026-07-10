#![cfg(feature = "lib-documents")]
#![allow(clippy::unwrap_used, reason = "tests use unwrap for HTTP fixtures")]

use anyhow::Context as _;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use quarry_storage::QuarryStore;
use serde_json::Value;
use tower::ServiceExt;

mod common;

use common::{document_test_app, json_request, response_json};

async fn block_test_app() -> (tempfile::TempDir, axum::Router, QuarryStore) {
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

async fn put_block_markdown(app: &axum::Router, path: &str, body: &str) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/v1/libraries/blocks/documents/{path}"))
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

async fn get_block_tree(app: &axum::Router, path: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/libraries/blocks/documents/{path}/blocks"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn get_document_markdown(app: &axum::Router, path: &str) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/libraries/blocks/documents/{path}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

#[tokio::test]
async fn agent_discovery_endpoints_expose_skill_docs_and_metadata() -> anyhow::Result<()> {
    let (_root, app, _store) = document_test_app().await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/quarry.SKILL.md")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let skill = String::from_utf8(to_bytes(response.into_body(), usize::MAX).await?.to_vec())
        .context("Quarry skill should be valid UTF-8")?;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/agent-docs")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let docs = String::from_utf8(to_bytes(response.into_body(), usize::MAX).await?.to_vec())
        .context("agent docs should be valid UTF-8")?;
    // The docs teach the session-scoped contract: stable block ids, the
    // transaction envelope with typed retryable errors, and the rows-backed
    // review projection (incl. conflict items).
    assert!(docs.contains("$DOC/blocks"));
    assert!(docs.contains("$DOC/transactions"));
    assert!(docs.contains("client_tx_id"));
    assert!(docs.contains("base_clock"));
    assert!(docs.contains("changed_block_ids"));
    assert!(docs.contains("committed_rebased"));
    assert!(docs.contains("STALE_BASE"));
    assert!(docs.contains("set_block_type"));
    // The documented block-type vocabulary is the codec's REAL one (the
    // insert example commits verbatim — see
    // agent_docs_insert_block_example_commits_as_documented). Fake friendly
    // names would 422 at commit time.
    assert!(docs.contains("`p`, `h1`\u{2013}`h6`"));
    assert!(docs.contains("`raw_markdown`"));
    assert!(docs.contains("listStyleType"));
    assert!(docs.contains("\"block_type\": \"p\""));
    assert!(!docs.contains("`paragraph`"));
    assert!(!docs.contains("list_item"));
    assert!(!docs.contains("image_embed"));
    assert!(docs.contains("comment.reply"));
    assert!(docs.contains("comment.edit"));
    assert!(docs.contains("suggestion.accept"));
    assert!(docs.contains("conflict"));
    assert!(docs.contains("GET $DOC/review"));
    assert!(docs.contains("re-read both `/blocks` and `/review`"));
    assert!(docs.contains("document API call carrying `X-Agent-Id` refreshes the TTL"));
    assert!(docs.contains("X-Quarry-Transaction-Actor"));
    assert!(docs.contains("conflict.add` is server-internal"));
    assert!(docs.contains("Do not send it"));
    assert!(docs.contains("local or hosted origins"));
    assert!(skill.contains("re-read both `/blocks` and `/review`"));
    assert!(skill.contains("X-Quarry-Transaction-Actor"));
    assert!(skill.contains("local or hosted"));
    assert!(docs.contains("/v1/tmp/documents/$SECRET"));
    let removed_tmp_signal = ["han", "doff"].join("");
    assert!(!docs.to_lowercase().contains(&removed_tmp_signal));
    // The legacy facade vocabulary is gone.
    assert!(!docs.contains("/edit"));
    assert!(!docs.contains("$DOC/ops"));
    assert!(!docs.contains("POST $DOC/review"));
    assert!(!docs.contains("ordinal"));
    assert!(!docs.contains("contentHash"));
    assert!(!docs.contains("baseToken\": \"version_123"));
    assert!(!docs.contains("Idempotency-Key"));
    assert!(!docs.contains("injection"));

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/.well-known/agent.json")
                .header(header::HOST, "127.0.0.1:7831")
                .body(Body::empty())
                .context("build request")?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    for operation in body["transaction_operations"]
        .as_array()
        .context("discovery should expose transaction operations")?
    {
        let operation = operation
            .as_str()
            .context("transaction operation should be a string")?;
        assert!(
            docs.contains(operation),
            "agent docs should describe public operation {operation}"
        );
        assert!(
            skill.contains(operation),
            "Quarry skill should describe public operation {operation}"
        );
    }
    assert!(
        !body["transaction_operations"]
            .as_array()
            .context("discovery should expose transaction operations")?
            .iter()
            .any(|operation| operation == "conflict.add")
    );
    assert_eq!(body["api_base"], "http://127.0.0.1:7831/v1");
    assert_eq!(body["docs_url"], "http://127.0.0.1:7831/agent-docs");
    assert_eq!(body["skill_url"], "http://127.0.0.1:7831/quarry.SKILL.md");
    assert_eq!(body["openapi_url"], "http://127.0.0.1:7831/v1/openapi.json");
    if cfg!(feature = "lib-documents") {
        assert_eq!(
            body["endpoints"]["transactions"]["method"],
            serde_json::json!("POST")
        );
        assert_eq!(
            body["route_hints"]["transactions"],
            serde_json::json!(
                "http://127.0.0.1:7831/v1/libraries/{library}/documents/{path}/transactions"
            )
        );
        assert_eq!(
            body["route_hints"]["blocks"],
            serde_json::json!(
                "http://127.0.0.1:7831/v1/libraries/{library}/documents/{path}/blocks"
            )
        );
        assert_eq!(
            body["endpoints"]["snapshot"]["url"],
            "http://127.0.0.1:7831/v1/libraries/{library}/documents/{path}/snapshot"
        );
        assert_eq!(
            body["endpoints"]["review"]["url"],
            "http://127.0.0.1:7831/v1/libraries/{library}/documents/{path}/review"
        );
        assert_eq!(
            body["route_hints"]["review"],
            "http://127.0.0.1:7831/v1/libraries/{library}/documents/{path}/review"
        );
    } else {
        assert!(body["endpoints"]["transactions"].is_null());
        assert!(body["route_hints"]["transactions"].is_null());
        assert!(body["endpoints"]["snapshot"].is_null());
    }
    if cfg!(feature = "tmp-documents") {
        assert_eq!(
            body["endpoints"]["tmp_transactions"]["method"],
            serde_json::json!("POST")
        );
        assert_eq!(
            body["route_hints"]["tmp_blocks"],
            "http://127.0.0.1:7831/v1/tmp/documents/{secret}/blocks"
        );
        let removed_tmp_signal_key = ["tmp_han", "doff"].join("");
        assert!(body["endpoints"][&removed_tmp_signal_key].is_null());
        assert!(body["route_hints"][removed_tmp_signal_key].is_null());
    } else {
        assert!(body["endpoints"]["tmp_transactions"].is_null());
        assert!(body["route_hints"]["tmp_blocks"].is_null());
    }
    // The legacy facades are gone from discovery entirely.
    assert!(body["endpoints"]["edit"].is_null());
    assert!(body["endpoints"]["ops"].is_null());
    assert!(body["endpoints"]["review_process"].is_null());
    assert!(body["route_hints"]["edit"].is_null());
    assert!(body["route_hints"]["ops"].is_null());
    let capabilities = body["capabilities"]
        .as_array()
        .context("discovery should expose capabilities")?;
    assert!(
        capabilities
            .iter()
            .any(|capability| capability == "presence")
    );
    assert!(
        capabilities
            .iter()
            .any(|capability| capability == "transactions")
    );
    assert!(capabilities.iter().any(|capability| capability == "review"));
    if cfg!(feature = "tmp-documents") {
        assert!(
            capabilities
                .iter()
                .any(|capability| capability == "tmp_documents")
        );
        assert!(
            !capabilities
                .iter()
                .any(|capability| capability == &removed_tmp_signal)
        );
    }
    assert!(
        body["auth_note"]
            .as_str()
            .context("discovery should expose an auth note")?
            .contains("trusted-localhost")
    );
    assert_eq!(body["auth"]["mode"], "trusted_localhost");
    assert!(
        body["presence_statuses"]
            .as_array()
            .context("discovery should expose presence statuses")?
            .len()
            >= 6
    );
    let transaction_operations = body["transaction_operations"]
        .as_array()
        .context("discovery should expose transaction operations")?;
    assert!(
        transaction_operations
            .iter()
            .any(|operation| operation == "replace_block_content")
    );
    assert!(
        transaction_operations
            .iter()
            .any(|operation| operation == "set_block_type")
    );
    assert!(
        transaction_operations
            .iter()
            .any(|operation| operation == "comment.add")
    );
    assert!(
        transaction_operations
            .iter()
            .any(|operation| operation == "comment.edit")
    );
    assert!(
        transaction_operations
            .iter()
            .any(|operation| operation == "suggestion.accept")
    );
    let limitations = body["limitations"]
        .as_array()
        .context("discovery should expose limitations")?;
    assert!(!limitations.iter().any(|limitation| {
        limitation
            .as_str()
            .is_some_and(|limitation| limitation.contains("comment.reply"))
    }));
    Ok(())
}

#[tokio::test]
async fn agent_docs_insert_block_example_commits_as_documented() -> anyhow::Result<()> {
    let (_root, app, _store) = block_test_app().await;
    put_block_markdown(&app, "doc.md", "# Title\n\nAlpha.\n").await;
    let tree = get_block_tree(&app, "doc.md").await;
    let clock = tree["document_clock"]
        .as_str()
        .context("block tree should include document clock")?
        .to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/agent-docs")
                .body(Body::empty())
                .context("build agent docs request")?,
        )
        .await
        .context("send agent docs request")?;
    let docs = String::from_utf8(to_bytes(response.into_body(), usize::MAX).await?.to_vec())
        .context("agent docs should be valid UTF-8")?;

    // The example is the curl payload after "Insert a paragraph…": the JSON
    // between `-d '` and the closing `}'` (no single quotes inside JSON).
    let anchor = docs
        .find("Insert a paragraph after the current second block")
        .context("docs should keep the insert example")?;
    let body_start = docs[anchor..]
        .find("-d '")
        .context("insert example should include curl -d payload")?
        + anchor
        + 4;
    let body_end = docs[body_start..]
        .find("}'")
        .context("insert example should include payload terminator")?
        + body_start
        + 1;
    let documented = docs[body_start..body_end].replace("version_124", &clock);
    let payload: Value =
        serde_json::from_str(&documented).context("documented example must be valid JSON")?;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/blocks/documents/doc.md/transactions",
            payload,
        ))
        .await
        .context("commit documented insert block example")?;
    let status = response.status();
    let ack = response_json(response).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "documented example must commit: {ack}"
    );
    assert_eq!(ack["status"], "committed");

    let after = get_block_tree(&app, "doc.md").await;
    assert_eq!(after["blocks"][2]["block_type"], "p");
    assert_eq!(after["blocks"][2]["text"], "A new paragraph.");
    assert_eq!(
        get_document_markdown(&app, "doc.md").await,
        "# Title\n\nAlpha.\n\nA new paragraph.\n"
    );
    Ok(())
}
