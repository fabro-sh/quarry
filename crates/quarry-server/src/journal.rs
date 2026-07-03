use quarry_storage::{QuarryStore, StoreEvent};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const AGENT_EVENT_JOURNAL_CAPACITY: usize = 4096;

#[derive(Clone, Default)]
pub(crate) struct AgentEventJournal {
    inner: Arc<Mutex<AgentEventJournalInner>>,
    acks: Arc<Mutex<HashMap<String, u64>>>,
    ingest_task: Arc<StdMutex<Option<JoinHandle<()>>>>,
}

#[derive(Default)]
struct AgentEventJournalInner {
    next_id: u64,
    events: VecDeque<LoggedStoreEvent>,
}

#[derive(Clone)]
pub(crate) struct LoggedStoreEvent {
    pub(crate) id: u64,
    pub(crate) event: StoreEvent,
}

impl AgentEventJournal {
    pub(crate) fn spawn_ingest(&self, store: QuarryStore, shutdown: CancellationToken) {
        let journal = self.clone();
        let mut receiver = store.subscribe_events();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => return,
                    received = receiver.recv() => {
                        match received {
                            Ok(event) => journal.push(event).await,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                tracing::warn!(
                                    event = "sse.stream.lagged",
                                    stream = "agent_event_journal",
                                    skipped,
                                    "agent event journal lagged"
                                );
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        }
                    }
                }
            }
        });
        if let Some(previous) = self
            .ingest_task
            .lock()
            .expect("agent event ingest task lock poisoned")
            .replace(task)
        {
            previous.abort();
        }
    }

    pub(crate) async fn join_ingest(&self) {
        let task = self
            .ingest_task
            .lock()
            .expect("agent event ingest task lock poisoned")
            .take();
        if let Some(task) = task {
            match task.await {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {}
                Err(error) => tracing::debug!(
                    event = "agent_event_journal.ingest_join_failed",
                    ?error,
                    "agent event journal ingest task ended with an unexpected join error"
                ),
            }
        }
    }

    async fn push(&self, event: StoreEvent) {
        let mut inner = self.inner.lock().await;
        inner.next_id = inner.next_id.saturating_add(1);
        let id = inner.next_id;
        inner.events.push_back(LoggedStoreEvent { id, event });
        while inner.events.len() > AGENT_EVENT_JOURNAL_CAPACITY {
            inner.events.pop_front();
        }
    }

    pub(crate) async fn pending_since(
        &self,
        library_id: &str,
        after: u64,
        limit: usize,
    ) -> Vec<LoggedStoreEvent> {
        let inner = self.inner.lock().await;
        inner
            .events
            .iter()
            .filter(|event| event.id > after && event.event.library_id() == library_id)
            .take(limit)
            .cloned()
            .collect()
    }

    pub(crate) async fn ack(&self, agent_id: String, event_id: u64) {
        let mut acks = self.acks.lock().await;
        let ack = acks.entry(agent_id).or_insert(0);
        *ack = (*ack).max(event_id);
    }
}
