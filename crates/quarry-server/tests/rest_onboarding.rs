#![allow(clippy::unwrap_used, reason = "tests use unwrap for HTTP fixtures")]

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use tower::ServiceExt;

mod common;

use common::document_test_app;

async fn get_page(uri: &str, forwarded: bool) -> (StatusCode, String, String) {
    let (_root, app, _store) = document_test_app().await;
    let mut request = Request::builder().method(Method::GET).uri(uri);
    if forwarded {
        request = request
            .header("x-forwarded-proto", "https")
            .header(header::HOST, "quarry.lithos.computer");
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        content_type,
        String::from_utf8(body.to_vec()).unwrap(),
    )
}

#[tokio::test]
async fn home_page_serves_marketing_html_with_agent_prompt() {
    let (status, content_type, body) = get_page("/", false).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/html; charset=utf-8");
    assert!(body.contains("Give this to your coding agent"));
    assert!(body.contains("Homebrew instructions"));
    // Without forwarding headers the origin falls back to the local default.
    assert!(body.contains("http://127.0.0.1:7831/setup.md"));
    assert!(!body.contains("__QUARRY_ORIGIN__"));
    // The copy button script must be same-origin: inline scripts violate the CSP.
    assert!(body.contains("src=\"/home.js\""));
    assert!(!body.contains("<script>"));
}

#[tokio::test]
async fn onboarding_documents_render_the_forwarded_origin() {
    let (status, content_type, body) = get_page("/setup.md", true).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/markdown; charset=utf-8");
    assert!(body.contains("brew tap fabro-sh/quarry https://github.com/fabro-sh/quarry.git"));
    assert!(body.contains("brew install fabro-sh/quarry/quarry"));
    assert!(body.contains("brew trust --tap fabro-sh/quarry"));
    assert!(body.contains("https://quarry.lithos.computer/prompt.md"));
    assert!(body.contains("https://quarry.lithos.computer/example.md"));
    assert!(body.contains("## Install or Refresh Your Persistent Instructions"));
    assert!(body.contains("replace the entire marked block"));
    assert!(body.contains("legacy unmarked `## Quarry` section"));
    assert!(body.contains("verify that each marker appears exactly once"));
    assert!(body.contains("A concrete imperative comment is an edit"));
    assert!(body.contains("Do not merely promise the requested edit"));
    assert!(!body.contains("__QUARRY_ORIGIN__"));
}

#[tokio::test]
async fn prompt_document_teaches_the_review_workflow() {
    let (status, content_type, body) = get_page("/prompt.md", false).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/markdown; charset=utf-8");
    assert_eq!(
        body.matches("<!-- BEGIN QUARRY AGENT INSTRUCTIONS -->")
            .count(),
        1
    );
    assert_eq!(
        body.matches("<!-- END QUARRY AGENT INSTRUCTIONS -->")
            .count(),
        1
    );
    assert!(body.contains(
        "Use Quarry when a Markdown document needs review, comments, collaboration, or user markup."
    ));
    assert!(body.contains("quarry open"));
    assert!(body.contains("creates the shared document"));
    assert!(body.contains("Follow them exactly, and do not edit until the user asks."));
    assert!(body.contains("suggestion.add"));
    assert!(body.contains("suggestion.add_block_delete"));
    assert!(body.contains("A concrete imperative comment"));
    assert!(body.contains("authorizes that requested edit"));
    assert!(body.contains("Do not answer an implementation request only with a promise"));
    assert!(body.contains("when the user asks for a proposal"));
    assert!(body.contains("bearer capabilities"));
    assert!(body.contains(
        "Never put sensitive content on an untrusted server or log/repost a document URL."
    ));
    assert!(body.contains("http://127.0.0.1:7831/quarry.SKILL.md"));
    assert!(!body.contains("send `X-Agent-Id` on every request"));
    assert!(!body.contains("__QUARRY_ORIGIN__"));
}

#[tokio::test]
async fn example_document_and_copy_script_are_served_verbatim() {
    let (status, content_type, body) = get_page("/example.md", false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/markdown; charset=utf-8");
    assert!(body.contains("# Welcome to Quarry"));
    assert!(body.contains("A sentence that needs work"));

    let (status, content_type, body) = get_page("/home.js", false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "text/javascript; charset=utf-8");
    assert!(body.contains("navigator.clipboard"));
}
