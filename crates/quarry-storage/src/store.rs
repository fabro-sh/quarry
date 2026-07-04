use crate::{
    blocks::{self, BlockMarkdownWriter},
    events::{StoreEvent, log_store_event},
};

use fs2::FileExt;
use quarry_cas::DiskCas;
use quarry_core::{QuarryError, Result};
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{ErrorKind, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::process;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{Mutex, MutexGuard, OwnedMutexGuard, broadcast};
use turso::{Builder, Connection, Database};

pub(crate) type WriteTransactionFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage busy: {source}")]
    Busy {
        #[source]
        source: turso::Error,
    },
    #[error("database error: {0}")]
    Database(#[from] turso::Error),
}

impl From<StorageError> for QuarryError {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::Busy { source } => Self::Busy(source.to_string()),
            StorageError::Database(source) => Self::StorageSource {
                source: Box::new(source),
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub db_path: PathBuf,
    pub cas_path: PathBuf,
    pub lock_path: Option<PathBuf>,
}

pub struct GlobalOperationGuard {
    _guard: OwnedMutexGuard<()>,
}

#[derive(Clone)]
pub struct QuarryStore {
    pub(crate) db: Database,
    pub(crate) cas: DiskCas,
    write_lock: Arc<Mutex<()>>,
    operation_lock: Arc<Mutex<()>>,
    event_tx: broadcast::Sender<StoreEvent>,
    /// Phase 4: the whole-file Markdown write path for BlockDocuments,
    /// installed by the serving process (quarry-server owns the single
    /// reconciliation implementation and the session mode switch). Shared
    /// across store clones. Weak: the writer itself holds store clones, so a
    /// strong ref here would cycle and keep the store (and its lock file)
    /// alive past shutdown — the installer keeps the strong handle for the
    /// serving lifetime.
    pub(crate) block_markdown_writer:
        Arc<std::sync::RwLock<std::sync::Weak<dyn BlockMarkdownWriter>>>,
    _lock_guard: Arc<LockGuard>,
}

tokio::task_local! {
    static GLOBAL_OPERATION_ACTIVE: ();
}

struct LockGuard {
    path: Option<PathBuf>,
    file: Option<File>,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
        if let Some(file) = &self.file {
            let _ = FileExt::unlock(file);
        }
    }
}

impl QuarryStore {
    pub async fn open(config: StoreConfig) -> Result<Self> {
        let started = Instant::now();
        tracing::debug!(
            event = "storage.open.started",
            db_path = %config.db_path.display(),
            cas_path = %config.cas_path.display(),
            "opening Quarry store"
        );
        if let Some(parent) = config.db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir_all(&config.cas_path)?;

        let lock_guard = acquire_lock(&config)?;
        let db_path = config.db_path.to_string_lossy().to_string();
        let db = Builder::new_local(&db_path)
            .build()
            .await
            .map_err(map_turso_error)?;
        let cas = DiskCas::open(config.cas_path)?;
        let (event_tx, _) = broadcast::channel(1024);
        let store = Self {
            db,
            cas,
            write_lock: Arc::new(Mutex::new(())),
            operation_lock: Arc::new(Mutex::new(())),
            event_tx,
            block_markdown_writer: Arc::new(std::sync::RwLock::new(std::sync::Weak::<
                blocks::NoBlockMarkdownWriter,
            >::new())),
            _lock_guard: Arc::new(lock_guard),
        };
        store.migrate().await?;
        tracing::debug!(
            event = "storage.open.completed",
            db_path = %db_path,
            cas_path = %store.cas.root().display(),
            duration_ms = started.elapsed().as_millis() as u64,
            "opened Quarry store"
        );
        Ok(store)
    }

    pub fn cas(&self) -> &DiskCas {
        &self.cas
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<StoreEvent> {
        self.event_tx.subscribe()
    }

    pub(crate) fn emit_event(&self, event: StoreEvent) {
        log_store_event(&event);
        let _ = self.event_tx.send(event);
    }

    pub async fn acquire_global_operation_lock(&self) -> GlobalOperationGuard {
        tracing::debug!(
            event = "storage.global_operation.waiting",
            "waiting for global operation lock"
        );
        let guard = self.operation_lock.clone().lock_owned().await;
        tracing::debug!(
            event = "storage.global_operation.acquired",
            "acquired global operation lock"
        );
        GlobalOperationGuard { _guard: guard }
    }

    pub(crate) async fn acquire_write_lock(&self) -> MutexGuard<'_, ()> {
        tracing::debug!(
            event = "storage.write_lock.waiting",
            "waiting for storage write lock"
        );
        let guard = self.write_lock.lock().await;
        tracing::debug!(
            event = "storage.write_lock.acquired",
            "acquired storage write lock"
        );
        guard
    }

    pub async fn run_global_operation<F, T>(&self, future: F) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        if GLOBAL_OPERATION_ACTIVE.try_with(|_| ()).is_ok() {
            return future.await;
        }
        let _guard = self.acquire_global_operation_lock().await;
        GLOBAL_OPERATION_ACTIVE.scope((), future).await
    }

    async fn normal_write_gate(&self) -> Option<GlobalOperationGuard> {
        if GLOBAL_OPERATION_ACTIVE.try_with(|_| ()).is_ok() {
            None
        } else {
            Some(self.acquire_global_operation_lock().await)
        }
    }

    pub(crate) async fn run_normal_write<F, T>(&self, future: F) -> Result<T>
    where
        F: Future<Output = Result<T>>,
    {
        let _guard = self.normal_write_gate().await;
        GLOBAL_OPERATION_ACTIVE.scope((), future).await
    }

    pub(crate) async fn write_transaction<T, F>(&self, f: F) -> Result<T>
    where
        F: for<'a> FnOnce(&'a QuarryStore, &'a Connection) -> WriteTransactionFuture<'a, T>,
    {
        let _operation_guard = self.normal_write_gate().await;
        let _guard = self.acquire_write_lock().await;
        let conn = self.conn()?;
        begin_immediate(&conn).await?;
        let result = f(self, &conn).await;
        finish_tx(&conn, result).await
    }
}

