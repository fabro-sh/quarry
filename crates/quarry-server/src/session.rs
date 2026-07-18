//! Ephemeral per-document editing sessions — Phase 3 of the session-scoped
//! collaboration rewrite.
//!
//! ## Lifecycle
//!
//! A session is an in-memory Yjs document that exists only while websocket
//! subscribers are connected:
//!
//! - The FIRST subscriber seeds a fresh doc from canonical block rows (and
//!   review anchors, overlaid as the browser's comment/suggestion marks —
//!   see the `quarry_collab_codec` session projection exports). If the stored projection is
//!   missing, rows are materialized in-memory from the head Markdown; the
//!   first checkpoint persists them.
//! - Updates broadcast to peers over the y-sync v1 protocol; awareness is
//!   relayed and never persisted.
//! - A debounced checkpoint ([`CHECKPOINT_DEBOUNCE`] after the last update)
//!   projects the session doc back to rows and commits ONE version + ONE
//!   coalesced `browser_session` history row through
//!   [`QuarryStore::commit_block_mutation`]. The checkpoint is the only
//!   durable effect of typing.
//! - The LAST subscriber leaving runs a final checkpoint and discards the
//!   doc. Nothing CRDT-shaped is ever stored.
//!
//! ## Checkpoint acks (the browser's save state)
//!
//! Every durable commit of the session doc — debounced checkpoint, final
//! checkpoint, or session-mode gateway transaction — broadcasts a
//! [`MSG_QUARRY_CHECKPOINT`] frame carrying the committed doc state as a
//! v1-encoded Yjs snapshot, and each new subscriber receives the current one
//! on join. The browser compares the acked snapshot against its local doc:
//! equality means everything it displays is canonical (`Saved`); anything
//! beyond the ack means a commit is still owed (`Saving…`). The snapshot is
//! captured under the doc write lock at projection/commit time, so the ack
//! can never claim coverage of state the commit did not contain — and a
//! persistently failing checkpoint simply never acks, which the browser
//! reports honestly instead of pretending the content is safe. The browser
//! half lives in `ui/src/features/collab/rust-ws-provider.ts`.
//!
//! ## The mode switch
//!
//! [`SessionHub::lock_document`] hands out the per-document async mutex that
//! serializes seed, checkpoint, discard, and semantic transactions. The
//! gateway locks it, asks for the live session, and dispatches: no session →
//! rows mode (plain SQL); session → the transaction applies into the live
//! doc as a collaborator and forces a checkpoint before acking (see
//! `gateway::apply_session_transaction`). Writers arriving mid-transition
//! wait on the mutex; they are never rejected because a session exists.
//!
//! ## Event provenance
//!
//! Checkpoint and session-mode transaction commits emit `doc.changed`
//! events with an `agent-injected:…` `origin_id`. Session participants
//! already carry that state in their live doc, so the browser classifies
//! these as benign metadata refreshes (`session-events.ts`) rather than
//! external changes. (The Phase 3 PUT-as-checkpoint autosave rule that also
//! relied on this dissolved with the Phase 5 browser: a browser-origin
//! Markdown PUT is now an ordinary whole-file write through the Phase 4
//! reconciler, like any other external writer.)
//!
//! ## Known hazards (accepted)
//!
//! - Whole-file writes (Markdown PUT, Git, FUSE, CLI, version restores) do
//!   not race sessions: they route through the gateway dispatch, where they
//!   take this module's document mutex and merge into the live doc as
//!   collaborator edits. Only the legacy non-Markdown byte path and staged
//!   transaction commits remain outside (they clear the projection
//!   fail-closed) — a recorded limitation (see the README).
//! - The collab websocket remains unauthenticated (phase-one loopback
//!   posture, design delta 2); sessions do not widen exposure.
//! - **Persistent checkpoint failure loses the session at discard.** When
//!   the doc→rows projection or the Markdown export of a checkpoint fails,
//!   the commit is skipped with a warn log; the session stays dirty and
//!   retries on the next edit/PUT/transaction, but if the failure persists
//!   until the last subscriber leaves, the final checkpoint fails too and
//!   every un-checkpointed edit is lost with the discarded doc. The known
//!   trigger classes are contained: unknown inline marks are DROPPED at
//!   projection time (`collab.session.unknown_marks_dropped` — they would
//!   otherwise fail the Markdown writer on every retry), and inline
//!   elements the row model cannot represent degrade blocks to
//!   `raw_markdown` rows. Residual triggers (doc shapes the Markdown
//!   writer still rejects) remain possible — checkpoints export through
//!   `block_rows_to_markdown`, a path the Phase 4 reconciler did not
//!   replace — and because a failing SHAPE keeps failing on every retry,
//!   the loss is UNBOUNDED while it persists: every edit made after the
//!   last successful checkpoint dies with the discard. That is why the
//!   projection must always export — the containment rules above exist to
//!   keep every reachable doc shape projectable.

use crate::collab::{SHARED_ROOT, serve_session_socket};
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use quarry_collab_codec::{
    BlockRow, Node, ReviewMeta, ReviewMetaEntry, SessionAnchor, SessionAnchorKind,
    SessionProjection, Unsupported, apply_built, block_rows_to_markdown, build_nodes,
    project_session_nodes, read_review_meta_from_map, seed_session_nodes, utf16_len,
    write_review_meta_to_map,
};
use quarry_core::{QuarryError, WriteOutcome, now_timestamp, render_markdown_frontmatter};
use quarry_storage::{
    BlockMutationCommit, BlockMutationOutcome, BlockReviewItem, BlockReviewKind, BlockReviewState,
    DocumentScopeRef, QuarryStore,
};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, MutexGuard as StdMutexGuard};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use yrs::encoding::write::Write;
use yrs::sync::Awareness;
use yrs::updates::encoder::{Encode, Encoder, EncoderV1};
use yrs::{Doc, OffsetKind, Options, ReadTxn, Transact, TransactionMut, WriteTxn, XmlTextRef};

/// How long after the last doc update the debounced checkpoint fires.
pub(crate) const CHECKPOINT_DEBOUNCE: Duration = Duration::from_millis(2_000);
/// Retries when an external legacy write races a checkpoint commit.
const CHECKPOINT_RETRY_LIMIT: usize = 5;
/// The review-map root shared with the browser (`review-doc.ts`).
pub(crate) const REVIEW_ROOT: &str = "review";

/// Top-level websocket message type for checkpoint-ack frames, outside the
/// y-protocols range (0–3). Payload: one var-length buffer carrying the
/// committed doc state as a v1-encoded Yjs snapshot (state vector + delete
/// set). Sent point-to-point to each new subscriber on join and broadcast
/// after every durable commit; the browser's save state derives from
/// comparing it against the local doc (`rust-ws-provider.ts`).
pub const MSG_QUARRY_CHECKPOINT: u8 = 113;

/// Top-level websocket message type broadcast when a checkpoint attempt
/// fails: no payload — the signal is "the last save attempt did not commit"
/// (details live in the server logs). The browser's save state surfaces it
/// as "Save failed" until a later checkpoint ack covers the doc.
pub const MSG_QUARRY_CHECKPOINT_FAILED: u8 = 114;

/// Application WebSocket close code for a refused collab session. The browser
/// provider keys off this code to stop reconnecting and surface the reason
/// instead of retrying forever (see `rust-ws-provider.ts`).
pub const SESSION_REFUSED_CLOSE_CODE: u16 = 4400;

/// The refusal reason for paths that must not reveal specifics — e.g. whether
/// an internal document id names a tmp document.
const SESSION_REFUSED_GENERIC_REASON: &str = "collab session refused";

