//! Per-document serialization for command commits and forks.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentSelection {
    pub client_id: String,
    pub author: String,
    pub points: Vec<quarry_document::TextPoint>,
}

type Selections = HashMap<(String, String), (DocumentSelection, std::time::Instant)>;

#[derive(Clone, Default)]
pub(crate) struct DocumentAuthority {
    locks: Arc<Mutex<HashMap<String, Weak<AsyncMutex<()>>>>>,
    selections: Arc<Mutex<Selections>>,
}

impl DocumentAuthority {
    pub async fn lock(&self, id: &str) -> OwnedMutexGuard<()> {
        let mutex = {
            let mut entries = self.locks.lock().expect("document lock registry");
            entries.retain(|_, value| value.strong_count() > 0);
            if let Some(mutex) = entries.get(id).and_then(Weak::upgrade) {
                mutex
            } else {
                let mutex = Arc::new(AsyncMutex::new(()));
                entries.insert(id.into(), Arc::downgrade(&mutex));
                mutex
            }
        };
        mutex.lock_owned().await
    }

    pub fn selections(
        &self,
        document: &str,
        update: Option<DocumentSelection>,
    ) -> Vec<DocumentSelection> {
        let mut entries = self.selections.lock().expect("document selections");
        entries.retain(|_, (_, seen)| seen.elapsed() < std::time::Duration::from_secs(30));
        if let Some(selection) = update {
            let key = (document.to_string(), selection.client_id.clone());
            if selection.points.is_empty() {
                entries.remove(&key);
            } else {
                entries.insert(key, (selection, std::time::Instant::now()));
            }
        }
        entries
            .iter()
            .filter(|((id, _), _)| id == document)
            .map(|(_, (selection, _))| selection.clone())
            .collect()
    }
}
