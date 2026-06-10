//! Collab websocket transport: y-sync v1 protocol plumbing between Axum
//! websockets and a live session's shared doc/awareness.
//!
//! All lifecycle (seeding, checkpoints, discard, the per-document mutex) and
//! persistence live in [`crate::session`]; this module only moves bytes. The
//! legacy injection gate, recovery-state persistence, and clean-room
//! reseeding that used to live here were deleted by Phase 3 of the
//! session-scoped collaboration rewrite: a live session is the write path
//! now, not an obstacle to it, so there is nothing to gate and nothing to
//! recover (sessions reseed from canonical rows).

use crate::session::LiveSession;
use axum::body::Bytes;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::select;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use yrs::sync::{DefaultProtocol, Error, Protocol};
use yrs::updates::encoder::Encode;

/// The shared Yjs root the browser's Plate binding edits. Root Y.Text and
/// Y.XmlText share wire updates; yrs exposes root creation as TextRef.
pub(crate) const SHARED_ROOT: &str = "content";

/// Pumps one websocket against a live session until either side closes:
/// broadcast fan-out → socket, and socket → y-sync protocol handling on the
/// session's awareness/doc.
pub(crate) async fn serve_session_socket(
    session: &LiveSession,
    socket: WebSocket,
    collab_session_id: &str,
) -> Result<(), Error> {
    let (sink, stream) = socket.split();
    let sink = Arc::new(Mutex::new(AxumSink::from(sink)));
    let stream = AxumStream::from(stream);

    // Subscribe before sending the join-time checkpoint ack so no commit
    // lands in the gap: the initial frame covers everything before the
    // subscription, the broadcast covers everything after.
    let receiver = session.subscribe_broadcast();
    sink.lock()
        .await
        .send(session.committed_ack_frame())
        .await?;

    let sink_task: JoinHandle<Result<(), Error>> = {
        let sink = sink.clone();
        let mut receiver = receiver;
        let document_id = session.document_id.clone();
        let collab_session_id = collab_session_id.to_string();
        tokio::spawn(async move {
            loop {
                let message = receiver.recv().await;
                let Ok(payload) = message else {
                    // Lagged or closed: a lagging receiver would desync the
                    // y-sync stream; closing makes the client resync.
                    return Ok(());
                };
                let update_bytes = payload.len();
                let mut sink = sink.lock().await;
                sink.send(payload).await?;
                tracing::trace!(
                    event = "collab.update.broadcast",
                    %document_id,
                    %collab_session_id,
                    update_bytes,
                    "collab update sent to socket"
                );
            }
        })
    };

    let stream_task: JoinHandle<Result<(), Error>> = {
        let awareness = session.awareness().clone();
        let document_id = session.document_id.clone();
        let collab_session_id = collab_session_id.to_string();
        let mut stream = stream;
        tokio::spawn(async move {
            loop {
                let Some(result) = stream.next().await else {
                    return Ok(());
                };
                let payload = result?;
                tracing::trace!(
                    event = "collab.update.received",
                    %document_id,
                    %collab_session_id,
                    update_bytes = payload.len(),
                    "collab update received from socket"
                );
                let replies = {
                    let mut awareness = awareness.write().await;
                    DefaultProtocol.handle(&mut awareness, &payload)?
                };
                for reply in replies {
                    let encoded = reply.encode_v1();
                    let mut sink = sink.lock().await;
                    sink.send(encoded)
                        .await
                        .map_err(|error| Error::Other(Box::new(error)))?;
                }
            }
        })
    };

    let subscription = Subscription {
        sink_task,
        stream_task,
    };
    subscription.completed().await
}

#[derive(Debug)]
struct Subscription {
    sink_task: JoinHandle<Result<(), Error>>,
    stream_task: JoinHandle<Result<(), Error>>,
}

impl Subscription {
    async fn completed(mut self) -> Result<(), Error> {
        let result = select! {
            sink = &mut self.sink_task => sink,
            stream = &mut self.stream_task => stream,
        };
        self.sink_task.abort();
        self.stream_task.abort();
        result.map_err(|error| Error::Other(error.into()))?
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.sink_task.abort();
        self.stream_task.abort();
    }
}

#[derive(Debug)]
struct AxumSink(SplitSink<WebSocket, WsMessage>);

impl From<SplitSink<WebSocket, WsMessage>> for AxumSink {
    fn from(sink: SplitSink<WebSocket, WsMessage>) -> Self {
        Self(sink)
    }
}

impl Sink<Vec<u8>> for AxumSink {
    type Error = Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_ready(cx)
            .map_err(|error| Error::Other(error.into()))
    }

    fn start_send(mut self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
        Pin::new(&mut self.0)
            .start_send(WsMessage::Binary(Bytes::from(item)))
            .map_err(|error| Error::Other(error.into()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_flush(cx)
            .map_err(|error| Error::Other(error.into()))
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0)
            .poll_close(cx)
            .map_err(|error| Error::Other(error.into()))
    }
}

#[derive(Debug)]
struct AxumStream(SplitStream<WebSocket>);

impl From<SplitStream<WebSocket>> for AxumStream {
    fn from(stream: SplitStream<WebSocket>) -> Self {
        Self(stream)
    }
}

impl Stream for AxumStream {
    type Item = Result<Vec<u8>, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match Pin::new(&mut self.0).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(Ok(WsMessage::Binary(bytes)))) => {
                    return Poll::Ready(Some(Ok(bytes.to_vec())));
                }
                Poll::Ready(Some(Ok(WsMessage::Close(_)))) => return Poll::Ready(None),
                Poll::Ready(Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_)))) => continue,
                Poll::Ready(Some(Ok(WsMessage::Text(_)))) => {
                    return Poll::Ready(Some(Err(Error::Other(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "expected binary collab websocket message",
                    ))))));
                }
                Poll::Ready(Some(Err(error))) => {
                    return Poll::Ready(Some(Err(Error::Other(error.into()))));
                }
            }
        }
    }
}