/// Completes the closing handshake with the refusal code and reason. A bare
/// drop resets the TCP stream instead, which the browser cannot distinguish
/// from a transient outage — it would retry forever. Dropping right after
/// `send` is just as bad: browsers report an unclean close as 1006 and
/// discard the code, so this drains until the peer's close reply (bounded)
/// to finish the handshake cleanly.
async fn refuse_socket(mut socket: WebSocket, reason: String) {
    let close_frame = Message::Close(Some(CloseFrame {
        code: SESSION_REFUSED_CLOSE_CODE,
        reason: reason.into(),
    }));
    if socket.send(close_frame).await.is_err() {
        return;
    }
    let _ = timeout(Duration::from_secs(1), async {
        while let Some(Ok(message)) = socket.recv().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    })
    .await;
}

type AwarenessRef = Arc<RwLock<Awareness>>;

/// The attribution label for a checkpoint: every connected client's
/// slate-yjs cursor-data name (`{ data: { name } }`), deduped and joined.
/// Multiple participants produce "Avery, Blake".
fn awareness_actor(awareness: &Awareness) -> Option<String> {
    let mut names: Vec<String> = awareness
        .iter()
        .filter_map(|(_, state)| {
            let raw = state.data?;
            let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
            let name = json.get("data")?.get("name")?.as_str()?.trim().to_owned();
            (!name.is_empty()).then_some(name)
        })
        .collect();
    names.sort();
    names.dedup();
    (!names.is_empty()).then(|| names.join(", "))
}

// ---------------------------------------------------------------------------
// Hub and per-document entries
// ---------------------------------------------------------------------------

/// Whether a collab socket has proven the capability needed to open a
/// tmp-scoped document. Tmp documents are secret-in-URL capabilities: only the
/// secret-authenticated `/v1/tmp/collab/{secret}/{room}` route may seed one.
/// The raw `/v1/collab/{document_id}` route takes an internal id and carries no
/// secret, so it must never seed a tmp document — otherwise a leaked or guessed
/// id would bypass the secret.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CollabAccess {
    /// Raw-id route: library documents only; a tmp-scoped seed is refused.
    LibraryOnly,
    /// Secret-authenticated tmp route: the caller resolved the secret already.
    TmpAuthorized,
}

impl CollabAccess {
    /// Whether a document of `scope` may not be reached at this access level.
    /// `LibraryOnly` (the secret-less raw route) is refused a tmp document.
    fn refuses(self, scope: &DocumentScopeRef) -> bool {
        matches!(self, CollabAccess::LibraryOnly) && matches!(scope, DocumentScopeRef::Tmp)
    }
}

#[derive(Clone)]
pub(crate) struct SessionHub {
    entries: Arc<Mutex<HashMap<String, Arc<DocEntry>>>>,
    store: QuarryStore,
}

struct DocEntry {
    state: Arc<Mutex<DocState>>,
}

#[derive(Default)]
struct DocState {
    session: Option<Arc<LiveSession>>,
}

/// The per-document mutex, held across seed/checkpoint/discard/transaction.
pub(crate) struct DocumentGuard {
    state: OwnedMutexGuard<DocState>,
}

impl DocumentGuard {
    pub(crate) fn session(&self) -> Option<Arc<LiveSession>> {
        self.state.session.clone()
    }
}

impl SessionHub {
    pub(crate) fn new(store: QuarryStore) -> Self {
        Self {
            entries: Arc::default(),
            store,
        }
    }

    async fn entry(&self, document_id: &str) -> Arc<DocEntry> {
        let mut entries = self.entries.lock().await;
        // Phase 4 funnels every Git/FUSE/CLI whole-file write through
        // `lock_document`, so entries must not accumulate one Arc per
        // document ever locked. Sweep on each access: an entry is evictable
        // only when nothing outside this map can reach it — no outstanding
        // `Arc<DocEntry>` (held by `serve_socket` for a socket's lifetime),
        // no outstanding state Arc (owned mutex guards, pending `lock_owned`
        // futures, and a live session's checkpoint task all hold one), and
        // no live session. Re-creating an entry later is safe because no
        // holder of the old mutex can still exist.
        entries.retain(|_, entry| {
            if Arc::strong_count(entry) > 1 || Arc::strong_count(&entry.state) > 1 {
                return true;
            }
            match entry.state.try_lock() {
                Ok(state) => state.session.is_some(),
                Err(_) => true,
            }
        });
        entries
            .entry(document_id.to_string())
            .or_insert_with(|| {
                Arc::new(DocEntry {
                    state: Arc::new(Mutex::new(DocState::default())),
                })
            })
            .clone()
    }

    #[cfg(test)]
    async fn entry_count(&self) -> usize {
        self.entries.lock().await.len()
    }

    /// Serializes against seed/checkpoint/discard/transactions for one
    /// document. The guard exposes the live session, if any.
    pub(crate) async fn lock_document(&self, document_id: &str) -> DocumentGuard {
        let entry = self.entry(document_id).await;
        DocumentGuard {
            state: entry.state.clone().lock_owned().await,
        }
    }

