use crate::headers::etag;
use crate::presence::PresenceStreamGuard;
use crate::{ApiError, AppState};
use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::{Stream, stream};
use quarry_storage::{DocumentScopeRef, QuarryStore, StoreEvent, StoreEventKind};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::convert::Infallible;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Deserialize)]
pub(crate) struct EventsQuery {
    library: String,
}

#[utoipa::path(
    get,
    path = "/v1/events",
    params(("library" = String, Query)),
    responses((status = 200, description = "Server-sent event stream"))
)]
pub(crate) async fn events(
    State(state): State<AppState>,
    Query(query): Query<EventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>> + use<>>, ApiError> {
    events_for_library(
        &state.store,
        &query.library,
        None,
        None,
        state.shutdown_token(),
    )
    .await
}

pub(crate) async fn events_for_library(
    store: &QuarryStore,
    library: &str,
    document_path: Option<String>,
    presence_guard: Option<PresenceStreamGuard>,
    shutdown: CancellationToken,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>> + use<>>, ApiError> {
    let library = store.get_library(library).await?;
    tracing::debug!(
        event = "sse.stream.opened",
        library = %library.slug,
        library_id = %library.id,
        path = document_path.as_deref().unwrap_or(""),
        "SSE stream opened"
    );
    let receiver = store.subscribe_events();
    let stream = stream::unfold(
        (
            receiver,
            library.id,
            library.slug,
            document_path,
            presence_guard,
            shutdown,
        ),
        |(mut receiver, library_id, library_slug, document_path, presence_guard, shutdown)| async move {
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => {
                        tracing::debug!(
                            event = "sse.stream.closed",
                            library = %library_slug,
                            library_id = %library_id,
                            reason_code = "shutdown",
                            "SSE stream closed for shutdown"
                        );
                        return None;
                    }
                    received = receiver.recv() => {
                        match received {
                            Ok(store_event)
                                if store_event.library_id() == library_id
                                    && event_matches_document_filter(
                                        &store_event,
                                        document_path.as_deref(),
                                    ) =>
                            {
                                let event_type = store_event_type(&store_event);
                                let payload = store_event_payload(
                                    &library_slug,
                                    &event_type,
                                    &store_event,
                                    StoreEventPayloadMode::IncludePaths,
                                );
                                tracing::debug!(
                                    event = "sse.event.sent",
                                    library = %library_slug,
                                    library_id = %library_id,
                                    sse_event = %event_type,
                                    path = store_event.path().unwrap_or(""),
                                    new_path = store_event.new_path().unwrap_or(""),
                                    tx_id = store_event.tx_id().unwrap_or(""),
                                    doc_id = store_event.doc_id().unwrap_or(""),
                                    version_id = store_event.version_id().unwrap_or(""),
                                    conflict_id = store_event.conflict_id().unwrap_or(""),
                                    origin_id = store_event.origin_id().unwrap_or(""),
                                    "SSE event sent"
                                );
                                let event = Event::default().event(event_type).data(payload.to_string());
                                return Some((
                                    Ok(event),
                                    (
                                        receiver,
                                        library_id,
                                        library_slug,
                                        document_path,
                                        presence_guard,
                                        shutdown,
                                    ),
                                ));
                            }
                            Ok(_) => continue,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                tracing::warn!(
                                    event = "sse.stream.lagged",
                                    library = %library_slug,
                                    library_id = %library_id,
                                    skipped,
                                    "SSE stream lagged"
                                );
                                let event_type = "stream.lagged".to_string();
                                let payload = serde_json::json!({
                                    "type": event_type,
                                    "library": library_slug,
                                    "skipped": skipped
                                });
                                let event = Event::default().event(event_type).data(payload.to_string());
                                return Some((
                                    Ok(event),
                                    (
                                        receiver,
                                        library_id,
                                        library_slug,
                                        document_path,
                                        presence_guard,
                                        shutdown,
                                    ),
                                ));
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                tracing::debug!(
                                    event = "sse.stream.closed",
                                    library = %library_slug,
                                    library_id = %library_id,
                                    "SSE stream closed"
                                );
                                return None;
                            }
                        }
                    }
                }
            }
        },
    );
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

