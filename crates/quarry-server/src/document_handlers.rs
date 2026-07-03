use crate::presence::PresenceStreamGuard;
use crate::review::{DocumentReviewQuery, agent_document_review, agent_document_snapshot};
use crate::sse::events_for_library;
use crate::{
    ApiError, AppState, DocumentSubResource, ErrorResponse, bytes_response_with_expiry, gateway,
    insert_document_headers, json_response, optional_header, parse_document_subresource,
    touch_agent_presence, transaction_metadata_from_headers,
};
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use quarry_core::{DocumentListEntry, DocumentSource, TransactionRecord};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct ListQuery {
    prefix: Option<String>,
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DocumentGetQuery {
    against: Option<String>,
    #[serde(default, flatten)]
    review: DocumentReviewQuery,
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents",
    params(("library" = String, Path), ("prefix" = Option<String>, Query), ("limit" = Option<u64>, Query)),
    responses((status = 200, body = [DocumentListEntry]))
)]
pub(crate) async fn list_documents(
    State(state): State<AppState>,
    Path(library): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<DocumentListEntry>>, ApiError> {
    Ok(Json(
        state
            .store
            .list_documents(&library, query.prefix.as_deref(), query.limit)
            .await?,
    ))
}

#[utoipa::path(
    get,
    path = "/v1/libraries/{library}/documents/{path}",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = String), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn get_document(
    State(state): State<AppState>,
    Query(query): Query<DocumentGetQuery>,
    Path((library, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (document_path, subresource) = parse_document_subresource(&path);
    match subresource {
        DocumentSubResource::Backlinks => {
            return json_response(
                StatusCode::OK,
                &state.store.backlinks(&library, document_path).await?,
            );
        }
        DocumentSubResource::OutgoingLinks => {
            return json_response(
                StatusCode::OK,
                &state.store.outgoing_links(&library, document_path).await?,
            );
        }
        DocumentSubResource::Blocks => {
            touch_agent_presence(&state, &headers, Some(&library), document_path).await?;
            return gateway::document_blocks(&state, &library, document_path).await;
        }
        DocumentSubResource::Snapshot => {
            touch_agent_presence(&state, &headers, Some(&library), document_path).await?;
            return json_response(
                StatusCode::OK,
                &agent_document_snapshot(&state.store, &library, document_path).await?,
            );
        }
        DocumentSubResource::Review => {
            touch_agent_presence(&state, &headers, Some(&library), document_path).await?;
            let include_resolved = query.review.include_resolved()?;
            return json_response(
                StatusCode::OK,
                &agent_document_review(&state.store, &library, document_path, include_resolved)
                    .await?,
            );
        }
        DocumentSubResource::Presence => {
            touch_agent_presence(&state, &headers, Some(&library), document_path).await?;
            state.store.head_document(&library, document_path).await?;
            return json_response(
                StatusCode::OK,
                &state.agent_presence.list(Some(&library), document_path),
            );
        }
        DocumentSubResource::EventsStream => {
            let document = state.store.head_document(&library, document_path).await?;
            let presence_guard = optional_header(&headers, "x-agent-id")?.map(|agent_id| {
                PresenceStreamGuard::open(
                    state.agent_presence.clone(),
                    Some(library.clone()),
                    document_path.to_string(),
                    document.id,
                    agent_id,
                )
            });
            return Ok(events_for_library(
                &state.store,
                &library,
                Some(document_path.to_string()),
                presence_guard,
                state.shutdown_token(),
            )
            .await?
            .into_response());
        }
        DocumentSubResource::Share => {
            return json_response(
                StatusCode::OK,
                &state
                    .store
                    .collab_invite_tokens(&library, document_path)
                    .await?,
            );
        }
        DocumentSubResource::RawVersions => {
            return json_response(
                StatusCode::OK,
                &state
                    .store
                    .raw_version_history(&library, document_path)
                    .await?,
            );
        }
        DocumentSubResource::Versions => {
            return json_response(
                StatusCode::OK,
                &state.store.version_history(&library, document_path).await?,
            );
        }
        DocumentSubResource::Version(version) => {
            return json_response(
                StatusCode::OK,
                &state
                    .store
                    .document_version(&library, document_path, version)
                    .await?,
            );
        }
        DocumentSubResource::VersionDiff(version) => {
            return json_response(
                StatusCode::OK,
                &state
                    .store
                    .version_diff(&library, document_path, version, query.against.as_deref())
                    .await?,
            );
        }
        DocumentSubResource::Document
        | DocumentSubResource::Metadata
        | DocumentSubResource::Move
        | DocumentSubResource::ShareRevoke(_)
        | DocumentSubResource::Transactions
        | DocumentSubResource::Ttl
        | DocumentSubResource::VersionRestore(_) => {}
    }

    touch_agent_presence(&state, &headers, Some(&library), &path).await?;
    let document = state.store.get_document(&library, &path).await?;
    bytes_response_with_expiry(
        StatusCode::OK,
        document.content,
        &document.version.content_type,
        &document.version.id,
        &document.id,
        document.expires_at.as_deref(),
    )
}

#[utoipa::path(
    head,
    path = "/v1/libraries/{library}/documents/{path}",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200), (status = 404, body = ErrorResponse))
)]
pub(crate) async fn head_document(
    State(state): State<AppState>,
    Path((library, path)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let document = state.store.head_document(&library, &path).await?;
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::OK;
    insert_document_headers(
        response.headers_mut(),
        &document.content_type,
        &document.head_version_id,
        &document.id,
        document.expires_at.as_deref(),
    )?;
    Ok(response)
}

#[utoipa::path(
    delete,
    path = "/v1/libraries/{library}/documents/{path}",
    params(("library" = String, Path), ("path" = String, Path)),
    responses((status = 200, body = TransactionRecord))
)]
pub(crate) async fn delete_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((library, path)): Path<(String, String)>,
) -> Result<Json<TransactionRecord>, ApiError> {
    let origin_id = optional_header(&headers, "x-quarry-origin-id")?;
    let actor = transaction_metadata_from_headers(&headers)?.actor;
    Ok(Json(
        state
            .store
            .delete_document_with_origin(&library, &path, DocumentSource::Rest, origin_id, actor)
            .await?,
    ))
}
