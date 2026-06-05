use axum::body::Bytes;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
#[cfg(test)]
use quarry_collab_codec::block_markdown_to_slate;
use quarry_collab_codec::{
    apply_built, build_nodes, encode_update_v1_from_built, review_blocks_to_slate,
    review_markdown_to_slate, strip_trailing_empty_paragraphs, xmltext_to_slate, BuiltNode, Node,
};
use quarry_storage::{CollabDocumentSeed, QuarryStore};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::select;
use tokio::sync::broadcast::{channel, Receiver, Sender};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio::sync::{watch, Mutex, OwnedRwLockWriteGuard, RwLock};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use yrs::encoding::write::Write;
use yrs::sync::protocol::{MSG_SYNC, MSG_SYNC_UPDATE};
use yrs::sync::{Awareness, DefaultProtocol, Error, Message, Protocol, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::{Encode, Encoder, EncoderV1};
#[cfg(test)]
use yrs::{Any, GetString};
use yrs::{
    Doc, Map, OffsetKind, Options, ReadTxn, StateVector, Text, Transact, Update, WriteTxn,
    XmlTextRef,
};

pub(crate) const SHARED_ROOT: &str = "content";
pub(crate) const INJECTION_ROOT: &str = "__quarry_injection";
const RECOVERY_PERSIST_DEBOUNCE: Duration = Duration::from_millis(50);
const INJECTION_ORIGIN: &str = "quarry:agent-injection";
const SERVER_SEED_ORIGIN: &str = "quarry:server-seed";

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

    pub(crate) async fn live_room(&self, document_id: &str) -> Option<Arc<CollabRoom>> {
        self.rooms.read().await.get(document_id).cloned()
    }

    #[cfg(test)]
    pub(crate) async fn room_count(&self) -> usize {
        self.rooms.read().await.len()
    }
}

pub(crate) struct CollabRoom {
    document_id: String,
    store: Option<QuarryStore>,
    broadcast: BroadcastGroup,
    recovery_state: Arc<RoomRecoveryState>,
}

impl CollabRoom {
    async fn new(document_id: &str, store: Option<QuarryStore>) -> Self {
        let initial_state = initial_room_state(store.as_ref(), document_id).await;
        let doc = Doc::with_options(Options {
            offset_kind: OffsetKind::Utf16,
            ..Default::default()
        });
        {
            let mut txn = doc.transact_mut();
            // Yjs root Y.Text and Y.XmlText share wire updates; yrs exposes root creation as TextRef.
            txn.get_or_insert_text(SHARED_ROOT);
            if let Some(update_v1) = &initial_state.update_v1 {
                match Update::decode_v1(update_v1) {
                    Ok(update) => {
                        if let Err(error) = txn.apply_update(update) {
                            tracing::warn!(%error, %document_id, "failed to apply initial collab state");
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, %document_id, "failed to decode initial collab state");
                    }
                }
            }
        }
        let awareness = Arc::new(RwLock::new(Awareness::new(doc)));
        let recovery_state = Arc::new(RoomRecoveryState::new(
            initial_state.base_version_id,
            initial_state.dirty,
        ));
        let persistence = store.clone().map(|store| RecoveryPersistence {
            store,
            document_id: document_id.to_string(),
            debounce: RECOVERY_PERSIST_DEBOUNCE,
            recovery_state: recovery_state.clone(),
        });

        Self {
            document_id: document_id.to_string(),
            store,
            broadcast: BroadcastGroup::new(awareness, 32, persistence).await,
            recovery_state,
        }
    }

    async fn serve_socket(&self, socket: WebSocket) {
        self.reseed_clean_room_if_head_changed().await;
        let (sink, stream) = socket.split();
        let sink = Arc::new(Mutex::new(AxumSink::from(sink)));
        let stream = AxumStream::from(stream);
        let subscription = self.broadcast.subscribe(sink, stream);

        if let Err(error) = subscription.completed().await {
            tracing::debug!(%error, "collab websocket closed with protocol error");
        }
    }

    async fn reseed_clean_room_if_head_changed(&self) {
        if self.recovery_state.is_dirty() {
            return;
        }
        let current_base = self.recovery_state.base_version_id();
        let Some(document_seed) =
            load_collab_document_seed(self.store.as_ref(), &self.document_id).await
        else {
            return;
        };
        if current_base.as_deref() == Some(document_seed.head_version_id.as_str()) {
            return;
        }
        let Some(seed) = clean_seed_update_from_document_seed(
            self.store.as_ref(),
            &self.document_id,
            document_seed,
        )
        .await
        else {
            return;
        };

        {
            let awareness = self.broadcast.awareness().write().await;
            let mut txn = awareness.doc().transact_mut_with(SERVER_SEED_ORIGIN);
            clear_shared_content(&mut txn);
            match Update::decode_v1(&seed.update_v1) {
                Ok(update) => {
                    if let Err(error) = txn.apply_update(update) {
                        tracing::warn!(
                            %error,
                            document_id = %self.document_id,
                            "failed to apply clean collab reseed update"
                        );
                        return;
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        document_id = %self.document_id,
                        "failed to decode clean collab reseed update"
                    );
                    return;
                }
            }
        }
        self.recovery_state.mark_clean(seed.base_version_id);
    }

