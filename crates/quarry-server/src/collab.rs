use axum::body::Bytes;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::select;
use tokio::sync::broadcast::{channel, Receiver, Sender};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use yrs::encoding::write::Write;
use yrs::sync::protocol::{MSG_SYNC, MSG_SYNC_UPDATE};
use yrs::sync::{Awareness, DefaultProtocol, Error, Message, Protocol};
use yrs::updates::encoder::{Encode, Encoder, EncoderV1};
#[cfg(test)]
use yrs::GetString;
#[cfg(test)]
use yrs::ReadTxn;
use yrs::{Doc, Transact, WriteTxn};

pub(crate) const SHARED_ROOT: &str = "content";

type AwarenessRef = Arc<RwLock<Awareness>>;

#[derive(Clone, Default)]
pub(crate) struct CollabHub {
    rooms: Arc<RwLock<HashMap<String, Arc<CollabRoom>>>>,
}

impl CollabHub {
    pub(crate) async fn serve_socket(&self, document_id: String, socket: WebSocket) {
        let room = self.room(&document_id).await;
        room.serve_socket(socket).await;
    }

    async fn room(&self, document_id: &str) -> Arc<CollabRoom> {
        if let Some(room) = self.rooms.read().await.get(document_id).cloned() {
            return room;
        }

        let mut rooms = self.rooms.write().await;
        if let Some(room) = rooms.get(document_id).cloned() {
            return room;
        }

        let room = Arc::new(CollabRoom::new().await);
        rooms.insert(document_id.to_string(), room.clone());
        room
    }

    #[cfg(test)]
    pub(crate) async fn room_count(&self) -> usize {
        self.rooms.read().await.len()
    }
}

pub(crate) struct CollabRoom {
    broadcast: BroadcastGroup,
}

impl CollabRoom {
    async fn new() -> Self {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            // Yjs root Y.Text and Y.XmlText share wire updates; yrs exposes root creation as TextRef.
            txn.get_or_insert_text(SHARED_ROOT);
        }
        let awareness = Arc::new(RwLock::new(Awareness::new(doc)));

        Self {
            broadcast: BroadcastGroup::new(awareness, 32).await,
        }
    }

    async fn serve_socket(&self, socket: WebSocket) {
        let (sink, stream) = socket.split();
        let sink = Arc::new(Mutex::new(AxumSink::from(sink)));
        let stream = AxumStream::from(stream);
        let subscription = self.broadcast.subscribe(sink, stream);

        if let Err(error) = subscription.completed().await {
            tracing::debug!(%error, "collab websocket closed with protocol error");
        }
    }

    #[cfg(test)]
    async fn content_text(&self) -> Option<String> {
        let awareness = self.broadcast.awareness().read().await;
        let txn = awareness.doc().transact();
        txn.get_text(SHARED_ROOT)
            .map(|content| content.get_string(&txn))
    }
}

struct BroadcastGroup {
    _awareness_sub: yrs::Subscription,
    _doc_sub: yrs::Subscription,
    awareness_ref: AwarenessRef,
    sender: Sender<Vec<u8>>,
    _receiver: Receiver<Vec<u8>>,
    awareness_updater: JoinHandle<()>,
}

unsafe impl Send for BroadcastGroup {}
unsafe impl Sync for BroadcastGroup {}

impl BroadcastGroup {
    async fn new(awareness: AwarenessRef, buffer_capacity: usize) -> Self {
        let (sender, receiver) = channel(buffer_capacity);
        let awareness_c = Arc::downgrade(&awareness);

        let mut lock = awareness.write().await;
        let sink = sender.clone();
        let doc_sub = {
            lock.doc()
                .observe_update_v1(move |_txn, update| {
                    let mut encoder = EncoderV1::new();
                    encoder.write_var(MSG_SYNC);
                    encoder.write_var(MSG_SYNC_UPDATE);
                    encoder.write_buf(&update.update);
                    let _ = sink.send(encoder.to_vec());
                })
                .unwrap()
        };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = sender.clone();
        let awareness_sub = lock.on_update(move |_awareness, event, _origin| {
            if tx.send(event.all_changes()).is_err() {
                tracing::warn!("failed to queue collab awareness update");
            }
        });
        drop(lock);

        let awareness_updater = tokio::task::spawn(async move {
            while let Some(changed_clients) = rx.recv().await {
                let Some(awareness) = awareness_c.upgrade() else {
                    return;
                };
                let awareness = awareness.read().await;
                match awareness.update_with_clients(changed_clients) {
                    Ok(update) => {
                        if sink.send(Message::Awareness(update).encode_v1()).is_err() {
                            tracing::warn!("failed to broadcast collab awareness update");
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "failed to compute collab awareness update");
                    }
                }
            }
        });

        Self {
            _awareness_sub: awareness_sub,
            _doc_sub: doc_sub,
            awareness_ref: awareness,
            sender,
            _receiver: receiver,
            awareness_updater,
        }
    }

    fn awareness(&self) -> &AwarenessRef {
        &self.awareness_ref
    }

