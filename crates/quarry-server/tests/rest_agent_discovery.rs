#![cfg(feature = "lib-documents")]
#![allow(clippy::unwrap_used, reason = "tests use unwrap for HTTP fixtures")]

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

mod common;

use common::{document_test_app, response_json};

#[tokio::test]
async fn agent_discovery_endpoints_expose_skill_docs_and_metadata() {
    let (_root, app, _store) = document_test_app().await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/quarry.SKILL.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/agent-docs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let docs = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
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
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
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
    assert!(
        body["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == "presence")
    );
    assert!(
        body["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == "transactions")
    );
    assert!(
        body["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability == "review")
    );
    if cfg!(feature = "tmp-documents") {
        assert!(
            body["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|capability| capability == "tmp_documents")
        );
        assert!(
            !body["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|capability| capability == &removed_tmp_signal)
        );
    }
    assert!(
        body["auth_note"]
            .as_str()
            .unwrap()
            .contains("trusted-localhost")
    );
    assert_eq!(body["auth"]["mode"], "trusted_localhost");
    assert!(body["presence_statuses"].as_array().unwrap().len() >= 6);
    assert!(
        body["transaction_operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation == "replace_block_content")
    );
    assert!(
        body["transaction_operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation == "set_block_type")
    );
    assert!(
        body["transaction_operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation == "comment.add")
    );
    assert!(
        body["transaction_operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation == "comment.edit")
    );
    assert!(
        body["transaction_operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation == "suggestion.accept")
    );
    assert!(
        !body["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|limitation| limitation
                .as_str()
                .is_some_and(|limitation| limitation.contains("comment.reply")))
    );
}