    #[cfg(test)]
    async fn content_text(&self) -> Option<String> {
        let awareness = self.broadcast.awareness().read().await;
        let txn = awareness.doc().transact();
        txn.get_text(SHARED_ROOT)
            .map(|content| content.get_string(&txn))
    }

    #[cfg(test)]
    async fn injection_envelope(&self) -> HashMap<String, String> {
        let awareness = self.broadcast.awareness().read().await;
        let txn = awareness.doc().transact();
        let Some(envelope) = txn.get_map(INJECTION_ROOT) else {
            return HashMap::new();
        };
        envelope
            .iter(&txn)
            .filter_map(|(key, value)| match value {
                yrs::Out::Any(Any::String(value)) => Some((key.to_string(), value.to_string())),
                _ => None,
            })
            .collect()
    }

    pub(crate) async fn begin_live_mutation(
        &self,
        mutation: LiveMutation,
        original_blocks: &[String],
        _base_version_id: String,
    ) -> Option<InjectionGuard> {
        // Reproduce the live room's nodes from the original blocks, including
        // CriticMarkup comment/suggestion marks; the trailing endmatter block
        // yields no nodes. Bails (→ no injection) if any block can't be matched.
        let expected: Vec<Node> = match review_blocks_to_slate(original_blocks) {
            Ok(nodes) => nodes.into_iter().flatten().collect(),
            Err(error) => {
                tracing::debug!(
                    document_id = %self.document_id,
                    reason = %error,
                    "agent injection gate rejected original blocks because they are not codec eligible"
                );
                return None;
            }
        };
        let expected = match build_nodes(&expected) {
            Ok(nodes) => clean_gate_nodes(&nodes),
            Err(error) => {
                tracing::debug!(
                    document_id = %self.document_id,
                    reason = %error,
                    "agent injection gate rejected original blocks because Yjs build failed"
                );
                return None;
            }
        };

        let awareness = self.broadcast.awareness().clone().write_owned().await;
        let live_content_len = {
            let txn = awareness.doc().transact();
            let root = root_xml_text(&txn)?;
            let live = xmltext_to_slate(&txn, &root).ok()?;
            let Node::Element { children, .. } = live else {
                return None;
            };
            let comparable = clean_gate_nodes(&children);
            let stripped = strip_trailing_empty_paragraphs(&comparable);
            if stripped != expected {
                tracing::debug!(
                    document_id = %self.document_id,
                    live = %serde_json::to_string(&stripped).unwrap_or_default(),
                    expected = %serde_json::to_string(&expected).unwrap_or_default(),
                    "agent injection clean gate rejected live room"
                );
                return None;
            }
            stripped.len() as u32
        };
        if let Some(batch) = &mutation.batch {
            if !batch.is_valid_for(live_content_len) {
                tracing::debug!(
                    document_id = %self.document_id,
                    live_content_len,
                    "agent injection batch rejected for live content length"
                );
                return None;
            }
        }
        if mutation.batch.is_none() && mutation.review.is_none() {
            tracing::debug!(
                document_id = %self.document_id,
                "agent injection rejected empty live mutation"
            );
            return None;
        }

        Some(InjectionGuard {
            awareness,
            document_id: self.document_id.clone(),
            mutation,
            persistence_failed: self.broadcast.persistence_failed.clone(),
            persistence_failure: self.broadcast.persistence_failure.clone(),
            recovery_state: self.recovery_state.clone(),
            store: self.store.clone(),
        })
    }
}

struct InitialRoomState {
    update_v1: Option<Vec<u8>>,
    base_version_id: Option<String>,
    dirty: bool,
}

#[derive(Debug)]
struct RoomRecoveryState {
    base_version_id: StdMutex<Option<String>>,
    dirty: AtomicBool,
}

impl RoomRecoveryState {
    fn new(base_version_id: Option<String>, dirty: bool) -> Self {
        Self {
            base_version_id: StdMutex::new(base_version_id),
            dirty: AtomicBool::new(dirty),
        }
    }

    fn base_version_id(&self) -> Option<String> {
        self.base_version_id.lock().unwrap().clone()
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    fn mark_clean(&self, base_version_id: String) {
        *self.base_version_id.lock().unwrap() = Some(base_version_id);
        self.dirty.store(false, Ordering::SeqCst);
    }

    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::SeqCst)
    }
}

async fn initial_room_state(store: Option<&QuarryStore>, document_id: &str) -> InitialRoomState {
    if let Some(recovery) = load_recovery_update(store, document_id).await {
        return InitialRoomState {
            update_v1: Some(recovery.update_v1),
            base_version_id: recovery.base_version_id,
            dirty: true,
        };
    }

    if let Some(seed) = clean_seed_update(store, document_id).await {
        return InitialRoomState {
            update_v1: Some(seed.update_v1),
            base_version_id: Some(seed.base_version_id),
            dirty: false,
        };
    }

    InitialRoomState {
        update_v1: None,
        base_version_id: None,
        dirty: false,
    }
}

struct CleanSeedUpdate {
    base_version_id: String,
    update_v1: Vec<u8>,
}

async fn clean_seed_update(
    store: Option<&QuarryStore>,
    document_id: &str,
) -> Option<CleanSeedUpdate> {
    let seed = load_collab_document_seed(store, document_id).await?;
    clean_seed_update_from_document_seed(store, document_id, seed).await
}