    pub(crate) async fn serve_socket(
        &self,
        document_id: String,
        access: CollabAccess,
        socket: WebSocket,
        shutdown: CancellationToken,
    ) {
        let collab_session_id = Uuid::new_v4().to_string();
        let entry = self.entry(&document_id).await;
        let session = {
            let mut state = entry.state.clone().lock_owned().await;
            let session = match &state.session {
                Some(session) => {
                    // Enforce the capability against the cached session too: a
                    // tmp document seeded via the secret route stays reachable
                    // by its internal id, and that id is not itself a secret.
                    if access.refuses(&session.scope) {
                        tracing::debug!(
                            event = "collab.session.refused",
                            %document_id,
                            reason_code = "tmp_requires_secret",
                            "collab session refused: tmp document requires the secret-authenticated route"
                        );
                        refuse_socket(socket, SESSION_REFUSED_GENERIC_REASON.to_string()).await;
                        return;
                    }
                    session.clone()
                }
                None => {
                    match LiveSession::seed(&self.store, &document_id, access, entry.state.clone())
                        .await
                    {
                        Ok(Some(session)) => {
                            state.session = Some(session.clone());
                            session
                        }
                        Ok(None) => {
                            // `seed` logs the specific refusal reason
                            // (not a block document, or a tmp document reached
                            // without the secret).
                            refuse_socket(socket, SESSION_REFUSED_GENERIC_REASON.to_string()).await;
                            return;
                        }
                        Err(error) => {
                            tracing::warn!(
                                event = "collab.session.refused",
                                %document_id,
                                ?error,
                                reason_code = "seed_failed",
                                "collab session refused: seeding from rows failed"
                            );
                            // Only the unsupported-markdown detail is shared;
                            // storage errors stay server-side.
                            let reason = match &error {
                                QuarryError::UnsupportedMarkdown(unsupported) => {
                                    format!("unsupported markdown: {}", unsupported.0)
                                }
                                _ => SESSION_REFUSED_GENERIC_REASON.to_string(),
                            };
                            refuse_socket(socket, reason).await;
                            return;
                        }
                    }
                }
            };
            session.subscribers.fetch_add(1, Ordering::SeqCst);
            session
        };
        tracing::debug!(
            event = "collab.socket.opened",
            %document_id,
            %collab_session_id,
            "collab socket opened"
        );

        let result = serve_session_socket(&session, socket, &collab_session_id, shutdown).await;
        match result {
            Ok(()) => tracing::debug!(
                event = "collab.socket.closed",
                %document_id,
                %collab_session_id,
                outcome = "closed",
                "collab socket closed"
            ),
            Err(error) => tracing::debug!(
                event = "collab.socket.closed",
                %document_id,
                %collab_session_id,
                outcome = "protocol_error",
                reason = ?error,
                "collab websocket closed with protocol error"
            ),
        }

        // Leave: under the document mutex, run the final checkpoint and
        // discard the session when the last subscriber departs.
        let mut state = entry.state.clone().lock_owned().await;
        if session.subscribers.fetch_sub(1, Ordering::SeqCst) == 1 {
            if let Err(error) = session.checkpoint_browser_session(&self.store).await {
                tracing::warn!(
                    event = "collab.session.final_checkpoint_failed",
                    %document_id,
                    ?error,
                    "final session checkpoint failed; un-checkpointed edits are lost"
                );
            }
            session.shutdown_background_tasks().await;
            if state
                .session
                .as_ref()
                .is_some_and(|live| Arc::ptr_eq(live, &session))
            {
                state.session = None;
            }
            tracing::debug!(
                event = "collab.session.discarded",
                %document_id,
                "collab session discarded after last subscriber left"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Live session
// ---------------------------------------------------------------------------

pub(crate) struct LiveSession {
    pub(crate) document_id: String,
    scope: DocumentScopeRef,
    awareness: AwarenessRef,
    broadcast_tx: broadcast::Sender<Vec<u8>>,
    _doc_sub: yrs::Subscription,
    _awareness_sub: yrs::Subscription,
    awareness_task: StdMutex<Option<JoinHandle<()>>>,
    checkpoint_task: StdMutex<Option<JoinHandle<()>>>,
    /// Bumped on every doc update (browser or server).
    update_seq: Arc<AtomicU64>,
    committed: StdMutex<CommittedState>,
    subscribers: AtomicUsize,
}

struct CommittedState {
    /// The `update_seq` covered by the last checkpoint.
    checkpointed_seq: u64,
    /// The head version the next checkpoint expects (last committed by this
    /// session, or the seed head).
    head_version_id: String,
    /// The committed doc state as a v1-encoded snapshot (seed or last commit),
    /// served to new subscribers and broadcast on every commit.
    snapshot: Vec<u8>,
    /// The full review item set as of the last seed/checkpoint/transaction.
    items: HashMap<String, BlockReviewItem>,
    /// Last non-empty awareness author label, used by any checkpoint that
    /// observes a name-less awareness: the final one after the socket closed
    /// and awareness emptied, but also e.g. when a named participant leaves
    /// cleanly while an unnamed one keeps editing — subsequent checkpoints keep
    /// the last seen name until the session is dropped. Known gap: if a client
    /// cleanly removes its awareness state and disconnects before any checkpoint
    /// ever ran, the final checkpoint falls back to "browser" — accepted, since
    /// the common abrupt close keeps the state and any prior checkpoint primes
    /// the cache.
    live_actor: Option<String>,
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        if let Some(task) = self.awareness_task_lock().take() {
            task.abort();
        }
        if let Some(task) = self.checkpoint_task_lock().take() {
            task.abort();
        }
    }
}

impl LiveSession {
    fn awareness_task_lock(&self) -> StdMutexGuard<'_, Option<JoinHandle<()>>> {
        self.awareness_task
            .lock()
            .expect("live session awareness task lock poisoned")
    }

    fn checkpoint_task_lock(&self) -> StdMutexGuard<'_, Option<JoinHandle<()>>> {
        self.checkpoint_task
            .lock()
            .expect("live session checkpoint task lock poisoned")
    }

    fn committed_lock(&self) -> StdMutexGuard<'_, CommittedState> {
        self.committed
            .lock()
            .expect("live session committed state lock poisoned")
    }

    /// Seeds a fresh session from canonical state. `Ok(None)` when the
    /// document cannot host a session (missing, deleted, or raw).
    async fn seed(
        store: &QuarryStore,
        document_id: &str,
        access: CollabAccess,
        entry_state: Arc<Mutex<DocState>>,
    ) -> Result<Option<Arc<LiveSession>>, QuarryError> {
        let Some(seed) = store.session_seed_state(document_id).await? else {
            tracing::debug!(
                event = "collab.session.refused",
                %document_id,
                reason_code = "not_a_block_document",
                "collab session refused: missing, deleted, or raw document"
            );
            return Ok(None);
        };
        if access.refuses(&seed.scope) {
            tracing::debug!(
                event = "collab.session.refused",
                %document_id,
                reason_code = "tmp_requires_secret",
                "collab session refused: tmp document requires the secret-authenticated route"
            );
            return Ok(None);
        }
        let items: HashMap<String, BlockReviewItem> = seed
            .review_items
            .iter()
            .map(|item| (item.id.clone(), item.clone()))
            .collect();
        let anchors = doc_anchors(&seed.review_items);
        let nodes = seed_session_nodes(&seed.rows, &anchors)?;
        let built = build_nodes(&nodes)?;
        let meta = review_meta_for_items(&seed.review_items);

        let doc = Doc::with_options(Options {
            offset_kind: OffsetKind::Utf16,
            ..Default::default()
        });
        {
            let mut txn = doc.transact_mut();
            let text = txn.get_or_insert_text(SHARED_ROOT);
            let root: &XmlTextRef = text.as_ref();
            let root = root.clone();
            apply_built(&mut txn, &root, 0, &built);
            let review = txn.get_or_insert_map(REVIEW_ROOT);
            write_review_meta_to_map(&mut txn, &review, &meta);
        }
        let committed_snapshot = doc.transact().snapshot().encode_v1();

        let awareness = Arc::new(RwLock::new(Awareness::new(doc)));
        let (broadcast_tx, _broadcast_keepalive) = broadcast::channel(64);
        let (dirty_tx, mut dirty_rx) = unbounded_channel::<()>();
        // The doc observer owns the only sender; the channel closes with it.
        let update_seq = Arc::new(AtomicU64::new(0));

        let mut lock = awareness.write().await;
        let doc_sub = {
            let sink = broadcast_tx.clone();
            let seq = update_seq.clone();
            let dirty = dirty_tx;
            lock.doc()
                .observe_update_v1(move |_txn, update| {
                    seq.fetch_add(1, Ordering::SeqCst);
                    let mut encoder = EncoderV1::new();
                    encoder.write_var(yrs::sync::protocol::MSG_SYNC);
                    encoder.write_var(yrs::sync::protocol::MSG_SYNC_UPDATE);
                    encoder.write_buf(&update.update);
                    let _ = sink.send(encoder.to_vec());
                    let _ = dirty.send(());
                })
                .map_err(|error| QuarryError::Invariant(format!("observe session doc: {error}")))?
        };
        let (awareness_changes_tx, mut awareness_changes_rx) = unbounded_channel();
        let awareness_sub = lock.on_update(move |_awareness, event, _origin| {
            let _ = awareness_changes_tx.send(event.all_changes());
        });
        drop(lock);

        let awareness_task = {
            let awareness = Arc::downgrade(&awareness);
            let sink = broadcast_tx.clone();
            tokio::spawn(async move {
                while let Some(changed) = awareness_changes_rx.recv().await {
                    let Some(awareness) = awareness.upgrade() else {
                        return;
                    };
                    let awareness = awareness.read().await;
                    if let Ok(update) = awareness.update_with_clients(changed) {
                        let _ = sink.send(yrs::sync::Message::Awareness(update).encode_v1());
                    }
                }
            })
        };

        let session = Arc::new(LiveSession {
            document_id: document_id.to_string(),
            scope: seed.scope.clone(),
            awareness,
            broadcast_tx,
            _doc_sub: doc_sub,
            _awareness_sub: awareness_sub,
            awareness_task: StdMutex::new(Some(awareness_task)),
            checkpoint_task: StdMutex::new(None),
            update_seq,
            committed: StdMutex::new(CommittedState {
                checkpointed_seq: 0,
                head_version_id: seed.head_version_id.clone(),
                snapshot: committed_snapshot,
                items,
                live_actor: None,
            }),
            subscribers: AtomicUsize::new(0),
        });

        // Debounced checkpointer: waits for quiet after the last update,
        // then checkpoints under the document mutex.
        let checkpoint_task = {
            let store = store.clone();
            let weak = Arc::downgrade(&session);
            tokio::spawn(async move {
                while dirty_rx.recv().await.is_some() {
                    while timeout(CHECKPOINT_DEBOUNCE, dirty_rx.recv())
                        .await
                        .is_ok_and(|received| received.is_some())
                    {}
                    let Some(session) = weak.upgrade() else {
                        return;
                    };
                    let _guard = entry_state.lock().await;
                    if let Err(error) = session.checkpoint_browser_session(&store).await {
                        tracing::warn!(
                            event = "collab.session.checkpoint_failed",
                            document_id = %session.document_id,
                            ?error,
                            "debounced session checkpoint failed; will retry on the next edit"
                        );
                    }
                }
            })
        };
        *session.checkpoint_task_lock() = Some(checkpoint_task);

        match &session.scope {
            DocumentScopeRef::Library { .. } => {
                tracing::debug!(
                    event = "collab.session.seeded",
                    %document_id,
                    scope = %session.scope_label(),
                    path = %seed.path,
                    head_version_id = %seed.head_version_id,
                    blocks = seed.rows.len(),
                    review_items = seed.review_items.len(),
                    "collab session seeded from block rows"
                );
            }
            DocumentScopeRef::Tmp => {
                tracing::debug!(
                    event = "collab.session.seeded",
                    %document_id,
                    scope = %"tmp",
                    head_version_id = %seed.head_version_id,
                    blocks = seed.rows.len(),
                    review_items = seed.review_items.len(),
                    "collab session seeded from block rows"
                );
            }
        }
        Ok(Some(session))
    }

