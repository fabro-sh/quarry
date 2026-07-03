use crate::{AgentPresenceEntry, AgentPresenceListResponse, AgentPresenceResponse};
use quarry_core::now_timestamp;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Expiry is the only way presence entries go away. Anything that signals the
/// agent is still around refreshes the clock: a document call carrying
/// `X-Agent-Id`, a `/presence` POST, or an open document event stream (which
/// touches the entry every [`AGENT_PRESENCE_STREAM_HEARTBEAT`]).
const AGENT_PRESENCE_TTL: Duration = Duration::from_secs(60);
const AGENT_PRESENCE_STREAM_HEARTBEAT: Duration = Duration::from_secs(15);

struct AgentPresenceSlot {
    entry: AgentPresenceEntry,
    touched: tokio::time::Instant,
}

#[derive(Clone, Default)]
pub(crate) struct AgentPresenceRegistry {
    entries: Arc<Mutex<HashMap<String, AgentPresenceSlot>>>,
}

/// The scope discriminant keeps tmp entries apart from any library — even a
/// library literally named "tmp".
fn agent_presence_key(library: Option<&str>, path: &str, agent_id: &str) -> String {
    match library {
        Some(library) => format!("library\0{library}\0{path}\0{agent_id}"),
        None => format!("tmp\0{path}\0{agent_id}"),
    }
}

impl AgentPresenceRegistry {
    fn live_entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, AgentPresenceSlot>> {
        let mut entries = self.entries.lock().expect("presence lock poisoned");
        entries.retain(|_, slot| slot.touched.elapsed() <= AGENT_PRESENCE_TTL);
        entries
    }

    pub(crate) fn update(
        &self,
        library: Option<&str>,
        path: &str,
        document_id: &str,
        agent_id: String,
        status: String,
        by: Option<String>,
    ) -> AgentPresenceResponse {
        let entry = AgentPresenceEntry {
            library: library.map(str::to_string),
            path: path.to_string(),
            document_id: document_id.to_string(),
            agent_id,
            status,
            by,
            updated_at: now_timestamp(),
        };
        let key = agent_presence_key(library, path, &entry.agent_id);
        let mut entries = self.live_entries();
        entries.insert(
            key,
            AgentPresenceSlot {
                entry: entry.clone(),
                touched: tokio::time::Instant::now(),
            },
        );
        let presence = entries
            .values()
            .filter(|slot| slot.entry.library.as_deref() == library && slot.entry.path == path)
            .map(|slot| slot.entry.clone())
            .collect();
        AgentPresenceResponse {
            current: entry,
            presence,
        }
    }

    /// Refreshes an entry's TTL without changing its declared status, creating
    /// a `waiting` entry for agents that connect before posting one.
    pub(crate) fn touch(
        &self,
        library: Option<&str>,
        path: &str,
        document_id: &str,
        agent_id: &str,
    ) {
        let key = agent_presence_key(library, path, agent_id);
        let mut entries = self.live_entries();
        let slot = entries.entry(key).or_insert_with(|| AgentPresenceSlot {
            entry: AgentPresenceEntry {
                library: library.map(str::to_string),
                path: path.to_string(),
                document_id: document_id.to_string(),
                agent_id: agent_id.to_string(),
                status: "waiting".to_string(),
                by: None,
                updated_at: now_timestamp(),
            },
            touched: tokio::time::Instant::now(),
        });
        slot.entry.updated_at = now_timestamp();
        slot.touched = tokio::time::Instant::now();
    }

    pub(crate) fn list(&self, library: Option<&str>, path: &str) -> AgentPresenceListResponse {
        let presence = self
            .live_entries()
            .values()
            .filter(|slot| slot.entry.library.as_deref() == library && slot.entry.path == path)
            .map(|slot| slot.entry.clone())
            .collect();
        AgentPresenceListResponse { presence }
    }
}

/// Keeps an agent's presence fresh for as long as a document event stream
/// stays open: touches presence on connect and every heartbeat. Dropping the
/// guard only stops the heartbeat — the entry survives until
/// [`AGENT_PRESENCE_TTL`], so stream reconnects and burst readers do not flap
/// presence.
pub(crate) struct PresenceStreamGuard {
    heartbeat: tokio::task::JoinHandle<()>,
}

impl PresenceStreamGuard {
    pub(crate) fn open(
        registry: AgentPresenceRegistry,
        library: Option<String>,
        path: String,
        document_id: String,
        agent_id: String,
    ) -> Self {
        registry.touch(library.as_deref(), &path, &document_id, &agent_id);
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::time::sleep(AGENT_PRESENCE_STREAM_HEARTBEAT).await;
                registry.touch(library.as_deref(), &path, &document_id, &agent_id);
            }
        });
        Self { heartbeat }
    }
}

impl Drop for PresenceStreamGuard {
    fn drop(&mut self) {
        self.heartbeat.abort();
    }
}
