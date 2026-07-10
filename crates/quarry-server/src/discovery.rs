use crate::{ApiError, json_response};
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use serde::Serialize;
use std::collections::BTreeMap;

const QUARRY_SKILL_MD: &str = include_str!("../resources/quarry.SKILL.md");
const AGENT_DOCS_MD: &str = include_str!("../resources/agent-docs.md");

#[derive(Debug, Serialize)]
struct AgentDiscovery {
    name: &'static str,
    api_base: String,
    docs_url: String,
    skill_url: String,
    openapi_url: String,
    capabilities: Vec<&'static str>,
    auth_note: &'static str,
    auth: AgentDiscoveryAuth,
    presence_statuses: Vec<&'static str>,
    /// `POST /transactions` op vocabulary (see the agent docs for shapes).
    transaction_operations: Vec<&'static str>,
    limitations: Vec<&'static str>,
    route_hints: AgentDiscoveryRouteHints,
    endpoints: BTreeMap<&'static str, AgentDiscoveryEndpoint>,
}

#[derive(Debug, Serialize)]
struct AgentDiscoveryAuth {
    mode: &'static str,
    token_role: &'static str,
    required_headers: Vec<&'static str>,
    note: &'static str,
}

#[derive(Debug, Serialize)]
struct AgentDiscoveryRouteHints {
    #[serde(skip_serializing_if = "Option::is_none")]
    presence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocks: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transactions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    review: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    events_stream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    events_pending: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmp_document: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmp_presence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmp_blocks: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmp_transactions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmp_review: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmp_events_stream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tmp_share: Option<String>,
}

#[derive(Debug, Serialize)]
struct AgentDiscoveryEndpoint {
    method: &'static str,
    path: &'static str,
    url: String,
}

pub(crate) async fn quarry_skill() -> Response {
    static_text_response("text/markdown; charset=utf-8", QUARRY_SKILL_MD)
}

pub(crate) async fn agent_docs() -> Response {
    static_text_response("text/markdown; charset=utf-8", AGENT_DOCS_MD)
}