pub(crate) async fn events_for_tmp_document(
    store: &QuarryStore,
    document_path: String,
    document_id: String,
    presence_guard: Option<PresenceStreamGuard>,
    shutdown: CancellationToken,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>> + use<>>, ApiError> {
    tracing::debug!(
        event = "sse.stream.opened",
        scope = %"tmp",
        document_id = %document_id,
        "tmp SSE stream opened"
    );
    let store_receiver = store.subscribe_events();
    let stream = stream::unfold(
        (
            store_receiver,
            document_path,
            document_id,
            presence_guard,
            shutdown,
        ),
        |(mut store_receiver, document_path, document_id, presence_guard, shutdown)| async move {
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => {
                        tracing::debug!(
                            event = "sse.stream.closed",
                            scope = %"tmp",
                            document_id = %document_id,
                            reason_code = "shutdown",
                            "tmp SSE stream closed for shutdown"
                        );
                        return None;
                    }
                    received = store_receiver.recv() => {
                        match received {
                            Ok(store_event)
                                if store_event.library_id() == DocumentScopeRef::Tmp.event_library_id()
                                    && event_matches_document_filter(&store_event, Some(&document_path)) =>
                            {
                                let event_type = store_event_type(&store_event);
                                let payload = store_event_payload(
                                    "tmp",
                                    &event_type,
                                    &store_event,
                                    StoreEventPayloadMode::OmitPaths,
                                );
                                let event = Event::default().event(event_type).data(payload.to_string());
                                return Some((
                                    Ok(event),
                                    (
                                        store_receiver,
                                        document_path,
                                        document_id,
                                        presence_guard,
                                        shutdown,
                                    ),
                                ));
                            }
                            Ok(_) => continue,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                tracing::warn!(
                                    event = "sse.stream.lagged",
                                    scope = %"tmp",
                                    skipped,
                                    "tmp SSE stream lagged"
                                );
                                let event_type = "stream.lagged".to_string();
                                let payload = serde_json::json!({
                                    "type": event_type,
                                    "library": "tmp",
                                    "skipped": skipped
                                });
                                let event = Event::default().event(event_type).data(payload.to_string());
                                return Some((
                                    Ok(event),
                                    (
                                        store_receiver,
                                        document_path,
                                        document_id,
                                        presence_guard,
                                        shutdown,
                                    ),
                                ));
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                        }
                    }
                }
            }
        },
    );
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

fn event_matches_document_filter(event: &StoreEvent, document_path: Option<&str>) -> bool {
    let Some(document_path) = document_path else {
        return true;
    };
    event.path() == Some(document_path) || event.new_path() == Some(document_path)
}

pub(crate) fn store_event_type(event: &StoreEvent) -> String {
    match event.kind() {
        StoreEventKind::DocumentPut => "doc.changed",
        StoreEventKind::DocumentDelete => "doc.deleted",
        StoreEventKind::DocumentMove => "doc.moved",
        StoreEventKind::LinksIndexed => "links.indexed",
        StoreEventKind::ConflictCreated => "conflict.created",
        StoreEventKind::ConflictResolved => "conflict.resolved",
        StoreEventKind::LibraryReindexed => "library.reindexed",
        StoreEventKind::GitSyncCompleted => "git.sync.completed",
        StoreEventKind::DirectoryPut
        | StoreEventKind::DirectoryDelete
        | StoreEventKind::DirectoryMove => "directory.changed",
    }
    .to_string()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StoreEventPayloadMode {
    IncludePaths,
    OmitPaths,
}

pub(crate) fn store_event_payload(
    library: &str,
    event_type: &str,
    event: &StoreEvent,
    mode: StoreEventPayloadMode,
) -> JsonValue {
    let mut payload = serde_json::json!({
        "type": event_type,
        "library": library,
        "source": event.source(),
        "tx_id": event.tx_id()
    });
    if let Some(object) = payload.as_object_mut() {
        if mode == StoreEventPayloadMode::IncludePaths {
            object.insert(
                "path".to_string(),
                event
                    .path()
                    .map(str::to_string)
                    .map(JsonValue::String)
                    .unwrap_or(JsonValue::Null),
            );
            if matches!(
                event.kind(),
                StoreEventKind::DocumentMove | StoreEventKind::DirectoryMove
            ) {
                object.insert(
                    "from".to_string(),
                    event
                        .path()
                        .map(str::to_string)
                        .map(JsonValue::String)
                        .unwrap_or(JsonValue::Null),
                );
                object.insert(
                    "to".to_string(),
                    event
                        .new_path()
                        .map(str::to_string)
                        .map(JsonValue::String)
                        .unwrap_or(JsonValue::Null),
                );
            }
        }
        if let Some(conflict_id) = event.conflict_id() {
            object.insert(
                "conflict_id".to_string(),
                JsonValue::String(conflict_id.to_string()),
            );
        }
        if let Some(doc_id) = event.doc_id() {
            object.insert("doc_id".to_string(), JsonValue::String(doc_id.to_string()));
        }
        if let Some(version_id) = event.version_id() {
            object.insert(
                "version_id".to_string(),
                JsonValue::String(version_id.to_string()),
            );
            object.insert("etag".to_string(), JsonValue::String(etag(version_id)));
        }
        if let Some(peer_id) = event.peer_id() {
            object.insert(
                "peer_id".to_string(),
                JsonValue::String(peer_id.to_string()),
            );
        }
        if let Some(applied) = event.applied() {
            object.insert("applied".to_string(), JsonValue::from(applied));
        }
        if let Some(conflicts) = event.conflicts() {
            object.insert("conflicts".to_string(), JsonValue::from(conflicts));
        }
        if let Some(origin_id) = event.origin_id() {
            object.insert(
                "origin_id".to_string(),
                JsonValue::String(origin_id.to_string()),
            );
        }
    }
    payload
}