    fn subscribe<S, St, E>(&self, sink: Arc<Mutex<S>>, stream: St) -> Subscription
    where
        S: SinkExt<Vec<u8>> + Send + Sync + Unpin + 'static,
        St: StreamExt<Item = Result<Vec<u8>, E>> + Send + Sync + Unpin + 'static,
        <S as Sink<Vec<u8>>>::Error: std::error::Error + Send + Sync,
        E: std::error::Error + Send + Sync + 'static,
    {
        self.subscribe_with(sink, stream, DefaultProtocol)
    }

    fn subscribe_with<S, St, E, P>(
        &self,
        sink: Arc<Mutex<S>>,
        mut stream: St,
        protocol: P,
    ) -> Subscription
    where
        S: SinkExt<Vec<u8>> + Send + Sync + Unpin + 'static,
        St: StreamExt<Item = Result<Vec<u8>, E>> + Send + Sync + Unpin + 'static,
        <S as Sink<Vec<u8>>>::Error: std::error::Error + Send + Sync,
        E: std::error::Error + Send + Sync + 'static,
        P: Protocol + Send + Sync + 'static,
    {
        let sink_task = {
            let sink = sink.clone();
            let mut receiver = self.sender.subscribe();
            tokio::spawn(async move {
                while let Ok(msg) = receiver.recv().await {
                    let mut sink = sink.lock().await;
                    if let Err(error) = sink.send(msg).await {
                        return Err(Error::Other(Box::new(error)));
                    }
                }
                Ok(())
            })
        };

        let stream_task = {
            let awareness = self.awareness().clone();
            tokio::spawn(async move {
                while let Some(result) = stream.next().await {
                    let payload = result.map_err(|error| Error::Other(Box::new(error)))?;
                    let replies = {
                        let mut awareness = awareness.write().await;
                        protocol.handle(&mut awareness, &payload)?
                    };

                    for reply in replies {
                        let mut sink = sink.lock().await;
                        sink.send(reply.encode_v1())
                            .await
                            .map_err(|error| Error::Other(Box::new(error)))?;
                    }
                }
                Ok(())
            })
        };

        Subscription {
            sink_task,
            stream_task,
        }
    }
}

impl Drop for BroadcastGroup {
    fn drop(&mut self) {
        self.awareness_updater.abort();
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{ready, SinkExt, StreamExt};
    use std::task::{Context, Poll};
    use tokio::sync::mpsc;
    use yrs::sync::{Message, SyncMessage};
    use yrs::updates::decoder::Decode;
    use yrs::updates::encoder::Encode;

    #[tokio::test]
    async fn rooms_are_keyed_by_document_id() {
        let hub = CollabHub::default();

        let first = hub.room("doc-1").await;
        let second = hub.room("doc-1").await;
        let other = hub.room("doc-2").await;

        assert!(Arc::ptr_eq(&first, &second));
        assert!(!Arc::ptr_eq(&first, &other));
        assert_eq!(hub.room_count().await, 2);
    }

    #[tokio::test]
    async fn applies_client_xml_text_updates_without_parsing_slate() {
        let hub = CollabHub::default();
        let room = hub.room("doc-1").await;
        let (server_sink, mut client_stream) = test_channel(8);
        let (mut client_sink, server_stream) = test_channel(8);
        let subscription = room
            .broadcast
            .subscribe(Arc::new(Mutex::new(server_sink)), server_stream);

        let update = vec![
            1, 1, 7, 0, 4, 1, 7, 99, 111, 110, 116, 101, 110, 116, 5, 104, 101, 108, 108, 111, 0,
        ];
        client_sink
            .send(Message::Sync(SyncMessage::Update(update)).encode_v1())
            .await
            .unwrap();

        let broadcast = client_stream.next().await.unwrap().unwrap();
        let message = Message::decode_v1(&broadcast).unwrap();
        assert!(matches!(message, Message::Sync(SyncMessage::Update(_))));
        assert_eq!(room.content_text().await.as_deref(), Some("hello"));

        drop(client_sink);
        subscription.completed().await.unwrap();
    }

    fn test_channel(
        capacity: usize,
    ) -> (
        TestSink,
        impl Stream<Item = Result<Vec<u8>, Error>> + Send + Sync + Unpin + 'static,
    ) {
        let (tx, rx) = mpsc::channel(capacity);
        (TestSink { tx }, ReceiverStream { inner: rx })
    }

    #[derive(Debug)]
    struct TestSink {
        tx: mpsc::Sender<Vec<u8>>,
    }

    impl Sink<Vec<u8>> for TestSink {
        type Error = Error;

        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
            self.tx
                .try_send(item)
                .map_err(|error| Error::Other(error.into()))
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    struct ReceiverStream {
        inner: mpsc::Receiver<Vec<u8>>,
    }

    impl Stream for ReceiverStream {
        type Item = Result<Vec<u8>, Error>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            match ready!(self.inner.poll_recv(cx)) {
                None => Poll::Ready(None),
                Some(value) => Poll::Ready(Some(Ok(value))),
            }
        }
    }
}