    pub(crate) fn awareness(&self) -> &AwarenessRef {
        &self.awareness
    }

    fn scope_label(&self) -> String {
        match &self.scope {
            DocumentScopeRef::Library { slug } => format!("library:{slug}"),
            DocumentScopeRef::Tmp => "tmp".to_string(),
        }
    }

    async fn shutdown_background_tasks(&self) {
        let checkpoint_task = self.checkpoint_task_lock().take();
        let awareness_task = self.awareness_task_lock().take();
        for (task_name, task) in [
            ("checkpoint", checkpoint_task),
            ("awareness", awareness_task),
        ] {
            if let Some(task) = task {
                task.abort();
                match task.await {
                    Ok(()) => {}
                    Err(error) if error.is_cancelled() => {}
                    Err(error) => tracing::debug!(
                        event = "collab.session.task_join_failed",
                        %task_name,
                        document_id = %self.document_id,
                        ?error,
                        "collab session background task ended with an unexpected join error"
                    ),
                }
            }
        }
    }

    pub(crate) fn subscribe_broadcast(&self) -> broadcast::Receiver<Vec<u8>> {
        self.broadcast_tx.subscribe()
    }

    /// The checkpoint-ack frame for the current committed state, sent to
    /// each new subscriber on join.
    pub(crate) fn committed_ack_frame(&self) -> Vec<u8> {
        checkpoint_ack_frame(&self.committed_lock().snapshot)
    }

    /// Captures the committed doc state under the doc write lock, records it,
    /// and broadcasts its ack frame (after the commit's own doc updates, so
    /// clients apply state first). The `&mut Awareness` is the type-level
    /// proof of exclusivity: the session's awareness only lives inside its
    /// `RwLock`, so a mutable borrow can only come from the held write guard
    /// — the snapshot can never cover state a concurrent writer is mutating.
    fn record_committed(
        &self,
        awareness: &mut Awareness,
        checkpointed_seq: u64,
        head_version_id: String,
        items: HashMap<String, BlockReviewItem>,
    ) {
        let snapshot = awareness.doc().transact().snapshot().encode_v1();
        let frame = checkpoint_ack_frame(&snapshot);
        {
            let mut committed = self.committed_lock();
            committed.checkpointed_seq = checkpointed_seq;
            committed.head_version_id = head_version_id;
            committed.items = items;
            committed.snapshot = snapshot;
        }
        let _ = self.broadcast_tx.send(frame);
    }

    pub(crate) fn items_snapshot(&self) -> Vec<BlockReviewItem> {
        let mut items: Vec<BlockReviewItem> =
            self.committed_lock().items.values().cloned().collect();
        items.sort_by(|left, right| left.id.cmp(&right.id));
        items
    }

    /// Commits a coalesced `browser_session` checkpoint of the current doc
    /// state. Caller holds the document mutex. `Ok(None)` = clean. A failed
    /// commit broadcasts [`MSG_QUARRY_CHECKPOINT_FAILED`] before returning,
    /// so still-connected browsers show "Save failed" instead of an
    /// indefinite "Saving…" while the retry loop spins.
    pub(crate) async fn checkpoint_browser_session(
        &self,
        store: &QuarryStore,
    ) -> Result<Option<WriteOutcome>, QuarryError> {
        let mut awareness = self.awareness.write().await;
        if !self.is_dirty() {
            return Ok(None);
        }
        match self.commit_doc_state(store, &mut awareness).await {
            Ok(outcome) => Ok(Some(outcome)),
            Err(error) => {
                let _ = self.broadcast_tx.send(checkpoint_failed_frame());
                Err(error)
            }
        }
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.update_seq.load(Ordering::SeqCst) != self.committed_lock().checkpointed_seq
    }