fn acquire_lock(config: &StoreConfig) -> Result<LockGuard> {
    let path = config
        .lock_path
        .clone()
        .unwrap_or_else(|| config.db_path.with_extension("lock"));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(QuarryError::Io)?;
    file.try_lock_exclusive().map_err(|err| {
        if err.kind() == ErrorKind::WouldBlock {
            QuarryError::Busy(format!(
                "another Quarry daemon appears to own {}",
                config.db_path.display()
            ))
        } else {
            QuarryError::Io(err)
        }
    })?;
    file.set_len(0)?;
    writeln!(&file, "{}", process::id())?;
    Ok(LockGuard {
        path: Some(path),
        file: Some(file),
    })
}

pub(crate) async fn begin_immediate(conn: &Connection) -> Result<()> {
    let mut delay = Duration::from_millis(5);
    for attempt in 0..6 {
        match conn.execute("BEGIN IMMEDIATE", ()).await {
            Ok(_) => return Ok(()),
            Err(err) if is_busy(&err) && attempt < 5 => {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            Err(err) => return Err(map_turso_error(err).into()),
        }
    }
    Err(QuarryError::Busy("database remained locked".to_string()))
}

pub(crate) async fn finish_tx<T>(conn: &Connection, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => {
            conn.execute("COMMIT", ()).await.map_err(map_turso_error)?;
            Ok(value)
        }
        Err(err) => {
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(err)
        }
    }
}

fn is_busy(err: &turso::Error) -> bool {
    matches!(err, turso::Error::Busy(_) | turso::Error::BusySnapshot(_))
}

pub(crate) fn map_turso_error(err: turso::Error) -> StorageError {
    if is_busy(&err) {
        StorageError::Busy { source: err }
    } else {
        StorageError::Database(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_busy_error_converts_to_branchable_quarry_error() {
        let err = QuarryError::from(StorageError::Busy {
            source: turso::Error::Busy("database is locked".to_string()),
        });

        assert!(
            matches!(err, QuarryError::Busy(message) if message.contains("database is locked"))
        );
    }
}
