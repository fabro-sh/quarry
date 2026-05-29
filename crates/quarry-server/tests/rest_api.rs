use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use quarry_core::DocumentSource;
use quarry_server::router;
use quarry_storage::{QuarryStore, StoreConfig};
use serde_json::Value;
use tower::ServiceExt;

#[tokio::test]
async fn rest_api_supports_documents_transactions_etags_and_openapi() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries",
            serde_json::json!({"slug":"alpha"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("one"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_string();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .header(header::IF_MATCH, "\"wrong\"")
                .body(Body::from("bad"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], etag);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "one"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri("/v1/libraries/alpha/documents/notes/one.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], etag);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        ""
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/alpha/documents/notes/created.md")
                .header(header::IF_NONE_MATCH, "*")
                .body(Body::from("created"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/alpha/documents/notes/created.md")
                .header(header::IF_NONE_MATCH, "*")
                .body(Body::from("duplicate"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/alpha/transactions",
            serde_json::json!({"message":"batch"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response_json(response).await;
    let tx = body["id"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!(
                    "/v1/libraries/alpha/transactions/{tx}/documents/notes/two.md"
                ))
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("two"))
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
                .uri("/v1/libraries/alpha/documents/notes/two.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/alpha/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/alpha/documents/notes/two.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "two"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let openapi: Value = response_json(response).await;
    assert!(openapi["paths"]["/v1/libraries"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}"]["head"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/metadata"].is_object());
    assert!(openapi["paths"]
        ["/v1/libraries/{library}/transactions/{tx}/documents/{path}/metadata"]
        .is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/git/peers"]["post"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/git/peers"]["get"].is_object());
}

#[tokio::test]
async fn rest_api_supports_browser_search_links_versions_and_events() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("browser").await.unwrap();
    let first_intro = store
        .put_document(
            &library.slug,
            "intro.md",
            b"# Intro\n\nLinks to [[Daily|today]], [[Missing]], [Guide](guide.md), and #planning.\n".to_vec(),
            serde_json::json!({"title":"Intro","content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "daily.md",
            b"# Daily\n\nBacklinked target with [[Chain]].\n".to_vec(),
            serde_json::json!({"title":"Daily","content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "guide.md",
            b"# Guide\n".to_vec(),
            serde_json::json!({
                "aliases": ["Manual Alias"],
                "title":"Guide",
                "content_type":"text/markdown"
            }),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let latest_intro = store
        .put_document(
            &library.slug,
            "intro.md",
            b"# Intro\n\nLinks to [[Daily|today]], [[Missing]], [Guide](guide.md), and #planning.\nupdated browser body with unique-search term.\n".to_vec(),
            serde_json::json!({"title":"Intro","content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::IfMatch(first_intro.version.id.clone()),
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "chain.md",
            b"# Chain\n".to_vec(),
            serde_json::json!({"title":"Chain","content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "projects/roadmap.md",
            b"# Roadmap\n".to_vec(),
            serde_json::json!({"title":"Roadmap","content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    store
        .put_document(
            &library.slug,
            "projects/brief.md",
            b"# Brief\n\nSee [[Roadmap]] and #planning.\n".to_vec(),
            serde_json::json!({"title":"Brief","content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/search?q=unique-search&limit=5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["results"][0]["path"], "intro.md");
    assert_eq!(
        body["results"][0]["head_version_id"],
        latest_intro.version.id
    );
    assert!(body["results"][0]["matched_fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field == "body"));
    assert!(body["results"][0]["snippet"]
        .as_str()
        .unwrap()
        .contains("unique-search"));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/search?q=%23planning&limit=5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(body["results"].as_array().unwrap().iter().any(|result| {
        result["path"] == "intro.md"
            && result["matched_fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field == "tag")
    }));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/search?q=manual%20alias&limit=5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(body["results"].as_array().unwrap().iter().any(|result| {
        result["path"] == "guide.md"
            && result["matched_fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field == "alias")
    }));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/search/suggest?q=dai&limit=5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body[0]["path"], "daily.md");
    assert_eq!(body[0]["match_type"], "title");

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/browser/reindex",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["indexed_documents"], 6);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/documents/intro.md/outgoing-links")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let links = body["links"].as_array().unwrap();
    assert!(links.iter().any(|link| link["target_kind"] == "wiki_link"
        && link["target_path"] == "daily.md"
        && link["alias"] == "today"));
    assert!(links
        .iter()
        .any(|link| link["target_kind"] == "markdown_link" && link["target_path"] == "guide.md"));
    assert!(links
        .iter()
        .any(|link| link["target_kind"] == "tag" && link["target_text"] == "planning"));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/documents/daily.md/backlinks")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(body["links"]
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["src_path"] == "intro.md"));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?root=intro.md&depth=1&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["path"] == "intro.md"));
    assert!(body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|edge| { edge["source_path"] == "intro.md" && edge["target_path"] == "daily.md" }));
    assert!(!body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["path"] == "chain.md"));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?root=intro.md&depth=2&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["path"] == "chain.md"));
    assert!(body["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|edge| { edge["source_path"] == "daily.md" && edge["target_path"] == "chain.md" }));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?root=intro.md&link_kind=tag&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let edges = body["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["target_kind"], "tag");
    assert_eq!(edges[0]["target_text"], "planning");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?folder=projects&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let nodes = body["nodes"].as_array().unwrap();
    assert!(!nodes.is_empty());
    assert!(nodes
        .iter()
        .all(|node| node["path"].as_str().unwrap().starts_with("projects/")));
    assert!(body["edges"].as_array().unwrap().iter().any(|edge| {
        edge["source_path"] == "projects/brief.md" && edge["target_path"] == "projects/roadmap.md"
    }));
    assert!(body["edges"].as_array().unwrap().iter().all(|edge| {
        edge["source_path"]
            .as_str()
            .unwrap()
            .starts_with("projects/")
            && edge["target_path"]
                .as_str()
                .is_none_or(|path| path.starts_with("projects/"))
    }));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?tag=planning&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let edges = body["edges"].as_array().unwrap();
    assert!(!edges.is_empty());
    assert!(edges
        .iter()
        .all(|edge| edge["target_kind"] == "tag" && edge["target_text"] == "planning"));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/graph?root=intro.md&link_kind=wiki_link&resolved=false&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let edges = body["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["target_kind"], "wiki_link");
    assert_eq!(edges[0]["target_text"], "Missing");
    assert_eq!(edges[0]["resolved"], false);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/documents/intro.md/versions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body[0]["id"], latest_intro.version.id);
    assert_eq!(body[1]["id"], first_intro.version.id);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/libraries/browser/documents/intro.md/versions/{}",
                    first_intro.version.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert!(body["content"]
        .as_str()
        .unwrap()
        .contains("[[Daily|today]]"));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v1/libraries/browser/documents/intro.md/versions/{}/diff?against={}",
                    first_intro.version.id, latest_intro.version.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let diff = body["unified_diff"].as_str().unwrap();
    assert!(diff.contains("+updated browser body"));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!(
                "/v1/libraries/browser/documents/intro.md/versions/{}/restore",
                first_intro.version.id
            ),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_ne!(response.headers()[header::ETAG], first_intro.version.id);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/browser/documents/intro.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "# Intro\n\nLinks to [[Daily|today]], [[Missing]], [Guide](guide.md), and #planning.\n"
    );

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/events?library=browser")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let openapi: Value = response_json(response).await;
    assert!(openapi["paths"]["/v1/libraries/{library}/search"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/backlinks"].is_object());
    assert!(openapi["paths"]["/v1/libraries/{library}/documents/{path}/versions"].is_object());
    assert!(openapi["paths"]["/v1/events"].is_object());
}

#[tokio::test]
async fn version_history_includes_transaction_metadata() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries",
            serde_json::json!({"slug":"versionmeta"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/versionmeta/transactions",
            serde_json::json!({
                "actor": "Avery",
                "message": "Imported from Git",
                "provenance": {"remote": "origin/main"}
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response_json(response).await;
    let tx = body["id"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!(
                    "/v1/libraries/versionmeta/transactions/{tx}/documents/meta.md"
                ))
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("# Meta\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/versionmeta/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/versionmeta/documents/meta.md/versions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body[0]["transaction_source"], "rest");
    assert_eq!(body[0]["transaction_actor"], "Avery");
    assert_eq!(body[0]["transaction_message"], "Imported from Git");
    assert_eq!(body[0]["transaction_provenance"]["remote"], "origin/main");
}

#[tokio::test]
async fn rest_api_supports_move_metadata_and_conflict_lookup_endpoints() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("actions").await.unwrap();
    store.create_library("other").await.unwrap();
    let written = store
        .put_document(
            &library.slug,
            "a.md",
            b"hello".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let sibling = store
        .put_document(
            &library.slug,
            "a.conflict.md",
            b"git version".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Git,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let conflict = store
        .record_conflict(
            &library.slug,
            "a.md",
            Some(written.version.id.clone()),
            Some(sibling.version.id.clone()),
        )
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/actions/documents/a.md/move",
            serde_json::json!({"to_path":"b.md"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/actions/documents/b.md/metadata",
            serde_json::json!({"reviewed":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            "/v1/libraries/actions/documents/b.md",
            serde_json::json!({"wrong":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/actions/documents/b.md")
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
                .uri(format!("/v1/libraries/actions/conflicts/{}", conflict.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["id"], conflict.id);
    assert_eq!(body["conflict_path"], "a.conflict.md");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v1/libraries/other/conflicts/{}", conflict.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/other/conflicts/{}/resolve", conflict.id),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/actions/conflicts/{}/resolve", conflict.id),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body["status"], "resolved");
    assert!(body["resolved_at"].as_str().is_some());
}

#[tokio::test]
async fn rest_api_marks_ambiguous_links() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("ambiguous").await.unwrap();

    for path in ["alpha/target.md", "omega/target.md"] {
        store
            .put_document(
                &library.slug,
                path,
                b"# Target\n".to_vec(),
                serde_json::json!({"content_type": "text/markdown"}),
                "text/markdown",
                DocumentSource::Rest,
                quarry_core::WritePrecondition::None,
            )
            .await
            .unwrap();
    }
    store
        .put_document(
            &library.slug,
            "source.md",
            b"See [[target]].\n".to_vec(),
            serde_json::json!({"content_type": "text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/ambiguous/documents/source.md/outgoing-links")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    let link = body["links"]
        .as_array()
        .unwrap()
        .iter()
        .find(|link| link["target_kind"] == "wiki_link" && link["target_text"] == "target")
        .unwrap();
    assert_eq!(link["target_path"], Value::Null);
    assert_eq!(link["resolved"], false);
    assert_eq!(link["resolution_status"], "ambiguous");
}

#[tokio::test]
async fn rest_api_supports_transaction_metadata_patch_and_move() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("txactions").await.unwrap();
    store
        .put_document(
            &library.slug,
            "drafts/a.md",
            b"draft".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/txactions/transactions",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response_json(response).await;
    let tx = body["id"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/libraries/txactions/transactions/{tx}/documents/drafts/a.md"),
            serde_json::json!({"wrong":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/libraries/txactions/transactions/{tx}/documents/drafts/a.md/metadata"),
            serde_json::json!({"reviewed":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txactions/transactions/{tx}/documents/drafts/a.md/move"),
            serde_json::json!({"to_path":"published/a.md"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txactions/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/txactions/documents/published/a.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/txactions/documents?prefix=published/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response_json(response).await;
    assert_eq!(body[0]["metadata"]["reviewed"], true);
}

#[tokio::test]
async fn rest_api_rejects_stale_transaction_commit_with_precondition_failed() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries",
            serde_json::json!({"slug":"txpreconditions"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/txpreconditions/documents/docs/a.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("base"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/txpreconditions/transactions",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response_json(response).await;
    let tx = body["id"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!(
                    "/v1/libraries/txpreconditions/transactions/{tx}/documents/docs/a.md"
                ))
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("staged"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/v1/libraries/txpreconditions/documents/docs/a.md")
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("newer"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txpreconditions/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v1/libraries/txpreconditions/documents/docs/a.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
        "newer"
    );

    let response = app
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txpreconditions/transactions/{tx}/rollback"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn rest_api_scopes_transaction_routes_to_the_url_library() {
    let root = tempfile::tempdir().unwrap();
    let store = QuarryStore::open(StoreConfig {
        db_path: root.path().join("quarry.db"),
        cas_path: root.path().join("cas"),
        lock_path: None,
    })
    .await
    .unwrap();
    let library = store.create_library("txscope").await.unwrap();
    store.create_library("other").await.unwrap();
    store
        .put_document(
            &library.slug,
            "drafts/a.md",
            b"draft".to_vec(),
            serde_json::json!({"content_type":"text/markdown"}),
            "text/markdown",
            DocumentSource::Rest,
            quarry_core::WritePrecondition::None,
        )
        .await
        .unwrap();
    let app = router(store);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/v1/libraries/txscope/transactions",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value = response_json(response).await;
    let tx = body["id"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!(
                    "/v1/libraries/other/transactions/{tx}/documents/drafts/leak.md"
                ))
                .header(header::CONTENT_TYPE, "text/markdown")
                .body(Body::from("leak"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::PATCH,
            &format!("/v1/libraries/other/transactions/{tx}/documents/drafts/a.md/metadata"),
            serde_json::json!({"wrong_library":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/other/transactions/{tx}/documents/drafts/a.md/move"),
            serde_json::json!({"to_path":"published/a.md"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::DELETE,
            &format!("/v1/libraries/other/transactions/{tx}/documents/drafts/a.md"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/other/transactions/{tx}/commit"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/other/transactions/{tx}/rollback"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .oneshot(json_request(
            Method::POST,
            &format!("/v1/libraries/txscope/transactions/{tx}/rollback"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

fn json_request(method: Method, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}
