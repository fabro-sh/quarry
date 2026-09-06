//! Browser notifications use WebSockets so idle streams cannot occupy the
//! HTTP connections needed to publish commands. HTTP clients retain SSE.
use crate::{ApiError, ApiErrorCode};
use axum::extract::FromRequestParts;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{Uri, header, request::Parts};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, Stream, StreamExt};
use serde_json::Value;
use std::convert::Infallible;
use std::time::Duration;

pub(crate) struct EventTransport(Option<WebSocketUpgrade>);

impl<S: Send + Sync> FromRequestParts<S> for EventTransport {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let websocket = parts
            .headers
            .get(header::UPGRADE)
            .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"websocket"));
        if !websocket {
            return Ok(Self(None));
        }
        // WebSockets do not enforce fetch CORS. Browser subscriptions must
        // come from this origin; non-browser clients can omit Origin.
        if let Some(origin) = parts.headers.get(header::ORIGIN) {
            let uri = origin
                .to_str()
                .ok()
                .and_then(|value| value.parse::<Uri>().ok());
            let host = parts
                .headers
                .get(header::HOST)
                .and_then(|value| value.to_str().ok());
            if !uri.as_ref().is_some_and(|uri| {
                matches!(uri.scheme_str(), Some("http" | "https"))
                    && uri.authority().map(|value| value.as_str()) == host
            }) {
                return Err(ApiError::new(
                    ApiErrorCode::Forbidden,
                    "Event subscriptions require the same origin",
                )
                .into_response());
            }
        }
        WebSocketUpgrade::from_request_parts(parts, state)
            .await
            .map(|upgrade| Self(Some(upgrade)))
            .map_err(IntoResponse::into_response)
    }
}

pub(crate) fn event_response<S>(events: S, transport: EventTransport) -> Response
where
    S: Stream<Item = Result<Value, Infallible>> + Send + 'static,
{
    if let Some(upgrade) = transport.0 {
        return upgrade
            .max_message_size(1024)
            .max_frame_size(1024)
            .on_upgrade(|socket| serve_socket(socket, events))
            .into_response();
    }
    Sse::new(events.map(|result| {
        result.map(|payload| {
            Event::default()
                .event(payload["type"].as_str().unwrap_or("message"))
                .data(payload.to_string())
        })
    }))
    .keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    )
    .into_response()
}

async fn serve_socket<S>(mut socket: WebSocket, events: S)
where
    S: Stream<Item = Result<Value, Infallible>> + Send + 'static,
{
    let mut events = Box::pin(events);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    loop {
        let message = tokio::select! {
            received = socket.recv() => match received {
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                // This channel only sends notifications. Content changes
                // always use the authenticated, durable HTTP command path.
                _ => break,
            },
            event = events.next() => match event {
                Some(Ok(payload)) => Message::Text(payload.to_string().into()),
                _ => break,
            },
            _ = heartbeat.tick() => Message::Ping(Vec::new().into()),
        };
        if !matches!(
            tokio::time::timeout(Duration::from_secs(5), socket.send(message)).await,
            Ok(Ok(()))
        ) {
            break;
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close()).await;
}
