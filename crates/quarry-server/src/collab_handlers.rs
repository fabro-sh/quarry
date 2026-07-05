use crate::session::CollabAccess;
use crate::{ApiError, AppState};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};

#[utoipa::path(
    get,
    path = "/v1/collab/{document_id}",
    params(("document_id" = String, Path)),
    responses((status = 101, description = "Yjs collaboration websocket"))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn collab_websocket_openapi() {}

pub(crate) async fn collab_websocket(
    State(state): State<AppState>,
    Path(document_id): Path<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let shutdown = state.shutdown_token();
    ws.on_upgrade(move |socket| async move {
        state
            .sessions
            .serve_socket(document_id, CollabAccess::LibraryOnly, socket, shutdown)
            .await;
    })
}

#[utoipa::path(
    get,
    path = "/v1/tmp/collab/{secret}/{room}",
    params(("secret" = String, Path), ("room" = String, Path)),
    responses((status = 101, description = "Yjs collaboration websocket for tmp capability documents"))
)]
#[expect(
    dead_code,
    reason = "OpenAPI documentation stubs are referenced by utoipa derive, not called at runtime"
)]
pub(crate) async fn tmp_collab_websocket_openapi() {}

pub(crate) async fn tmp_collab_websocket(
    State(state): State<AppState>,
    Path((secret, _room)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let document = state.store.head_tmp_document(&secret).await?;
    let shutdown = state.shutdown_token();
    Ok(ws
        .on_upgrade(move |socket| async move {
            state
                .sessions
                .serve_socket(
                    document.id.to_string(),
                    CollabAccess::TmpAuthorized,
                    socket,
                    shutdown,
                )
                .await;
        })
        .into_response())
}