    /// Projects the locked doc and commits it as the new canonical state:
    /// one version + one coalesced `browser_session` history row. (Gateway
    /// session-mode transactions commit through their own
    /// `BlockMutationCommit` and record themselves via [`Self::mark_committed`].)
    pub(crate) async fn commit_doc_state(
        &self,
        store: &QuarryStore,
        awareness: &mut Awareness,
    ) -> Result<WriteOutcome, QuarryError> {
        let seq = self.update_seq.load(Ordering::SeqCst);
        let (projection, meta) = self.project_locked(awareness)?;
        let prior_items = self.committed_lock().items.clone();
        let now = now_timestamp();
        let items =
            reconcile_review_items(&self.document_id, &prior_items, &projection, &meta, &now);
        let transaction_actor = {
            let mut committed = self.committed_lock();
            if let Some(actor) = awareness_actor(awareness) {
                committed.live_actor = Some(actor);
            }
            committed
                .live_actor
                .clone()
                .unwrap_or_else(|| "browser".to_string())
        };

        for attempt in 0..CHECKPOINT_RETRY_LIMIT {
            let Some(head) = store.session_seed_state(&self.document_id).await? else {
                return Err(QuarryError::NotFound(format!(
                    "document {} disappeared mid-session",
                    self.document_id
                )));
            };
            let expected_head_version_id = self.committed_lock().head_version_id.clone();
            if attempt > 0 || head.head_version_id != expected_head_version_id {
                tracing::warn!(
                    event = "collab.session.head_moved",
                    document_id = %self.document_id,
                    expected = %expected_head_version_id,
                    found = %head.head_version_id,
                    "external write raced a live session; the session state wins (transitional)"
                );
            }
            let normalized = format!(
                "{}{}",
                render_markdown_frontmatter(&head.metadata)?,
                block_rows_to_markdown(&projection.rows)?
            );
            let commit = BlockMutationCommit {
                document_id: self.document_id.clone(),
                expected_head_version_id: head.head_version_id.clone(),
                client_tx_id: format!("session-checkpoint-{}", Uuid::new_v4()),
                actor_kind: "browser_session".to_string(),
                actor_id: None,
                transaction_actor: Some(transaction_actor.clone()),
                transaction_message: Some("Live session edits".to_string()),
                transaction_provenance: Some(json!({
                    "history": {
                        "kind": "autosave",
                        "reason": "session_checkpoint",
                    }
                })),
                origin_id: Some(format!(
                    "agent-injected:session-checkpoint:{}",
                    Uuid::new_v4()
                )),
                source: quarry_core::DocumentSource::Rest,
                recorded_ops: json!({
                    "ops": [],
                    "actor": { "kind": "browser_session" },
                    "ack": { "status": "committed", "changed_block_ids": [] },
                }),
                metadata: head.metadata.clone(),
                content_type: head.content_type.clone(),
                rows: projection.rows.clone(),
                review_items: sorted_items(&items),
                normalized_markdown: normalized,
            };
            match store
                .commit_block_mutation_for_scope(&self.scope, commit)
                .await
            {
                Ok(BlockMutationOutcome::Applied { outcome, .. }) => {
                    self.record_committed(awareness, seq, outcome.version.id.to_string(), items);
                    tracing::debug!(
                        event = "collab.session.checkpointed",
                        document_id = %self.document_id,
                        version_id = %outcome.version.id,
                        blocks = projection.rows.len(),
                        "session checkpoint committed"
                    );
                    return Ok(*outcome);
                }
                Ok(BlockMutationOutcome::Replayed(_)) => {
                    return Err(QuarryError::Conflict(format!(
                        "session checkpoint client_tx_id collided for document {}",
                        self.document_id
                    )));
                }
                Err(QuarryError::PreconditionFailed(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(QuarryError::Busy(format!(
            "document {} head kept moving during a session checkpoint",
            self.document_id
        )))
    }

    /// Projects the locked doc into rows/anchors plus the review meta map.
    /// Caller must hold the doc's write lock (the `awareness` it passes is
    /// the locked guard's target) so the projection cannot race updates.
    pub(crate) fn project_locked(
        &self,
        awareness: &Awareness,
    ) -> Result<(SessionProjection, ReviewMeta), QuarryError> {
        let txn = awareness.doc().transact();
        let root = content_root(&txn)?;
        let fragment =
            quarry_collab_codec::xmltext_to_slate(&txn, &root).map_err(session_projection_error)?;
        let Node::Element { children, .. } = fragment else {
            return Err(QuarryError::Invariant(
                "session doc root did not project to a fragment".to_string(),
            ));
        };
        let projection = project_session_nodes(&children, || Uuid::new_v4().to_string())
            .map_err(session_projection_error)?;
        if !projection.dropped_marks.is_empty() {
            tracing::warn!(
                event = "collab.session.unknown_marks_dropped",
                document_id = %self.document_id,
                marks = ?projection.dropped_marks,
                "dropped inline marks the Markdown writer cannot render; \
                 the checkpoint persists without them"
            );
        }
        let meta = match txn.get_map(REVIEW_ROOT) {
            Some(map) => read_review_meta_from_map(&txn, &map),
            None => ReviewMeta::default(),
        };
        Ok((projection, meta))
    }

    /// Applies a desired doc image (from the gateway's apply engine) into the
    /// locked live doc, as the session's own collaborator client.
    pub(crate) fn apply_desired_state(
        &self,
        awareness: &Awareness,
        pre: &[Node],
        desired: &[Node],
        desired_items: &[BlockReviewItem],
    ) -> Result<(), QuarryError> {
        let mut txn = awareness.doc().transact_mut();
        let root = content_root_mut(&mut txn)?;
        quarry_collab_codec::reconcile_session_children(&mut txn, &root, pre, desired)
            .map_err(session_projection_error)?;
        let review = txn.get_or_insert_map(REVIEW_ROOT);
        write_review_meta_to_map(&mut txn, &review, &review_meta_for_items(desired_items));
        Ok(())
    }

    /// Marks the current doc state as covered by `outcome` (used by the
    /// gateway after committing a session-mode transaction while holding the
    /// awareness write lock) and broadcasts the covering checkpoint ack.
    pub(crate) fn mark_committed(
        &self,
        awareness: &mut Awareness,
        outcome: &WriteOutcome,
        items: &[BlockReviewItem],
    ) {
        let items = items
            .iter()
            .map(|item| (item.id.clone(), item.clone()))
            .collect();
        self.record_committed(
            awareness,
            self.update_seq.load(Ordering::SeqCst),
            outcome.version.id.to_string(),
            items,
        );
    }
}

/// Encodes one checkpoint-ack frame: `MSG_QUARRY_CHECKPOINT` + the
/// v1-encoded snapshot as a var-length buffer.
fn checkpoint_ack_frame(snapshot: &[u8]) -> Vec<u8> {
    let mut encoder = EncoderV1::new();
    encoder.write_var(MSG_QUARRY_CHECKPOINT);
    encoder.write_buf(snapshot);
    encoder.to_vec()
}

fn checkpoint_failed_frame() -> Vec<u8> {
    let mut encoder = EncoderV1::new();
    encoder.write_var(MSG_QUARRY_CHECKPOINT_FAILED);
    encoder.to_vec()
}

fn session_projection_error(error: Unsupported) -> QuarryError {
    QuarryError::UnsupportedMarkdown(error)
}

fn content_root<T: ReadTxn>(txn: &T) -> Result<XmlTextRef, QuarryError> {
    let text = txn.get_text(SHARED_ROOT).ok_or_else(|| {
        QuarryError::Invariant("session doc is missing its content root".to_string())
    })?;
    let root: &XmlTextRef = text.as_ref();
    Ok(root.clone())
}

fn content_root_mut(txn: &mut TransactionMut<'_>) -> Result<XmlTextRef, QuarryError> {
    let text = txn.get_or_insert_text(SHARED_ROOT);
    let root: &XmlTextRef = text.as_ref();
    Ok(root.clone())
}

// ---------------------------------------------------------------------------
// Review item ↔ session doc reconciliation
// ---------------------------------------------------------------------------

/// Whether an item is represented in the live doc (as marks and/or a review
/// meta entry). Everything else passes through checkpoints untouched.
fn doc_represented(item: &BlockReviewItem) -> bool {
    match item.kind {
        BlockReviewKind::Comment => {
            if item.parent_item_id.is_some() {
                // Replies ride only in the meta map, keyed under the thread.
                return matches!(
                    item.state,
                    BlockReviewState::Open | BlockReviewState::Resolved
                );
            }
            matches!(
                item.state,
                BlockReviewState::Open | BlockReviewState::Resolved
            ) && item.start_offset < item.end_offset
        }
        BlockReviewKind::Suggestion => {
            item.state == BlockReviewState::Open
                && (item.start_offset < item.end_offset
                    || item
                        .replacement
                        .as_deref()
                        .is_some_and(|replacement| !replacement.is_empty()))
        }
        BlockReviewKind::Conflict => false,
    }
}

/// The doc-mark image of a row item set: open/resolved comments and open
/// suggestions become [`SessionAnchor`]s (replies and dead anchors do not).
pub(crate) fn doc_anchors(items: &[BlockReviewItem]) -> Vec<SessionAnchor> {
    items
        .iter()
        .filter(|item| doc_represented(item) && item.parent_item_id.is_none())
        .map(|item| SessionAnchor {
            id: item.id.clone(),
            kind: match item.kind {
                BlockReviewKind::Suggestion => SessionAnchorKind::Suggestion {
                    replacement: item.replacement.clone().unwrap_or_default(),
                    by: item.author.clone(),
                    at_ms: chrono_ms(&item.created_at),
                },
                _ => SessionAnchorKind::Comment,
            },
            block_id: item.block_id.clone(),
            start: item.start_offset,
            end: item.end_offset,
        })
        .collect()
}

fn chrono_ms(at: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0)
}

/// The review meta map image of a row item set (what the browser reads for
/// bodies, authors, threading, and resolution state).
pub(crate) fn review_meta_for_items(items: &[BlockReviewItem]) -> ReviewMeta {
    let mut comments = BTreeMap::new();
    let mut suggestions = BTreeMap::new();
    for item in items.iter().filter(|item| doc_represented(item)) {
        let entry = ReviewMetaEntry {
            by: item.author.clone().unwrap_or_else(|| "unknown".to_string()),
            at: item.created_at.clone(),
            edited_at: edited_at_for_item(item),
            body: item.body.clone(),
            re: item.parent_item_id.clone(),
            status: (item.state == BlockReviewState::Resolved).then(|| "resolved".to_string()),
            resolved: (item.state == BlockReviewState::Resolved).then(|| item.updated_at.clone()),
        };
        match item.kind {
            BlockReviewKind::Suggestion => {
                suggestions.insert(item.id.clone(), entry);
            }
            _ => {
                comments.insert(item.id.clone(), entry);
            }
        }
    }
    ReviewMeta {
        comments,
        suggestions,
    }
}

fn edited_at_for_item(item: &BlockReviewItem) -> Option<String> {
    (item.updated_at != item.created_at).then(|| item.updated_at.clone())
}

/// Merges the checkpoint projection back into the full review item set:
///
/// - extracted anchors update/create items (anchor offsets, bodies and
///   resolution state from the meta map);
/// - doc-represented items whose marks vanished orphan (comments) or
///   invalidate (suggestions) when their meta entry remains, and are deleted
///   when the browser removed the entry (comment delete, suggestion
///   accept/reject);
/// - meta-only reply entries upsert reply items under their thread root;
/// - items never represented in the doc (orphaned/invalidated/resolved
///   suggestions/conflicts) pass through, with anchors clamped to the new
///   text so the commit-time validation holds.
fn reconcile_review_items(
    document_id: &str,
    prior: &HashMap<String, BlockReviewItem>,
    projection: &SessionProjection,
    meta: &ReviewMeta,
    now: &str,
) -> HashMap<String, BlockReviewItem> {
    ReviewReconciliation::new(document_id, prior, projection, meta, now).run()
}

struct ReviewReconciliation<'a> {
    document_id: &'a str,
    prior: &'a HashMap<String, BlockReviewItem>,
    projection: &'a SessionProjection,
    meta: &'a ReviewMeta,
    now: &'a str,
    texts: HashMap<&'a str, &'a str>,
    extracted: HashMap<&'a str, &'a SessionAnchor>,
    items: HashMap<String, BlockReviewItem>,
}

impl<'a> ReviewReconciliation<'a> {
    fn new(
        document_id: &'a str,
        prior: &'a HashMap<String, BlockReviewItem>,
        projection: &'a SessionProjection,
        meta: &'a ReviewMeta,
        now: &'a str,
    ) -> Self {
        let texts = projection
            .rows
            .iter()
            .map(|row| (row.block_id.as_str(), row.text.as_str()))
            .collect();
        let extracted = projection
            .anchors
            .iter()
            .map(|anchor| (anchor.id.as_str(), anchor))
            .collect();
        Self {
            document_id,
            prior,
            projection,
            meta,
            now,
            texts,
            extracted,
            items: HashMap::new(),
        }
    }

    fn run(mut self) -> HashMap<String, BlockReviewItem> {
        self.reconcile_prior_items();
        self.insert_browser_created_anchors();
        self.upsert_reply_items();
        self.items
    }

    fn reconcile_prior_items(&mut self) {
        for (id, item) in self.prior {
            if !doc_represented(item) {
                self.items
                    .insert(id.clone(), clamped(item.clone(), &self.texts));
                continue;
            }
            if item.parent_item_id.is_some() {
                // Replies are handled with their thread root below.
                continue;
            }
            self.reconcile_prior_root(id, item);
        }
    }

    fn reconcile_prior_root(&mut self, id: &str, item: &BlockReviewItem) {
        let meta_entry = self.meta_entry_for(item.kind, id);
        match (self.extracted.get(id), meta_entry) {
            (Some(anchor), entry) => {
                self.items.insert(
                    id.to_string(),
                    self.updated_from_anchor(item, anchor, entry),
                );
            }
            (None, Some(_)) => {
                // The anchored text was deleted but the entry remains: the
                // Gate A "collapsed means orphaned" rule, at the row layer.
                let mut updated = item.clone();
                updated.state = match item.kind {
                    BlockReviewKind::Suggestion => BlockReviewState::Invalidated,
                    _ => BlockReviewState::Orphaned,
                };
                updated.updated_at = self.now.to_string();
                self.items
                    .insert(id.to_string(), clamped(updated, &self.texts));
            }
            (None, None) => {
                // Browser deleted the item (comment delete, suggestion
                // accept/reject): drop it and its replies.
            }
        }
    }

    fn updated_from_anchor(
        &self,
        item: &BlockReviewItem,
        anchor: &SessionAnchor,
        entry: Option<&ReviewMetaEntry>,
    ) -> BlockReviewItem {
        let mut updated = item.clone();
        updated.block_id = anchor.block_id.clone();
        updated.start_offset = anchor.start;
        updated.end_offset = anchor.end;
        if item.kind == BlockReviewKind::Suggestion {
            updated.replacement = match &anchor.kind {
                SessionAnchorKind::Suggestion { replacement, .. } => Some(replacement.clone()),
                SessionAnchorKind::Comment => None,
            };
        }
        if let Some(entry) = entry {
            updated.body = entry.body.clone().or(updated.body);
            let resolved = entry.status.as_deref() == Some("resolved");
            updated.state = if resolved {
                BlockReviewState::Resolved
            } else if anchor.start == anchor.end && item.kind == BlockReviewKind::Comment {
                BlockReviewState::Orphaned
            } else {
                BlockReviewState::Open
            };
        } else if anchor.start == anchor.end && item.kind == BlockReviewKind::Comment {
            updated.state = BlockReviewState::Orphaned;
        }
        if updated != *item {
            let body_changed = updated.body != item.body;
            updated.updated_at = if body_changed {
                entry
                    .and_then(|entry| entry.edited_at.clone())
                    .unwrap_or_else(|| self.now.to_string())
            } else {
                self.now.to_string()
            };
        }
        updated
    }

    fn insert_browser_created_anchors(&mut self) {
        for anchor in &self.projection.anchors {
            if self.items.contains_key(&anchor.id) || self.prior.contains_key(&anchor.id) {
                continue;
            }
            self.items
                .insert(anchor.id.clone(), self.browser_created_item(anchor));
        }
    }

    fn browser_created_item(&self, anchor: &SessionAnchor) -> BlockReviewItem {
        let (kind, meta_entry, replacement, anchor_author) = match &anchor.kind {
            SessionAnchorKind::Suggestion {
                replacement, by, ..
            } => (
                BlockReviewKind::Suggestion,
                self.meta.suggestions.get(&anchor.id),
                Some(replacement.clone()),
                by.clone(),
            ),
            SessionAnchorKind::Comment => (
                BlockReviewKind::Comment,
                self.meta.comments.get(&anchor.id),
                None,
                None,
            ),
        };
        let quote = self
            .texts
            .get(anchor.block_id.as_str())
            .map(|text| utf16_slice_clamped(text, anchor.start, anchor.end));
        let created_at = meta_entry
            .map(|entry| entry.at.clone())
            .filter(|at| !at.is_empty())
            .unwrap_or_else(|| self.now.to_string());
        let updated_at = meta_entry
            .and_then(|entry| entry.edited_at.clone())
            .unwrap_or_else(|| created_at.clone());
        BlockReviewItem {
            id: anchor.id.clone(),
            document_id: self.document_id.to_string(),
            block_id: anchor.block_id.clone(),
            kind,
            start_offset: anchor.start,
            end_offset: anchor.end,
            body: meta_entry.and_then(|entry| entry.body.clone()),
            replacement,
            author: meta_entry.map(|entry| entry.by.clone()).or(anchor_author),
            state: if meta_entry.is_some_and(|entry| entry.status.as_deref() == Some("resolved")) {
                BlockReviewState::Resolved
            } else {
                BlockReviewState::Open
            },
            quote,
            context_before: None,
            context_after: None,
            parent_item_id: None,
            created_at,
            updated_at,
        }
    }

    fn upsert_reply_items(&mut self) {
        for (id, entry) in &self.meta.comments {
            let Some(parent_id) = &entry.re else {
                continue;
            };
            let Some(root) = self.items.get(parent_id).cloned() else {
                continue;
            };
            self.items
                .insert(id.clone(), self.reply_item(id, entry, parent_id, &root));
        }
    }

    fn reply_item(
        &self,
        id: &str,
        entry: &ReviewMetaEntry,
        parent_id: &str,
        root: &BlockReviewItem,
    ) -> BlockReviewItem {
        let prior_reply = self.prior.get(id);
        let created_at = prior_reply
            .map(|item| item.created_at.clone())
            .or_else(|| Some(entry.at.clone()).filter(|at| !at.is_empty()))
            .unwrap_or_else(|| self.now.to_string());
        let updated_at = entry
            .edited_at
            .clone()
            .or_else(|| prior_reply.map(|item| item.updated_at.clone()))
            .unwrap_or_else(|| created_at.clone());
        let mut reply = BlockReviewItem {
            id: id.to_string(),
            document_id: self.document_id.to_string(),
            block_id: root.block_id.clone(),
            kind: BlockReviewKind::Comment,
            start_offset: root.start_offset,
            end_offset: root.end_offset,
            body: entry.body.clone(),
            replacement: None,
            author: Some(entry.by.clone()),
            state: root.state,
            quote: root.quote.clone(),
            context_before: None,
            context_after: None,
            parent_item_id: Some(parent_id.to_string()),
            created_at,
            updated_at,
        };
        if entry.edited_at.is_none()
            && let Some(prior_reply) = prior_reply
        {
            let mut comparable = reply.clone();
            comparable.updated_at = prior_reply.updated_at.clone();
            if comparable != *prior_reply {
                reply.updated_at = self.now.to_string();
            }
        }
        reply
    }

    fn meta_entry_for(&self, kind: BlockReviewKind, id: &str) -> Option<&ReviewMetaEntry> {
        match kind {
            BlockReviewKind::Suggestion => self.meta.suggestions.get(id),
            _ => self.meta.comments.get(id),
        }
    }
}

/// Anchors of pass-through items must stay valid against the new text. Most
/// are dead anchors; an open block-deletion suggestion is the exception and
/// already carries the stable `[0, 0)` block anchor. Clamp stale ranges rather
/// than failing the whole checkpoint.
fn clamped(mut item: BlockReviewItem, texts: &HashMap<&str, &str>) -> BlockReviewItem {
    let Some(text) = texts.get(item.block_id.as_str()) else {
        return item;
    };
    let len = utf16_len(text);
    let start = clamp_to_boundary(text, item.start_offset.min(len));
    let end = clamp_to_boundary(text, item.end_offset.min(len)).max(start);
    if (start, end) != (item.start_offset, item.end_offset) {
        item.start_offset = start;
        item.end_offset = end;
        if item.state == BlockReviewState::Open && start == end {
            item.state = match item.kind {
                BlockReviewKind::Suggestion => BlockReviewState::Invalidated,
                _ => BlockReviewState::Orphaned,
            };
        }
    }
    item
}

fn clamp_to_boundary(text: &str, offset: u32) -> u32 {
    let mut candidate = offset;
    while candidate > 0 && !quarry_collab_codec::is_utf16_boundary(text, candidate) {
        candidate -= 1;
    }
    candidate
}

fn utf16_slice_clamped(text: &str, start: u32, end: u32) -> String {
    let len = utf16_len(text);
    let start = clamp_to_boundary(text, start.min(len));
    let end = clamp_to_boundary(text, end.min(len)).max(start);
    let units: Vec<u16> = text
        .encode_utf16()
        .skip(start as usize)
        .take((end - start) as usize)
        .collect();
    String::from_utf16_lossy(&units)
}

fn sorted_items(items: &HashMap<String, BlockReviewItem>) -> Vec<BlockReviewItem> {
    let mut sorted: Vec<BlockReviewItem> = items.values().cloned().collect();
    sorted.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    sorted
}

/// Rebuilds the doc image of a row+item state (the gateway uses pre/desired
/// pairs of these for in-place reconciliation).
pub(crate) fn doc_image(
    rows: &[BlockRow],
    items: &[BlockReviewItem],
) -> Result<Vec<Node>, QuarryError> {
    seed_session_nodes(rows, &doc_anchors(items)).map_err(session_projection_error)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        reason = "tests use unwrap for awareness fixtures"
    )]

