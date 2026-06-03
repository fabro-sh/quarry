use axum::body::Bytes;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use quarry_storage::QuarryStore;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::select;
use tokio::sync::broadcast::{channel, Receiver, Sender};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use yrs::encoding::write::Write;
use yrs::sync::protocol::{MSG_SYNC, MSG_SYNC_UPDATE};
use yrs::sync::{Awareness, DefaultProtocol, Error, Message, Protocol};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::{Encode, Encoder, EncoderV1};
#[cfg(test)]
use yrs::GetString;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update, WriteTxn};

pub(crate) const SHARED_ROOT: &str = "content";
const RECOVERY_PERSIST_DEBOUNCE: Duration = Duration::from_millis(50);

type AwarenessRef = Arc<RwLock<Awareness>>;

#[derive(Clone, Default)]
pub(crate) struct CollabHub {
    rooms: Arc<RwLock<HashMap<String, Arc<CollabRoom>>>>,
    store: Option<QuarryStore>,
}

impl CollabHub {
    pub(crate) fn new(store: QuarryStore) -> Self {
        Self {
            rooms: Arc::default(),
            store: Some(store),
        }
    }

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

        let room = Arc::new(CollabRoom::new(document_id, self.store.clone()).await);
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
    async fn new(document_id: &str, store: Option<QuarryStore>) -> Self {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            // Yjs root Y.Text and Y.XmlText share wire updates; yrs exposes root creation as TextRef.
            txn.get_or_insert_text(SHARED_ROOT);
            if let Some(recovery) = load_recovery_update(store.as_ref(), document_id).await {
                match Update::decode_v1(&recovery.update_v1) {
                    Ok(update) => {
                        if let Err(error) = txn.apply_update(update) {
                            tracing::warn!(%error, %document_id, "failed to apply collab recovery state");
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, %document_id, "failed to decode collab recovery state");
                    }
                }
            }
        }
        let awareness = Arc::new(RwLock::new(Awareness::new(doc)));
        let persistence = store.map(|store| RecoveryPersistence {
            store,
            document_id: document_id.to_string(),
            debounce: RECOVERY_PERSIST_DEBOUNCE,
        });

