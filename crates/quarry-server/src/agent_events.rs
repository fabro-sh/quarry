use crate::sse::{StoreEventPayloadMode, store_event_payload, store_event_type};
use crate::{ApiError, AppState, ErrorResponse, agent_id_from_headers_or_body};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use utoipa::ToSchema;

#[derive(Debug, Deserialize)]
pub(crate) struct AgentPendingEventsQuery {
    after: Option<u64>,
    limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentEventRecord {
    pub id: u64,
    pub event: String,
    pub data: JsonValue,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentPendingEventsResponse {
    pub events: Vec<AgentEventRecord>,
    #[serde(rename = "nextAfter")]
    pub next_after: u64,
}

#[derive(Clone, Debug, Deserialize, ToSchema)]
pub(crate) struct AgentEventsAckRequest {
    #[serde(default, rename = "agentId")]
    pub agent_id: Option<String>,
    #[serde(rename = "eventId")]
    pub event_id: u64,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct AgentEventsAckResponse {
    pub ok: bool,
    #[serde(rename = "agentId")]
    pub agent_id: String,
    #[serde(rename = "ackedThrough")]
    pub acked_through: u64,
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/events/pending",
    params(("library" = String, Path), ("after" = Option<u64>, Query), ("limit" = Option<usize>, Query)),
    responses((status = 200, body = AgentPendingEventsResponse), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn agent_events_pending(
    State(state): State<AppState>,
    Path(library): Path<String>,
    Query(query): Query<AgentPendingEventsQuery>,
) -> Result<Json<AgentPendingEventsResponse>, ApiError> {
    let library = state.store.get_library(&library).await?;
    let after = query.after.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let pending = state
        .agent_events
        .pending_since(&library.id, after, limit)
        .await;
    let next_after = pending.last().map(|event| event.id).unwrap_or(after);
    let events = pending
        .into_iter()
        .map(|logged| {
            let event_type = store_event_type(&logged.event);
            let mut data = store_event_payload(
                &library.slug,
                &event_type,
                &logged.event,
                StoreEventPayloadMode::IncludePaths,
            );
            if let Some(object) = data.as_object_mut() {
                object.insert("event_id".to_string(), JsonValue::from(logged.id));
            }
            AgentEventRecord {
                id: logged.id,
                event: event_type,
                data,
            }
        })
        .collect();

    Ok(Json(AgentPendingEventsResponse { events, next_after }))
}

#[utoipa::path(
    post,
    path = "/v1/libraries/{library}/events/ack",
    params(("library" = String, Path)),
    request_body = AgentEventsAckRequest,
    responses((status = 200, body = AgentEventsAckResponse), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn agent_events_ack(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(library): Path<String>,
    Json(request): Json<AgentEventsAckRequest>,
) -> Result<Json<AgentEventsAckResponse>, ApiError> {
    state.store.get_library(&library).await?;
    let agent_id = agent_id_from_headers_or_body(&headers, request.agent_id.as_deref())?;
    state
        .agent_events
        .ack(agent_id.clone(), request.event_id)
        .await;
    Ok(Json(AgentEventsAckResponse {
        ok: true,
        agent_id,
        acked_through: request.event_id,
    }))
}