    use super::*;
    use quarry_collab_codec::SessionProjection;
    use yrs::sync::awareness::{AwarenessUpdate, AwarenessUpdateEntry};

    /// An awareness populated with the given client states, applied the same
    /// way incoming websocket awareness messages are.
    fn awareness_with_states<const N: usize>(states: [(u64, &str); N]) -> Awareness {
        let clients = states
            .into_iter()
            .map(|(id, json)| {
                (
                    yrs::ClientID::new(id),
                    AwarenessUpdateEntry {
                        clock: 1,
                        json: json.into(),
                    },
                )
            })
            .collect();
        let mut awareness = Awareness::new(Doc::new());
        awareness.apply_update(AwarenessUpdate { clients }).unwrap();
        awareness
    }

    #[test]
    fn awareness_actor_joins_distinct_names_sorted() {
        let awareness = awareness_with_states([
            (2, r##"{"data":{"name":"Blake","color":"#f00"}}"##),
            (1, r##"{"data":{"name":"Avery","color":"#0f0"}}"##),
        ]);
        assert_eq!(awareness_actor(&awareness).as_deref(), Some("Avery, Blake"));
    }

    #[test]
    fn awareness_actor_dedupes_duplicate_names() {
        let awareness = awareness_with_states([
            (1, r#"{"data":{"name":"Avery"}}"#),
            (2, r#"{"data":{"name":"Avery"}}"#),
        ]);
        assert_eq!(awareness_actor(&awareness).as_deref(), Some("Avery"));
    }

    #[test]
    fn awareness_actor_drops_blank_and_missing_names() {
        let awareness = awareness_with_states([
            (1, r#"{"data":{"name":"  "}}"#),
            (2, r##"{"data":{"color":"#fff"}}"##),
            (3, r#"{"selection":null}"#),
            (4, r#"{"data":{"name":"Avery"}}"#),
        ]);
        assert_eq!(awareness_actor(&awareness).as_deref(), Some("Avery"));
    }

    #[test]
    fn awareness_actor_is_none_without_any_names() {
        assert_eq!(awareness_actor(&Awareness::new(Doc::new())), None);
        let blank = awareness_with_states([(1, r#"{"data":{"name":" "}}"#)]);
        assert_eq!(awareness_actor(&blank), None);
    }

    /// Adapter whole-file writes lock every document they touch; the hub must
    /// not keep one entry per document forever. Uncontended, sessionless
    /// entries are swept on the next access; an entry whose mutex is still
    /// held (an in-flight write or transaction) survives.
    #[tokio::test]
    async fn lock_document_entries_evict_once_uncontended_and_sessionless() {
        let root = tempfile::tempdir().unwrap();
        let store = quarry_storage::QuarryStore::open(quarry_storage::StoreConfig {
            db_path: root.path().join("quarry.db"),
            cas_path: root.path().join("cas"),
            lock_path: None,
        })
        .await
        .unwrap();
        let hub = SessionHub::new(store);

        let released = hub.lock_document("doc-released").await;
        drop(released);
        assert_eq!(hub.entry_count().await, 1, "entries linger until a sweep");

        // The next access sweeps doc-released; doc-held survives while its
        // guard lives.
        let _held = hub.lock_document("doc-held").await;
        assert_eq!(hub.entry_count().await, 1);
        let _third = hub.lock_document("doc-third").await;
        assert_eq!(hub.entry_count().await, 2);
    }

    fn item(id: &str, kind: BlockReviewKind, start: u32, end: u32) -> BlockReviewItem {
        BlockReviewItem {
            id: id.to_string(),
            document_id: "doc-1".to_string(),
            block_id: "b1".to_string(),
            kind,
            start_offset: start,
            end_offset: end,
            body: Some("body".to_string()),
            replacement: matches!(kind, BlockReviewKind::Suggestion)
                .then(|| "replacement".to_string()),
            author: Some("user".to_string()),
            state: BlockReviewState::Open,
            quote: Some("quote".to_string()),
            context_before: None,
            context_after: None,
            parent_item_id: None,
            created_at: "2026-06-09T00:00:00.000Z".to_string(),
            updated_at: "2026-06-09T00:00:00.000Z".to_string(),
        }
    }

    fn row(block_id: &str, text: &str) -> BlockRow {
        BlockRow {
            block_id: block_id.to_string(),
            parent_block_id: None,
            position: 0,
            block_type: "p".to_string(),
            attrs: Default::default(),
            text: text.to_string(),
            marks: Vec::new(),
            links: Vec::new(),
        }
    }

    fn anchor(id: &str, kind: SessionAnchorKind, start: u32, end: u32) -> SessionAnchor {
        SessionAnchor {
            id: id.to_string(),
            kind,
            block_id: "b1".to_string(),
            start,
            end,
        }
    }

    fn projection(rows: Vec<BlockRow>, anchors: Vec<SessionAnchor>) -> SessionProjection {
        SessionProjection {
            rows,
            anchors,
            dropped_marks: Default::default(),
        }
    }

    fn prior(items: &[BlockReviewItem]) -> HashMap<String, BlockReviewItem> {
        items
            .iter()
            .map(|item| (item.id.clone(), item.clone()))
            .collect()
    }

    const NOW: &str = "2026-06-09T01:00:00.000Z";

    #[test]
    fn extracted_anchor_updates_offsets_and_meta_resolution() {
        let comment = item("c1", BlockReviewKind::Comment, 0, 4);
        let meta = review_meta_for_items(&[BlockReviewItem {
            state: BlockReviewState::Resolved,
            ..comment.clone()
        }]);
        let items = reconcile_review_items(
            "doc-1",
            &prior(&[comment]),
            &projection(
                vec![row("b1", "XX text")],
                vec![anchor("c1", SessionAnchorKind::Comment, 2, 6)],
            ),
            &meta,
            NOW,
        );
        let updated = &items["c1"];
        assert_eq!((updated.start_offset, updated.end_offset), (2, 6));
        assert_eq!(updated.state, BlockReviewState::Resolved);
    }

    #[test]
    fn lost_marks_with_surviving_meta_orphan_comments_and_invalidate_suggestions() {
        let comment = item("c1", BlockReviewKind::Comment, 0, 4);
        let suggestion = item("s1", BlockReviewKind::Suggestion, 5, 9);
        let meta = review_meta_for_items(&[comment.clone(), suggestion.clone()]);
        let items = reconcile_review_items(
            "doc-1",
            &prior(&[comment, suggestion]),
            &projection(vec![row("b1", "shrunk")], vec![]),
            &meta,
            NOW,
        );
        assert_eq!(items["c1"].state, BlockReviewState::Orphaned);
        assert_eq!(items["s1"].state, BlockReviewState::Invalidated);
        // Clamped to the new text so the commit-time validation holds.
        assert!(items["s1"].end_offset <= 6);
    }

    #[test]
    fn deleted_meta_entry_drops_the_item_and_its_replies() {
        let root = item("c1", BlockReviewKind::Comment, 0, 4);
        let reply = BlockReviewItem {
            id: "r1".to_string(),
            parent_item_id: Some("c1".to_string()),
            ..root.clone()
        };
        let items = reconcile_review_items(
            "doc-1",
            &prior(&[root, reply]),
            &projection(vec![row("b1", "text stays")], vec![]),
            &ReviewMeta::default(),
            NOW,
        );
        assert!(items.is_empty());
    }

    #[test]
    fn browser_created_reply_threads_under_its_root() {
        let root = item("c1", BlockReviewKind::Comment, 0, 4);
        let mut meta = review_meta_for_items(std::slice::from_ref(&root));
        meta.comments.insert(
            "r1".to_string(),
            ReviewMetaEntry {
                by: "Blair".to_string(),
                at: "2026-06-09T00:30:00.000Z".to_string(),
                edited_at: None,
                body: Some("A reply".to_string()),
                re: Some("c1".to_string()),
                status: None,
                resolved: None,
            },
        );
        let items = reconcile_review_items(
            "doc-1",
            &prior(&[root]),
            &projection(
                vec![row("b1", "text stays")],
                vec![anchor("c1", SessionAnchorKind::Comment, 0, 4)],
            ),
            &meta,
            NOW,
        );
        let reply = &items["r1"];
        assert_eq!(reply.parent_item_id.as_deref(), Some("c1"));
        assert_eq!(reply.body.as_deref(), Some("A reply"));
        assert_eq!(reply.author.as_deref(), Some("Blair"));
        assert_eq!(reply.block_id, "b1");
    }

    #[test]
    fn passthrough_items_survive_untouched_and_clamped() {
        let orphaned = BlockReviewItem {
            state: BlockReviewState::Orphaned,
            start_offset: 3,
            end_offset: 3,
            ..item("c-old", BlockReviewKind::Comment, 3, 3)
        };
        let stale = BlockReviewItem {
            state: BlockReviewState::Invalidated,
            start_offset: 2,
            end_offset: 40,
            ..item("s-old", BlockReviewKind::Suggestion, 2, 40)
        };
        let items = reconcile_review_items(
            "doc-1",
            &prior(&[orphaned.clone(), stale]),
            &projection(vec![row("b1", "short")], vec![]),
            &ReviewMeta::default(),
            NOW,
        );
        assert_eq!(items["c-old"], orphaned);
        assert_eq!(items["s-old"].end_offset, 5);
    }

    #[test]
    fn insertion_suggestions_and_dead_anchors_are_not_doc_represented() {
        let insertion = BlockReviewItem {
            start_offset: 4,
            end_offset: 4,
            ..item("s-ins", BlockReviewKind::Suggestion, 4, 4)
        };
        assert!(doc_represented(&insertion));
        let block_delete = BlockReviewItem {
            start_offset: 0,
            end_offset: 0,
            replacement: None,
            ..item("s-delete", BlockReviewKind::Suggestion, 0, 0)
        };
        assert!(!doc_represented(&block_delete));
        let orphaned = BlockReviewItem {
            state: BlockReviewState::Orphaned,
            ..item("c1", BlockReviewKind::Comment, 0, 4)
        };
        assert!(!doc_represented(&orphaned));
        let resolved_comment = BlockReviewItem {
            state: BlockReviewState::Resolved,
            ..item("c2", BlockReviewKind::Comment, 0, 4)
        };
        assert!(doc_represented(&resolved_comment));
        let conflict = item("x1", BlockReviewKind::Conflict, 0, 4);
        assert!(!doc_represented(&conflict));
    }
}