pub(crate) async fn agent_discovery(headers: HeaderMap) -> Result<Response, ApiError> {
    let origin = request_origin(&headers);
    let api_base = format!("{origin}/v1");
    let document_path = "/v1/libraries/{library}/documents/{path}";
    let tmp_document_path = "/v1/tmp/documents/{secret}";
    let lib_documents_enabled = cfg!(feature = "lib-documents");
    let tmp_documents_enabled = cfg!(feature = "tmp-documents");
    let mut endpoints = BTreeMap::new();
    if lib_documents_enabled {
        endpoints.insert(
            "presence",
            discovery_endpoint(
                "POST",
                "/v1/libraries/{library}/documents/{path}/presence",
                &api_base,
            ),
        );
        endpoints.insert(
            "presence_list",
            discovery_endpoint(
                "GET",
                "/v1/libraries/{library}/documents/{path}/presence",
                &api_base,
            ),
        );
        endpoints.insert(
            "snapshot",
            discovery_endpoint(
                "GET",
                "/v1/libraries/{library}/documents/{path}/snapshot",
                &api_base,
            ),
        );
        endpoints.insert(
            "blocks",
            discovery_endpoint(
                "GET",
                "/v1/libraries/{library}/documents/{path}/blocks",
                &api_base,
            ),
        );
        endpoints.insert(
            "transactions",
            discovery_endpoint(
                "POST",
                "/v1/libraries/{library}/documents/{path}/transactions",
                &api_base,
            ),
        );
        endpoints.insert(
            "review",
            discovery_endpoint(
                "GET",
                "/v1/libraries/{library}/documents/{path}/review",
                &api_base,
            ),
        );
        endpoints.insert(
            "document",
            discovery_endpoint("GET", document_path, &api_base),
        );
        endpoints.insert(
            "events_stream",
            discovery_endpoint(
                "GET",
                "/v1/libraries/{library}/documents/{path}/events/stream",
                &api_base,
            ),
        );
        endpoints.insert(
            "events_pending",
            discovery_endpoint(
                "GET",
                "/v1/libraries/{library}/events/pending?after={last-seen-id}",
                &api_base,
            ),
        );
        endpoints.insert(
            "events_ack",
            discovery_endpoint("POST", "/v1/libraries/{library}/events/ack", &api_base),
        );
    }
    if tmp_documents_enabled {
        endpoints.insert(
            "tmp_document",
            discovery_endpoint("GET", tmp_document_path, &api_base),
        );
        endpoints.insert(
            "tmp_presence",
            discovery_endpoint("POST", "/v1/tmp/documents/{secret}/presence", &api_base),
        );
        endpoints.insert(
            "tmp_presence_list",
            discovery_endpoint("GET", "/v1/tmp/documents/{secret}/presence", &api_base),
        );
        endpoints.insert(
            "tmp_blocks",
            discovery_endpoint("GET", "/v1/tmp/documents/{secret}/blocks", &api_base),
        );
        endpoints.insert(
            "tmp_transactions",
            discovery_endpoint("POST", "/v1/tmp/documents/{secret}/transactions", &api_base),
        );
        endpoints.insert(
            "tmp_review",
            discovery_endpoint("GET", "/v1/tmp/documents/{secret}/review", &api_base),
        );
        endpoints.insert(
            "tmp_events_stream",
            discovery_endpoint("GET", "/v1/tmp/documents/{secret}/events/stream", &api_base),
        );
    }
    endpoints.insert(
        "openapi",
        discovery_endpoint("GET", "/v1/openapi.json", &api_base),
    );
    endpoints.insert(
        "docs",
        AgentDiscoveryEndpoint {
            method: "GET",
            path: "/agent-docs",
            url: format!("{origin}/agent-docs"),
        },
    );
    endpoints.insert(
        "skill",
        AgentDiscoveryEndpoint {
            method: "GET",
            path: "/quarry.SKILL.md",
            url: format!("{origin}/quarry.SKILL.md"),
        },
    );
    let mut capabilities = vec![
        "presence",
        "blocks",
        "transactions",
        "review",
        "events",
        "comments",
        "suggestions",
    ];
    if lib_documents_enabled {
        capabilities.extend(["library_documents", "snapshot", "events_pending"]);
    }
    if tmp_documents_enabled {
        capabilities.extend(["tmp_documents", "capability_urls"]);
    }
    let library_route = |suffix: &str| {
        if lib_documents_enabled {
            Some(format!(
                "{api_base}/libraries/{{library}}/documents/{{path}}{suffix}"
            ))
        } else {
            None
        }
    };
    let tmp_route = |suffix: &str| {
        if tmp_documents_enabled {
            Some(format!("{api_base}/tmp/documents/{{secret}}{suffix}"))
        } else {
            None
        }
    };
    json_response(
        StatusCode::OK,
        &AgentDiscovery {
            name: "quarry",
            api_base: api_base.clone(),
            docs_url: format!("{origin}/agent-docs"),
            skill_url: format!("{origin}/quarry.SKILL.md"),
            openapi_url: format!("{api_base}/openapi.json"),
            capabilities,
            auth_note: "Tmp document URLs are bearer capabilities: anyone with /tmp/{secret} can access that tmp document. Library REST APIs remain trusted-localhost for now.",
            auth: AgentDiscoveryAuth {
                mode: "trusted_localhost",
                token_role: "tmp_capability_url",
                required_headers: vec!["Content-Type", "X-Agent-Id"],
                note: "Tmp document URL secrets authorize tmp access. Use X-Agent-Id to identify each agent.",
            },
            presence_statuses: vec![
                "reading",
                "thinking",
                "acting",
                "waiting",
                "completed",
                "error",
            ],
            transaction_operations: crate::gateway::PUBLIC_TRANSACTION_OPERATIONS.to_vec(),
            limitations: vec![
                "Library REST agent endpoints in the full/local build trust localhost and do not currently enforce bearer-token auth.",
                "Tmp document URL secrets are bearer capabilities; do not log or redistribute them.",
                "Library invite URL tokens identify browser/collab joins and are not REST bearer tokens.",
                "Quarry does not currently support rewrite.apply.",
            ],
            route_hints: AgentDiscoveryRouteHints {
                presence: library_route("/presence"),
                snapshot: library_route("/snapshot"),
                blocks: library_route("/blocks"),
                transactions: library_route("/transactions"),
                review: library_route("/review"),
                events_stream: library_route("/events/stream"),
                events_pending: if lib_documents_enabled {
                    Some(format!(
                        "{api_base}/libraries/{{library}}/events/pending?after={{last-seen-id}}"
                    ))
                } else {
                    None
                },
                tmp_document: if tmp_documents_enabled {
                    Some(format!("{api_base}/tmp/documents/{{secret}}"))
                } else {
                    None
                },
                tmp_presence: tmp_route("/presence"),
                tmp_blocks: tmp_route("/blocks"),
                tmp_transactions: tmp_route("/transactions"),
                tmp_review: tmp_route("/review"),
                tmp_events_stream: tmp_route("/events/stream"),
                tmp_share: None,
            },
            endpoints,
        },
    )
}

fn discovery_endpoint(
    method: &'static str,
    path: &'static str,
    api_base: &str,
) -> AgentDiscoveryEndpoint {
    AgentDiscoveryEndpoint {
        method,
        path,
        url: format!("{}{}", api_base.trim_end_matches("/v1"), path),
    }
}

pub(crate) fn request_origin(headers: &HeaderMap) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("127.0.0.1:7831");
    format!("{scheme}://{host}")
}

fn static_text_response(content_type: &'static str, body: &'static str) -> Response {
    let mut response = Response::new(Body::from(body));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}