async fn load_collab_document_seed(
    store: Option<&QuarryStore>,
    document_id: &str,
) -> Option<CollabDocumentSeed> {
    let store = store?;
    match store.collab_document_seed(document_id).await {
        Ok(Some(seed)) => Some(seed),
        Ok(None) => return None,
        Err(error) => {
            tracing::warn!(%error, %document_id, "failed to load collab document seed");
            return None;
        }
    }
}

async fn clean_seed_update_from_document_seed(
    store: Option<&QuarryStore>,
    document_id: &str,
    seed: CollabDocumentSeed,
) -> Option<CleanSeedUpdate> {
    let store = store?;
    if !is_markdown_content_type(&seed.content_type) {
        tracing::debug!(
            %document_id,
            content_type = %seed.content_type,
            "skipping collab server seed for non-markdown document"
        );
        return None;
    }
    let markdown = match std::str::from_utf8(&seed.content) {
        Ok(markdown) => markdown,
        Err(error) => {
            tracing::warn!(%error, %document_id, "failed to decode markdown collab seed as UTF-8");
            return None;
        }
    };
    let slate = match review_markdown_to_slate(markdown) {
        Ok(slate) => slate,
        Err(error) => {
            tracing::debug!(
                %error,
                %document_id,
                "skipping collab server seed because markdown is not codec eligible"
            );
            return None;
        }
    };
    let built = match build_nodes(&slate) {
        Ok(built) => built,
        Err(error) => {
            tracing::debug!(
                %error,
                %document_id,
                "skipping collab server seed because Slate nodes are not Yjs-buildable"
            );
            return None;
        }
    };
    let update_v1 = encode_update_v1_from_built(&built, SHARED_ROOT);
    if let Err(error) = store
        .put_collab_recovery_state(
            document_id,
            Some(seed.head_version_id.clone()),
            update_v1.clone(),
            false,
        )
        .await
    {
        tracing::warn!(%error, %document_id, "failed to persist clean collab server seed");
    }
    Some(CleanSeedUpdate {
        base_version_id: seed.head_version_id,
        update_v1,
    })
}

fn is_markdown_content_type(content_type: &str) -> bool {
    matches!(
        content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "text/markdown" | "text/x-markdown" | "application/markdown" | "application/x-markdown"
    )
}

#[derive(Clone, Debug)]
pub(crate) struct LiveMutation {
    pub(crate) batch: Option<InjectionBatch>,
    pub(crate) review: Option<JsonValue>,
}

impl LiveMutation {
    pub(crate) fn content(batch: InjectionBatch, review: Option<JsonValue>) -> Self {
        Self {
            batch: Some(batch),
            review,
        }
    }