        Self {
            broadcast: BroadcastGroup::new(awareness, 32, persistence).await,
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

async fn load_recovery_update(
    store: Option<&QuarryStore>,
    document_id: &str,
) -> Option<quarry_storage::CollabRecoveryState> {
    let store = store?;
    match store.collab_recovery_state(document_id).await {
        Ok(Some(state)) if state.dirty && !state.update_v1.is_empty() => Some(state),
        Ok(_) => None,
        Err(error) => {
            tracing::warn!(%error, %document_id, "failed to load collab recovery state");
            None
        }
    }
}

#[derive(Clone)]
struct RecoveryPersistence {
    store: QuarryStore,
    document_id: String,
    debounce: Duration,
}

struct BroadcastGroup {
    _awareness_sub: yrs::Subscription,
    _doc_sub: yrs::Subscription,
    awareness_ref: AwarenessRef,
    sender: Sender<Vec<u8>>,
    _receiver: Receiver<Vec<u8>>,
    awareness_updater: JoinHandle<()>,
    persistence_failed: Arc<AtomicBool>,
    persistence_failure: watch::Sender<Option<String>>,
    recovery_persister: Option<JoinHandle<()>>,
}

unsafe impl Send for BroadcastGroup {}
unsafe impl Sync for BroadcastGroup {}

impl BroadcastGroup {
    async fn new(
        awareness: AwarenessRef,
        buffer_capacity: usize,
        persistence: Option<RecoveryPersistence>,
    ) -> Self {
        let (sender, receiver) = channel(buffer_capacity);
        let persistence_failed = Arc::new(AtomicBool::new(false));
        let (persistence_failure, _persistence_failure_rx) = watch::channel(None);
        let awareness_c = Arc::downgrade(&awareness);
        let (recovery_tx, recovery_persister) = persistence
            .map(|persistence| {
                let (tx, rx) = unbounded_channel();
                (
                    Some(tx),
                    Some(spawn_recovery_persister(
                        Arc::downgrade(&awareness),
                        persistence,
                        rx,
                        persistence_failed.clone(),
                        persistence_failure.clone(),
                    )),
                )
            })
            .unwrap_or((None, None));

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
                    if let Some(recovery_tx) = &recovery_tx {
                        let _ = recovery_tx.send(());
                    }
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
            persistence_failed,
            persistence_failure,
            recovery_persister,
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
            let mut failure = self.persistence_failure.subscribe();
            tokio::spawn(async move {
                loop {
                    select! {
                        changed = failure.changed() => {
                            if changed.is_err() {
                                return Ok(());
                            }
                            if let Some(message) = failure.borrow().clone() {
                                return Err(collab_persistence_error(message));
                            }
                        }
                        message = receiver.recv() => {
                            let Ok(msg) = message else {
                                return Ok(());
                            };
                            let mut sink = sink.lock().await;
                            if let Err(error) = sink.send(msg).await {
                                return Err(Error::Other(Box::new(error)));
                            }
                        }
                    }
                }
            })
        };

        let stream_task = {
            let awareness = self.awareness().clone();
            let persistence_failed = self.persistence_failed.clone();
            let mut failure = self.persistence_failure.subscribe();
            tokio::spawn(async move {
                loop {
                    select! {
                        changed = failure.changed() => {
                            if changed.is_err() {
                                return Ok(());
                            }
                            if let Some(message) = failure.borrow().clone() {
                                return Err(collab_persistence_error(message));
                            }
                        }
                        result = stream.next() => {
                            let Some(result) = result else {
                                return Ok(());
                            };
                            if persistence_failed.load(Ordering::SeqCst) {
                                return Err(collab_persistence_error(
                                    "collab recovery persistence failed".to_string(),
                                ));
                            }
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
                    }
                }
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
        if let Some(task) = &self.recovery_persister {
            task.abort();
        }
    }
}

fn spawn_recovery_persister(
    awareness: Weak<RwLock<Awareness>>,
    persistence: RecoveryPersistence,
    mut rx: UnboundedReceiver<()>,
    persistence_failed: Arc<AtomicBool>,
    persistence_failure: watch::Sender<Option<String>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            loop {
                match timeout(persistence.debounce, rx.recv()).await {
                    Ok(Some(_)) => continue,
                    Ok(None) => {
                        persist_recovery_snapshot(
                            &awareness,
                            &persistence,
                            &persistence_failed,
                            &persistence_failure,
                        )
                        .await;
                        return;
                    }
                    Err(_) => break,
                }
            }
            if !persist_recovery_snapshot(
                &awareness,
                &persistence,
                &persistence_failed,
                &persistence_failure,
            )
            .await
            {
                return;
            }
        }
    })
}

async fn persist_recovery_snapshot(
    awareness: &Weak<RwLock<Awareness>>,
    persistence: &RecoveryPersistence,
    persistence_failed: &AtomicBool,
    persistence_failure: &watch::Sender<Option<String>>,
) -> bool {
    let Some(awareness) = awareness.upgrade() else {
        return false;
    };
    let update_v1 = {
        let awareness = awareness.read().await;
        let update = awareness
            .doc()
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        update
    };
    if let Err(error) = persistence
        .store
        .put_collab_recovery_state(&persistence.document_id, None, update_v1, true)
        .await
    {
        let message = format!("failed to persist collab recovery state: {error}");
        tracing::warn!(
            %error,
            document_id = %persistence.document_id,
            "failed to persist collab recovery state"
        );
        persistence_failed.store(true, Ordering::SeqCst);
        signal_recovery_persistence_error(&awareness, &persistence.document_id, &message).await;
        let _ = persistence_failure.send(Some(message));
        return false;
    }
    true
}

async fn signal_recovery_persistence_error(
    awareness: &RwLock<Awareness>,
    document_id: &str,
    message: &str,
) {
    let mut awareness = awareness.write().await;
    let state = serde_json::json!({
        "quarryServer": {
            "recoveryError": {
                "documentId": document_id,
                "message": message,
            }
        }
    });
    if let Err(error) = awareness.set_local_state(state) {
        tracing::warn!(
            %error,
            %document_id,
            "failed to broadcast collab recovery persistence error"
        );
    }
}

fn collab_persistence_error(message: String) -> Error {
    Error::Other(Box::new(std::io::Error::other(message)))
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
    use quarry_core::{DocumentSource, WritePrecondition};
    use quarry_storage::{QuarryStore, StoreConfig};
    use std::task::{Context, Poll};
    use tokio::sync::mpsc;
    use tokio::time::{sleep, Duration};
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

    #[tokio::test]
    async fn persists_and_loads_recovery_state_by_document_id() {
        let root = tempfile::tempdir().unwrap();
        let store = QuarryStore::open(StoreConfig {
            db_path: root.path().join("quarry.db"),
            cas_path: root.path().join("cas"),
            lock_path: None,
        })
        .await
        .unwrap();
        let library = store.create_library("collab").await.unwrap();
        let written = store
            .put_document(
                &library.slug,
                "live.md",
                b"markdown".to_vec(),
                serde_json::json!({"content_type":"text/markdown"}),
                "text/markdown",
                DocumentSource::Rest,
                WritePrecondition::None,
            )
            .await
            .unwrap();
        let document_id = written.document.id.clone();

        let hub = CollabHub::new(store.clone());
        let room = hub.room(&document_id).await;
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
        let _ = client_stream.next().await.unwrap().unwrap();

        let state = wait_for_recovery_state(&store, &document_id).await;
        assert_eq!(state.document_id, document_id);
        assert_eq!(state.base_version_id, Some(written.version.id));
        assert!(state.dirty);
        assert!(!state.update_v1.is_empty());

        drop(client_sink);
        subscription.completed().await.unwrap();
        drop(room);
        drop(hub);

        let restored_hub = CollabHub::new(store);
        let restored = restored_hub.room(&document_id).await;
        assert_eq!(restored.content_text().await.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn signals_recovery_persistence_failures_to_peers() {
        let root = tempfile::tempdir().unwrap();
        let store = QuarryStore::open(StoreConfig {
            db_path: root.path().join("quarry.db"),
            cas_path: root.path().join("cas"),
            lock_path: None,
        })
        .await
        .unwrap();
        let awareness = Arc::new(RwLock::new(Awareness::new(Doc::new())));
        let persistence = RecoveryPersistence {
            store,
            document_id: "missing-document".to_string(),
            debounce: Duration::from_millis(1),
        };
        let failed = AtomicBool::new(false);
        let (failure_tx, failure_rx) = watch::channel(None);

        assert!(
            !persist_recovery_snapshot(
                &Arc::downgrade(&awareness),
                &persistence,
                &failed,
                &failure_tx,
            )
            .await
        );

        assert!(failed.load(Ordering::SeqCst));
        assert!(failure_rx
            .borrow()
            .as_deref()
            .unwrap()
            .contains("failed to persist collab recovery state"));
        let state = awareness.read().await.local_state_raw().unwrap();
        assert!(state.contains("quarryServer"));
        assert!(state.contains("missing-document"));
    }

    async fn wait_for_recovery_state(
        store: &QuarryStore,
        document_id: &str,
    ) -> quarry_storage::CollabRecoveryState {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(state) = store.collab_recovery_state(document_id).await.unwrap() {
                    if state.dirty {
                        return state;
                    }
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
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