    pub(crate) fn metadata(review: JsonValue) -> Self {
        Self {
            batch: None,
            review: Some(review),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct InjectionBatch {
    ops: Vec<InjectionOp>,
}

impl InjectionBatch {
    pub(crate) fn new(ops: Vec<InjectionOp>) -> Option<Self> {
        if ops.is_empty() {
            return None;
        }
        Some(Self { ops })
    }

    fn is_valid_for(&self, content_len: u32) -> bool {
        self.ops.iter().all(|op| match op {
            InjectionOp::ReplaceSpan {
                start,
                old_node_count,
                ..
            }
            | InjectionOp::DeleteSpan {
                start,
                old_node_count,
            } => start
                .checked_add(*old_node_count)
                .is_some_and(|end| end <= content_len),
            InjectionOp::InsertAt { index, .. } => *index <= content_len,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) enum InjectionOp {
    ReplaceSpan {
        start: u32,
        old_node_count: u32,
        new_nodes: Vec<BuiltNode>,
    },
    InsertAt {
        index: u32,
        new_nodes: Vec<BuiltNode>,
    },
    DeleteSpan {
        start: u32,
        old_node_count: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommitOutcome {
    Injected,
    InjectedRecoveryDegraded,
}

pub(crate) struct InjectionGuard {
    awareness: OwnedRwLockWriteGuard<Awareness>,
    document_id: String,
    mutation: LiveMutation,
    persistence_failed: Arc<AtomicBool>,
    persistence_failure: watch::Sender<Option<String>>,
    recovery_state: Arc<RoomRecoveryState>,
    store: Option<QuarryStore>,
}

impl InjectionGuard {
    pub(crate) async fn commit(mut self, new_version_id: String) -> CommitOutcome {
        {
            let mut txn = self.awareness.doc().transact_mut_with(INJECTION_ORIGIN);
            let root = root_xml_text_mut(&mut txn).expect("collab root must exist");
            if let Some(batch) = &self.mutation.batch {
                apply_injection_ops(&mut txn, &root, &batch.ops);
            }
            let envelope = txn.get_or_insert_map(INJECTION_ROOT);
            envelope.insert(&mut txn, "version_id", new_version_id.clone());
            envelope.insert(&mut txn, "etag", format!("\"{new_version_id}\""));
            if let Some(review) = &self.mutation.review {
                if let Ok(review_json) = serde_json::to_string(review) {
                    envelope.insert(&mut txn, "review", review_json);
                } else {
                    let _ = envelope.remove(&mut txn, "review");
                }
            } else {
                let _ = envelope.remove(&mut txn, "review");
            }
        }

        let Some(store) = self.store.clone() else {
            self.recovery_state.mark_clean(new_version_id);
            return CommitOutcome::Injected;
        };
        let update_v1 = self
            .awareness
            .doc()
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        match store
            .put_collab_recovery_state(&self.document_id, Some(new_version_id), update_v1, false)
            .await
        {
            Ok(state) => {
                if let Some(base_version_id) = state.base_version_id {
                    self.recovery_state.mark_clean(base_version_id);
                }
                CommitOutcome::Injected
            }
            Err(error) => {
                let message = format!("failed to persist collab recovery state: {error}");
                tracing::warn!(
                    %error,
                    document_id = %self.document_id,
                    "failed to persist injected collab recovery state"
                );
                self.persistence_failed.store(true, Ordering::SeqCst);
                signal_recovery_persistence_error_locked(
                    &mut self.awareness,
                    &self.document_id,
                    &message,
                );
                let _ = self.persistence_failure.send(Some(message));
                CommitOutcome::InjectedRecoveryDegraded
            }
        }
    }
}

fn apply_injection_ops(txn: &mut yrs::TransactionMut<'_>, root: &XmlTextRef, ops: &[InjectionOp]) {
    let mut ops = ops
        .iter()
        .cloned()
        .enumerate()
        .collect::<Vec<(usize, InjectionOp)>>();
    ops.sort_by(|(left_order, left), (right_order, right)| {
        right
            .start_index()
            .cmp(&left.start_index())
            .then_with(|| right_order.cmp(left_order))
    });
    for (_, op) in ops {
        match op {
            InjectionOp::ReplaceSpan {
                start,
                old_node_count,
                new_nodes,
            } => {
                if old_node_count > 0 {
                    root.remove_range(txn, start, old_node_count);
                }
                apply_built(txn, root, start, &new_nodes);
            }
            InjectionOp::InsertAt { index, new_nodes } => {
                apply_built(txn, root, index, &new_nodes);
            }
            InjectionOp::DeleteSpan {
                start,
                old_node_count,
            } => {
                if old_node_count > 0 {
                    root.remove_range(txn, start, old_node_count);
                }
            }
        }
    }
}

impl InjectionOp {
    fn start_index(&self) -> u32 {
        match self {
            InjectionOp::ReplaceSpan { start, .. } | InjectionOp::DeleteSpan { start, .. } => {
                *start
            }
            InjectionOp::InsertAt { index, .. } => *index,
        }
    }
}

fn root_xml_text<T: ReadTxn>(txn: &T) -> Option<XmlTextRef> {
    let text = txn.get_text(SHARED_ROOT)?;
    let root: &XmlTextRef = text.as_ref();
    Some(root.clone())
}

fn root_xml_text_mut(txn: &mut yrs::TransactionMut<'_>) -> Option<XmlTextRef> {
    let text = txn.get_text(SHARED_ROOT)?;
    let root: &XmlTextRef = text.as_ref();
    Some(root.clone())
}

fn clear_shared_content(txn: &mut yrs::TransactionMut<'_>) {
    if let Some(root) = root_xml_text_mut(txn) {
        let len = root.len(txn);
        if len > 0 {
            root.remove_range(txn, 0, len);
        }
    }
    if let Some(envelope) = txn.get_map(INJECTION_ROOT) {
        let keys = envelope
            .iter(txn)
            .map(|(key, _)| key.to_string())
            .collect::<Vec<_>>();
        for key in keys {
            let _ = envelope.remove(txn, key.as_str());
        }
    }
}

fn adopt_clean_duplicate_seed_update(
    awareness: &mut Awareness,
    recovery_state: &RoomRecoveryState,
    payload: &[u8],
    suppress_next_seed_broadcast: &AtomicBool,
) -> Option<Message> {
    if recovery_state.is_dirty() {
        return None;
    }
    let Ok(Message::Sync(SyncMessage::Update(update_v1))) = Message::decode_v1(payload) else {
        return None;
    };
    let Some(current) = slate_children_from_doc(awareness.doc()) else {
        return None;
    };
    let incoming = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    });
    {
        let mut txn = incoming.transact_mut();
        txn.get_or_insert_text(SHARED_ROOT);
        let Ok(update) = Update::decode_v1(&update_v1) else {
            return None;
        };
        if txn.apply_update(update).is_err() {
            return None;
        }
    }
    let Some(incoming) = slate_children_from_doc(&incoming) else {
        return None;
    };
    if comparable_seed_nodes(&incoming) != comparable_seed_nodes(&current) {
        return None;
    }

    suppress_next_seed_broadcast.store(true, Ordering::SeqCst);
    {
        let mut txn = awareness.doc().transact_mut_with(SERVER_SEED_ORIGIN);
        clear_shared_content(&mut txn);
        let Ok(update) = Update::decode_v1(&update_v1) else {
            suppress_next_seed_broadcast.store(false, Ordering::SeqCst);
            return None;
        };
        if let Err(error) = txn.apply_update(update) {
            suppress_next_seed_broadcast.store(false, Ordering::SeqCst);
            tracing::warn!(%error, "failed to adopt clean duplicate collab seed update");
            return None;
        }
    }
    if suppress_next_seed_broadcast.load(Ordering::SeqCst) {
        suppress_next_seed_broadcast.store(false, Ordering::SeqCst);
    }
    Some(Message::Sync(SyncMessage::Update(update_v1)))
}

fn slate_children_from_doc(doc: &Doc) -> Option<Vec<Node>> {
    let txn = doc.transact();
    let root = root_xml_text(&txn)?;
    let Node::Element { children, .. } = xmltext_to_slate(&txn, &root).ok()? else {
        return None;
    };
    Some(children)
}

fn comparable_seed_nodes(nodes: &[Node]) -> Vec<Node> {
    strip_trailing_empty_paragraphs(&clean_gate_nodes(nodes))
}

fn clean_gate_nodes(nodes: &[Node]) -> Vec<Node> {
    nodes.iter().map(clean_gate_node).collect()
}

fn clean_gate_node(node: &Node) -> Node {
    match node {
        Node::Text { text, marks } => Node::Text {
            text: text.clone(),
            marks: marks.clone(),
        },
        Node::Element {
            ty,
            attrs,
            children,
        } => {
            let mut attrs = attrs.clone();
            attrs.shift_remove("id");
            Node::element(ty.clone(), attrs, clean_gate_nodes(children))
        }
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
    recovery_state: Arc<RoomRecoveryState>,
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
    recovery_state: Option<Arc<RoomRecoveryState>>,
    suppress_next_seed_broadcast: Arc<AtomicBool>,
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
        let suppress_next_seed_broadcast = Arc::new(AtomicBool::new(false));
        let (persistence_failure, _persistence_failure_rx) = watch::channel(None);
        let awareness_c = Arc::downgrade(&awareness);
        let recovery_state = persistence
            .as_ref()
            .map(|persistence| persistence.recovery_state.clone());
        let recovery_state_for_doc = recovery_state.clone();
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
        let suppress_seed_broadcast = suppress_next_seed_broadcast.clone();
        let doc_sub = {
            lock.doc()
                .observe_update_v1(move |txn, update| {
                    let server_injection = txn
                        .origin()
                        .is_some_and(|origin| origin.as_ref() == INJECTION_ORIGIN.as_bytes());
                    let server_seed = txn
                        .origin()
                        .is_some_and(|origin| origin.as_ref() == SERVER_SEED_ORIGIN.as_bytes());
                    if server_seed && suppress_seed_broadcast.swap(false, Ordering::SeqCst) {
                        return;
                    }
                    let mut encoder = EncoderV1::new();
                    encoder.write_var(MSG_SYNC);
                    encoder.write_var(MSG_SYNC_UPDATE);
                    encoder.write_buf(&update.update);
                    let _ = sink.send(encoder.to_vec());
                    if !server_injection && !server_seed {
                        if let Some(recovery_state) = &recovery_state_for_doc {
                            recovery_state.mark_dirty();
                        }
                        if let Some(recovery_tx) = &recovery_tx {
                            let _ = recovery_tx.send(());
                        }
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
            recovery_state,
            suppress_next_seed_broadcast,
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
            let recovery_state = self.recovery_state.clone();
            let suppress_next_seed_broadcast = self.suppress_next_seed_broadcast.clone();
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
                                if let Some(recovery_state) = &recovery_state {
                                    if let Some(reply) = adopt_clean_duplicate_seed_update(
                                        &mut awareness,
                                        recovery_state,
                                        &payload,
                                        &suppress_next_seed_broadcast,
                                    ) {
                                        vec![reply]
                                    } else {
                                        protocol
                                            .handle(&mut awareness, &payload)?
                                            .into_iter()
                                            .collect()
                                    }
                                } else {
                                    protocol
                                        .handle(&mut awareness, &payload)?
                                        .into_iter()
                                        .collect()
                                }
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
        .put_collab_recovery_state(
            &persistence.document_id,
            persistence.recovery_state.base_version_id(),
            update_v1,
            true,
        )
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
    signal_recovery_persistence_error_locked(&mut awareness, document_id, message);
}

fn signal_recovery_persistence_error_locked(
    awareness: &mut Awareness,
    document_id: &str,
    message: &str,
) {
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
    async fn injects_agent_edit_into_equal_live_room() {
        let hub = CollabHub::default();
        let room = hub.room("doc-1").await;
        seed_room(&room, "Hello\n").await;
        let batch = InjectionBatch::new(vec![InjectionOp::ReplaceSpan {
            start: 0,
            old_node_count: 1,
            new_nodes: built_nodes("Hi\n"),
        }])
        .unwrap();
        let original_blocks = vec!["Hello\n".to_string()];

        let guard = room
            .begin_live_mutation(
                LiveMutation::content(batch, None),
                &original_blocks,
                "v1".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(
            guard.commit("v2".to_string()).await,
            CommitOutcome::Injected
        );

        assert_eq!(
            room_slate_children(&room).await,
            block_markdown_to_slate("Hi\n").unwrap()
        );
        let envelope = room.injection_envelope().await;
        assert_eq!(envelope.get("version_id").map(String::as_str), Some("v2"));
        assert_eq!(envelope.get("etag").map(String::as_str), Some("\"v2\""));
        assert!(!envelope.contains_key("review"));
    }

    #[tokio::test]
    async fn injection_gate_ignores_plate_runtime_element_ids() {
        let hub = CollabHub::default();
        let room = hub.room("doc-1").await;
        let live_nodes = block_markdown_to_slate("Hello\n\nWorld\n")
            .unwrap()
            .into_iter()
            .enumerate()
            .map(|(index, node)| with_plate_id(node, &format!("runtime-{index}")))
            .collect::<Vec<_>>();
        seed_room_nodes(&room, &build_nodes(&live_nodes).unwrap()).await;
        let batch = InjectionBatch::new(vec![InjectionOp::ReplaceSpan {
            start: 1,
            old_node_count: 1,
            new_nodes: built_nodes("Everyone\n"),
        }])
        .unwrap();
        let original_blocks = vec!["Hello\n\n".to_string(), "World\n".to_string()];

        let guard = room
            .begin_live_mutation(
                LiveMutation::content(batch, None),
                &original_blocks,
                "v1".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(
            guard.commit("v2".to_string()).await,
            CommitOutcome::Injected
        );

        let children = room_slate_children(&room).await;
        assert_eq!(
            children[1],
            block_markdown_to_slate("Everyone\n").unwrap()[0]
        );
    }

    #[tokio::test]
    async fn injection_gate_rejects_untouched_live_block_difference() {
        let hub = CollabHub::default();
        let room = hub.room("doc-1").await;
        seed_room(&room, "Changed\n\nSecond\n").await;
        let batch = InjectionBatch::new(vec![InjectionOp::ReplaceSpan {
            start: 1,
            old_node_count: 1,
            new_nodes: built_nodes("New second\n"),
        }])
        .unwrap();
        let original_blocks = vec!["First\n\n".to_string(), "Second\n".to_string()];

        assert!(room
            .begin_live_mutation(
                LiveMutation::content(batch, None),
                &original_blocks,
                "v1".to_string(),
            )
            .await
            .is_none());
    }

    #[tokio::test]
    async fn injection_gate_accepts_live_room_with_review_comment_marks() {
        let hub = CollabHub::default();
        let room = hub.room("doc-1").await;
        // Seed the live room exactly as the browser would from a commented doc:
        // the comment becomes `comment`/`comment_c1` leaf marks.
        let body = "Para with {==quote==}{>>note<<}{#c1} after\n\n";
        let endmatter =
            "---\ncomments:\n  c1:\n    by: ai:codex\n    at: 2026-06-05T02:41:00.480Z\n";
        let doc = format!("{body}{endmatter}");
        let live = quarry_collab_codec::review_markdown_to_slate(&doc).unwrap();
        seed_room_nodes(&room, &build_nodes(&live).unwrap()).await;

        // Replace the commented prose block (1 live node) with plain text.
        let batch = InjectionBatch::new(vec![InjectionOp::ReplaceSpan {
            start: 0,
            old_node_count: 1,
            new_nodes: built_nodes("Replaced\n"),
        }])
        .unwrap();
        let original_blocks = vec![body.to_string(), endmatter.to_string()];

        let guard = room
            .begin_live_mutation(
                LiveMutation::content(
                    batch,
                    Some(serde_json::json!({
                        "comments": {
                            "c1": {
                                "by": "ai:codex",
                                "at": "2026-06-05T02:41:00.480Z",
                                "body": "note"
                            }
                        }
                    })),
                ),
                &original_blocks,
                "v1".to_string(),
            )
            .await
            .expect("review-comment live room should pass the injection gate");
        assert_eq!(
            guard.commit("v2".to_string()).await,
            CommitOutcome::Injected
        );

        assert_eq!(
            room_slate_children(&room).await,
            block_markdown_to_slate("Replaced\n").unwrap()
        );
        let envelope = room.injection_envelope().await;
        assert_eq!(envelope.get("version_id").map(String::as_str), Some("v2"));
        assert_eq!(envelope.get("etag").map(String::as_str), Some("\"v2\""));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                envelope.get("review").expect("review envelope")
            )
            .unwrap()["comments"]["c1"]["body"],
            "note"
        );
    }

    #[tokio::test]
    async fn injection_allows_trailing_scaffold_and_inserts_before_it() {
        let hub = CollabHub::default();
        let room = hub.room("doc-1").await;
        let mut nodes = block_markdown_to_slate("Hello\n").unwrap();
        nodes.push(Node::element(
            "p",
            [("id".to_string(), serde_json::json!("trailing-id"))].into(),
            vec![Node::text("", Default::default())],
        ));
        seed_room_nodes(&room, &build_nodes(&nodes).unwrap()).await;
        let batch = InjectionBatch::new(vec![InjectionOp::InsertAt {
            index: 1,
            new_nodes: built_nodes("Inserted\n"),
        }])
        .unwrap();
        let original_blocks = vec!["Hello\n".to_string()];

        let guard = room
            .begin_live_mutation(
                LiveMutation::content(batch, None),
                &original_blocks,
                "v1".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(
            guard.commit("v2".to_string()).await,
            CommitOutcome::Injected
        );

        let children = room_slate_children(&room).await;
        assert_eq!(
            children[..2].to_vec(),
            block_markdown_to_slate("Hello\n\nInserted\n").unwrap()
        );
        let comparable = clean_gate_nodes(&children);
        assert!(quarry_collab_codec::is_empty_paragraph(&comparable[2]));
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
        replace_room_markdown(&room, "hello\n").await;

        let state = wait_for_recovery_state(&store, &document_id).await;
        assert_eq!(state.document_id, document_id);
        assert_eq!(state.base_version_id, Some(written.version.id));
        assert!(state.dirty);
        assert!(!state.update_v1.is_empty());

        drop(room);
        drop(hub);

        let restored_hub = CollabHub::new(store);
        let restored = restored_hub.room(&document_id).await;
        assert_eq!(
            room_slate_children(&restored).await,
            block_markdown_to_slate("hello\n").unwrap()
        );
    }

    #[tokio::test]
    async fn seeds_clean_room_from_current_markdown_head() {
        let (_root, store, document_id, head_version_id) =
            store_with_markdown_document("collabseed", "# Untitled\n\nDraft\n").await;

        let hub = CollabHub::new(store.clone());
        let room = hub.room(&document_id).await;

        let state =
            wait_for_recovery_state_matching(&store, &document_id, |state| !state.dirty).await;
        assert_eq!(state.document_id, document_id);
        assert_eq!(
            state.base_version_id.as_deref(),
            Some(head_version_id.as_str())
        );
        assert!(!state.update_v1.is_empty());
        assert_eq!(
            room_slate_children(&room).await,
            block_markdown_to_slate("# Untitled\n\nDraft\n").unwrap()
        );
    }

    #[tokio::test]
    async fn initial_client_sync_does_not_mark_seeded_recovery_dirty() {
        let (_root, store, document_id, head_version_id) =
            store_with_markdown_document("collabsync", "# Untitled\n").await;
        let hub = CollabHub::new(store.clone());
        let room = hub.room(&document_id).await;
        let _ = wait_for_recovery_state_matching(&store, &document_id, |state| !state.dirty).await;
        let (server_sink, mut client_stream) = test_channel(8);
        let (mut client_sink, server_stream) = test_channel(8);
        let subscription = room
            .broadcast
            .subscribe(Arc::new(Mutex::new(server_sink)), server_stream);
        let client_doc = Doc::with_options(Options {
            offset_kind: OffsetKind::Utf16,
            ..Default::default()
        });

        client_sink
            .send(
                Message::Sync(SyncMessage::SyncStep1(client_doc.transact().state_vector()))
                    .encode_v1(),
            )
            .await
            .unwrap();

        let reply = client_stream.next().await.unwrap().unwrap();
        assert!(matches!(
            Message::decode_v1(&reply).unwrap(),
            Message::Sync(SyncMessage::SyncStep2(_))
        ));
        sleep(RECOVERY_PERSIST_DEBOUNCE * 2).await;
        let state = store
            .collab_recovery_state(&document_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            state.base_version_id.as_deref(),
            Some(head_version_id.as_str())
        );
        assert!(!state.dirty);

        drop(client_sink);
        subscription.completed().await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_clean_client_seed_update_stays_clean_without_duplicating_content() {
        let markdown = "See {==this==}{>>Check it<<}{#c1}.\n\n---\ncomments:\n  c1:\n    at: \"2026-01-01T00:00:00.000Z\"\n    by: user\n";
        let (_root, store, document_id, head_version_id) =
            store_with_markdown_document("collabdupe", markdown).await;
        let hub = CollabHub::new(store.clone());
        let room = hub.room(&document_id).await;
        let _ = wait_for_recovery_state_matching(&store, &document_id, |state| !state.dirty).await;
        let (server_sink, mut client_stream) = test_channel(8);
        let (mut client_sink, server_stream) = test_channel(8);
        let subscription = room
            .broadcast
            .subscribe(Arc::new(Mutex::new(server_sink)), server_stream);
        let update = encode_update_v1_from_built(
            &build_nodes(&review_markdown_to_slate(markdown).unwrap()).unwrap(),
            SHARED_ROOT,
        );

        client_sink
            .send(Message::Sync(SyncMessage::Update(update)).encode_v1())
            .await
            .unwrap();

        let _ = client_stream.next().await.unwrap().unwrap();
        sleep(RECOVERY_PERSIST_DEBOUNCE * 2).await;
        let state = store
            .collab_recovery_state(&document_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            state.base_version_id.as_deref(),
            Some(head_version_id.as_str())
        );
        assert!(!state.dirty);
        assert_eq!(
            room_slate_children(&room).await,
            review_markdown_to_slate(markdown).unwrap()
        );

        drop(client_sink);
        subscription.completed().await.unwrap();
    }

    #[tokio::test]
    async fn first_real_client_edit_marks_dirty_preserving_original_base() {
        let (_root, store, document_id, original_version_id) =
            store_with_markdown_document("collabdirty", "Original\n").await;
        let hub = CollabHub::new(store.clone());
        let room = hub.room(&document_id).await;
        let _ = wait_for_recovery_state_matching(&store, &document_id, |state| !state.dirty).await;
        let next_version_id = put_markdown(&store, "collabdirty", "live.md", "External\n").await;
        assert_ne!(original_version_id, next_version_id);

        replace_room_markdown(&room, "Client edit\n").await;

        let state = wait_for_recovery_state(&store, &document_id).await;
        assert_eq!(
            state.base_version_id.as_deref(),
            Some(original_version_id.as_str())
        );
        assert!(state.dirty);
        assert_eq!(
            room_slate_children(&room).await,
            block_markdown_to_slate("Client edit\n").unwrap()
        );
    }

    #[tokio::test]
    async fn clean_room_reseeds_from_new_head_before_next_socket_join() {
        let (_root, store, document_id, original_version_id) =
            store_with_markdown_document("collabreseeds", "Original\n").await;
        let hub = CollabHub::new(store.clone());
        let room = hub.room(&document_id).await;
        let _ = wait_for_recovery_state_matching(&store, &document_id, |state| !state.dirty).await;
        let next_version_id =
            put_markdown(&store, "collabreseeds", "live.md", "Manual update\n").await;
        assert_ne!(original_version_id, next_version_id);

        room.reseed_clean_room_if_head_changed().await;

        let state =
            wait_for_recovery_state_matching(&store, &document_id, |state| !state.dirty).await;
        assert_eq!(
            state.base_version_id.as_deref(),
            Some(next_version_id.as_str())
        );
        assert_eq!(
            room_slate_children(&room).await,
            block_markdown_to_slate("Manual update\n").unwrap()
        );
    }

    #[tokio::test]
    async fn dirty_room_does_not_reseed_from_external_head_change() {
        let (_root, store, document_id, original_version_id) =
            store_with_markdown_document("collabdirtyreseed", "Original\n").await;
        let hub = CollabHub::new(store.clone());
        let room = hub.room(&document_id).await;
        let _ = wait_for_recovery_state_matching(&store, &document_id, |state| !state.dirty).await;
        replace_room_markdown(&room, "Client edit\n").await;
        let dirty = wait_for_recovery_state(&store, &document_id).await;
        assert_eq!(
            dirty.base_version_id.as_deref(),
            Some(original_version_id.as_str())
        );
        let _ = put_markdown(&store, "collabdirtyreseed", "live.md", "External update\n").await;

        room.reseed_clean_room_if_head_changed().await;

        let state = wait_for_recovery_state(&store, &document_id).await;
        assert_eq!(
            state.base_version_id.as_deref(),
            Some(original_version_id.as_str())
        );
        assert!(state.dirty);
        assert_eq!(
            room_slate_children(&room).await,
            block_markdown_to_slate("Client edit\n").unwrap()
        );
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
            recovery_state: Arc::new(RoomRecoveryState::new(None, false)),
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

    async fn store_with_markdown_document(
        library_slug: &str,
        markdown: &str,
    ) -> (tempfile::TempDir, QuarryStore, String, String) {
        let root = tempfile::tempdir().unwrap();
        let store = QuarryStore::open(StoreConfig {
            db_path: root.path().join("quarry.db"),
            cas_path: root.path().join("cas"),
            lock_path: None,
        })
        .await
        .unwrap();
        let library = store.create_library(library_slug).await.unwrap();
        let written = store
            .put_document(
                &library.slug,
                "live.md",
                markdown.as_bytes().to_vec(),
                serde_json::json!({"content_type":"text/markdown"}),
                "text/markdown",
                DocumentSource::Rest,
                WritePrecondition::None,
            )
            .await
            .unwrap();
        (root, store, written.document.id, written.version.id)
    }

    async fn put_markdown(
        store: &QuarryStore,
        library_slug: &str,
        path: &str,
        markdown: &str,
    ) -> String {
        store
            .put_document(
                library_slug,
                path,
                markdown.as_bytes().to_vec(),
                serde_json::json!({"content_type":"text/markdown"}),
                "text/markdown",
                DocumentSource::Rest,
                WritePrecondition::None,
            )
            .await
            .unwrap()
            .version
            .id
    }

    async fn wait_for_recovery_state(
        store: &QuarryStore,
        document_id: &str,
    ) -> quarry_storage::CollabRecoveryState {
        wait_for_recovery_state_matching(store, document_id, |state| state.dirty).await
    }

    async fn wait_for_recovery_state_matching(
        store: &QuarryStore,
        document_id: &str,
        matches: impl Fn(&quarry_storage::CollabRecoveryState) -> bool,
    ) -> quarry_storage::CollabRecoveryState {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(state) = store.collab_recovery_state(document_id).await.unwrap() {
                    if matches(&state) {
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

    fn built_nodes(markdown: &str) -> Vec<BuiltNode> {
        build_nodes(&block_markdown_to_slate(markdown).unwrap()).unwrap()
    }

    fn with_plate_id(mut node: Node, id: &str) -> Node {
        if let Node::Element { attrs, .. } = &mut node {
            attrs.insert("id".to_string(), serde_json::json!(id));
        }
        node
    }

    async fn seed_room(room: &CollabRoom, markdown: &str) {
        seed_room_nodes(room, &built_nodes(markdown)).await;
    }

    async fn seed_room_nodes(room: &CollabRoom, nodes: &[BuiltNode]) {
        let awareness = room.broadcast.awareness().write().await;
        let mut txn = awareness.doc().transact_mut();
        let root = root_xml_text_mut(&mut txn).unwrap();
        apply_built(&mut txn, &root, 0, nodes);
    }

    async fn replace_room_markdown(room: &CollabRoom, markdown: &str) {
        let awareness = room.broadcast.awareness().write().await;
        let mut txn = awareness.doc().transact_mut();
        let root = root_xml_text_mut(&mut txn).unwrap();
        let len = root.len(&txn);
        if len > 0 {
            root.remove_range(&mut txn, 0, len);
        }
        apply_built(&mut txn, &root, 0, &built_nodes(markdown));
    }

    async fn room_slate_children(room: &CollabRoom) -> Vec<Node> {
        let awareness = room.broadcast.awareness().read().await;
        let txn = awareness.doc().transact();
        let root = root_xml_text(&txn).unwrap();
        let Node::Element { children, .. } = xmltext_to_slate(&txn, &root).unwrap() else {
            panic!("expected fragment");
        };
        children
    }
}
