use quarry_core::{
    normalize_path, parent_dirs, DocumentSource, QuarryError, Result, WritePrecondition,
};
use quarry_storage::{
    BlockMarkdownWrite, BlockWriteBase, DocumentKind, DocumentScopeRef, QuarryStore,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, Notify, RwLock};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FuseNodeKind {
    Directory,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuseDirEntry {
    pub name: String,
    pub kind: FuseNodeKind,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuseAttr {
    pub inode: i64,
    pub kind: FuseNodeKind,
    pub size: u64,
    pub mode: Option<u32>,
    pub mtime: Option<String>,
}

#[derive(Clone)]
pub struct FuseProjection {
    store: QuarryStore,
    library: String,
    read_only: bool,
    explicit_dirs: Arc<RwLock<BTreeSet<String>>>,
    handles: Arc<Mutex<HashMap<u64, OpenHandle>>>,
    next_handle: Arc<AtomicU64>,
    invalidation_generation: Arc<AtomicU64>,
    invalidation_notify: Arc<Notify>,
}

#[derive(Clone, Debug)]
struct OpenHandle {
    path: String,
    content: Vec<u8>,
    base_version_id: Option<String>,
    /// diff3 shadow base for Markdown documents, captured at `open()` (Phase
    /// 4): the document text this handle last saw (or last wrote). `None`
    /// for raw documents and non-UTF-8 content — those writes degrade to the
    /// two-way merge.
    base_markdown: Option<String>,
    created: bool,
    dirty: bool,
}

impl FuseProjection {
    pub async fn open(store: QuarryStore, library: &str, read_only: bool) -> Result<Self> {
        let library_record = store.get_library(library).await?;
        let library = library_record.slug;
        let library_id = library_record.id;
        let mut initial_dirs = BTreeSet::new();
        for document in store.list_documents(&library, None, Some(10_000)).await? {
            initial_dirs.extend(parent_dirs(&document.path));
        }
        for directory in store.list_directories(&library, None).await? {
            if !directory.path.is_empty() {
                initial_dirs.insert(directory.path);
            }
        }
        let invalidation_generation = Arc::new(AtomicU64::new(0));
        let invalidation_notify = Arc::new(Notify::new());
        watch_store_events(
            store.subscribe_events(),
            library_id.clone(),
            invalidation_generation.clone(),
            invalidation_notify.clone(),
        );
        Ok(Self {
            store,
            library,
            read_only,
            explicit_dirs: Arc::new(RwLock::new(initial_dirs)),
            handles: Arc::new(Mutex::new(HashMap::new())),
            next_handle: Arc::new(AtomicU64::new(1)),
            invalidation_generation,
            invalidation_notify,
        })
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub fn invalidation_generation(&self) -> u64 {
        self.invalidation_generation.load(Ordering::SeqCst)
    }

    pub async fn wait_for_invalidation_after(&self, previous: u64) {
        while self.invalidation_generation() <= previous {
            self.invalidation_notify.notified().await;
        }
    }

    pub async fn attr(&self, path: &str) -> Result<FuseAttr> {
        let path = normalize_mount_path(path)?;
        if self.directory_exists(&path).await? {
            let metadata = self.directory_metadata(&path).await?;
            return Ok(FuseAttr {
                inode: metadata
                    .as_ref()
                    .map(|metadata| metadata.inode)
                    .unwrap_or(self.store.inode_for_path(&self.library, &path).await?),
                kind: FuseNodeKind::Directory,
                size: 0,
                mode: metadata
                    .as_ref()
                    .and_then(|metadata| metadata.mode)
                    .map(|mode| mode as u32),
                mtime: metadata.map(|metadata| metadata.mtime),
            });
        }
        let document = self.store.head_document(&self.library, &path).await?;
        Ok(FuseAttr {
            inode: self.store.inode_for_path(&self.library, &path).await?,
            kind: FuseNodeKind::File,
            size: document.byte_size,
            mode: None,
            mtime: None,
        })
    }

    pub async fn list_dir(&self, path: &str) -> Result<Vec<FuseDirEntry>> {
        let path = normalize_mount_path(path)?;
        if !self.directory_exists(&path).await? {
            return Err(QuarryError::NotFound(path));
        }

        let prefix = if path.is_empty() {
            None
        } else {
            Some(format!("{path}/"))
        };
        let mut entries = BTreeMap::<String, FuseDirEntry>::new();
        for document in self
            .store
            .list_documents(&self.library, prefix.as_deref(), Some(10_000))
            .await?
        {
            let remainder = if let Some(prefix) = &prefix {
                document.path.strip_prefix(prefix).unwrap_or(&document.path)
            } else {
                &document.path
            };
            if remainder.is_empty() {
                continue;
            }
            if let Some((dir, _)) = remainder.split_once('/') {
                entries.entry(dir.to_string()).or_insert(FuseDirEntry {
                    name: dir.to_string(),
                    kind: FuseNodeKind::Directory,
                    size: 0,
                });
            } else {
                entries.insert(
                    remainder.to_string(),
                    FuseDirEntry {
                        name: remainder.to_string(),
                        kind: FuseNodeKind::File,
                        size: document.byte_size,
                    },
                );
            }
        }

        let dirs = self.explicit_dirs.read().await;
        for dir in dirs.iter() {
            let Some(name) = child_name(&path, dir) else {
                continue;
            };
            entries.entry(name.to_string()).or_insert(FuseDirEntry {
                name: name.to_string(),
                kind: FuseNodeKind::Directory,
                size: 0,
            });
        }
        drop(dirs);

        for dir in self
            .store
            .list_directories(&self.library, prefix.as_deref())
            .await?
        {
            let Some(name) = child_name(&path, &dir.path) else {
                continue;
            };
            entries.entry(name.to_string()).or_insert(FuseDirEntry {
                name: name.to_string(),
                kind: FuseNodeKind::Directory,
                size: 0,
            });
        }

        Ok(entries.into_values().collect())
    }

    pub async fn read_file(&self, path: &str, offset: u64, size: u32) -> Result<Vec<u8>> {
        let path = normalize_mount_path(path)?;
        let document = self.store.get_document(&self.library, &path).await?;
        Ok(read_slice(&document.content, offset, size))
    }

    pub async fn create_file(&self, path: &str) -> Result<u64> {
        self.ensure_writable()?;
        let path = normalize_mount_path(path)?;
        self.ensure_parent_dir(&path).await?;
        if self.path_exists(&path).await? {
            return Err(QuarryError::Conflict(format!("{path} already exists")));
        }
        let content_type = content_type_for_path(&path);
        let version_id = if is_block_document(&path, &content_type) {
            // Markdown creation routes through the Phase 4 reconciled write
            // (first import), so the block projection exists from byte one.
            self.store
                .write_block_markdown(self.block_write(&path, String::new(), None, None))
                .await?
                .outcome
                .version
                .id
        } else {
            self.store
                .put_document(
                    &self.library,
                    &path,
                    Vec::new(),
                    serde_json::json!({ "content_type": content_type }),
                    &content_type,
                    DocumentSource::Fuse,
                    WritePrecondition::IfNoneMatch,
                )
                .await?
                .version
                .id
        };
        self.remember_parent_dirs(&path).await;
        self.insert_handle(OpenHandle {
            path,
            content: Vec::new(),
            base_version_id: Some(version_id),
            base_markdown: Some(String::new()),
            created: false,
            dirty: false,
        })
        .await
    }

    pub async fn open_file_for_write(&self, path: &str) -> Result<u64> {
        self.ensure_writable()?;
        let path = normalize_mount_path(path)?;
        let document = self.store.get_document(&self.library, &path).await?;
        let base_markdown = String::from_utf8(document.content.clone()).ok();
        self.insert_handle(OpenHandle {
            path,
            content: document.content,
            base_version_id: Some(document.version.id),
            base_markdown,
            created: false,
            dirty: false,
        })
        .await
    }

    pub async fn open_file_for_write_truncating(&self, path: &str) -> Result<u64> {
        self.ensure_writable()?;
        let path = normalize_mount_path(path)?;
        let document = self.store.get_document(&self.library, &path).await?;
        // The base is what the writer last SAW (the pre-truncation content),
        // even though the handle starts empty.
        let base_markdown = String::from_utf8(document.content).ok();
        self.insert_handle(OpenHandle {
            path,
            content: Vec::new(),
            base_version_id: Some(document.version.id),
            base_markdown,
            created: false,
            dirty: true,
        })
        .await
    }

    pub async fn read_handle(&self, handle_id: u64, offset: u64, size: u32) -> Result<Vec<u8>> {
        let handles = self.handles.lock().await;
        let handle = handles
            .get(&handle_id)
            .ok_or_else(|| QuarryError::NotFound(format!("file handle {handle_id}")))?;
        Ok(read_slice(&handle.content, offset, size))
    }

    pub async fn write_handle(&self, handle_id: u64, offset: u64, data: &[u8]) -> Result<usize> {
        self.ensure_writable()?;
        let mut handles = self.handles.lock().await;
        let handle = handles
            .get_mut(&handle_id)
            .ok_or_else(|| QuarryError::NotFound(format!("file handle {handle_id}")))?;
        let offset = usize::try_from(offset)
            .map_err(|_| QuarryError::InvalidPath("write offset too large".to_string()))?;
        if handle.content.len() < offset {
            handle.content.resize(offset, 0);
        }
        let required_len = offset.saturating_add(data.len());
        if handle.content.len() < required_len {
            handle.content.resize(required_len, 0);
        }
        handle.content[offset..offset + data.len()].copy_from_slice(data);
        handle.dirty = true;
        Ok(data.len())
    }

    pub async fn set_handle_len(&self, handle_id: u64, size: u64) -> Result<()> {
        self.ensure_writable()?;
        let mut handles = self.handles.lock().await;
        let handle = handles
            .get_mut(&handle_id)
            .ok_or_else(|| QuarryError::NotFound(format!("file handle {handle_id}")))?;
        handle.content.resize(
            usize::try_from(size)
                .map_err(|_| QuarryError::InvalidPath("file size too large".to_string()))?,
            0,
        );
        handle.dirty = true;
        Ok(())
    }

    pub async fn flush_handle(&self, handle_id: u64) -> Result<()> {
        let snapshot = {
            let handles = self.handles.lock().await;
            let Some(handle) = handles.get(&handle_id) else {
                return Ok(());
            };
            handle.clone()
        };
        if !snapshot.dirty {
            return Ok(());
        }
        let outcome = self.commit_handle(&snapshot).await?;
        let mut handles = self.handles.lock().await;
        if let Some(handle) = handles.get_mut(&handle_id) {
            handle.base_version_id = Some(outcome.version.id);
            // The next diff3 base is what this handle just wrote (its own
            // view); canonical-side divergence stays mergeable.
            handle.base_markdown = String::from_utf8(snapshot.content.clone()).ok();
            handle.created = false;
            handle.dirty = false;
        }
        Ok(())
    }

    pub async fn release_handle(&self, handle_id: u64) -> Result<()> {
        let handle = {
            let mut handles = self.handles.lock().await;
            let Some(handle) = handles.remove(&handle_id) else {
                return Ok(());
            };
            handle
        };
        if handle.dirty {
            self.commit_handle(&handle).await?;
        }
        Ok(())
    }

    pub async fn mkdir(&self, path: &str) -> Result<()> {
        self.mkdir_with_optional_mode(path, None).await
    }

    pub async fn mkdir_with_mode(&self, path: &str, mode: u32) -> Result<()> {
        self.mkdir_with_optional_mode(path, Some(mode)).await
    }

    async fn mkdir_with_optional_mode(&self, path: &str, mode: Option<u32>) -> Result<()> {
        self.ensure_writable()?;
        let path = normalize_mount_path(path)?;
        if path.is_empty() {
            return Err(QuarryError::Conflict(
                "root directory already exists".to_string(),
            ));
        }
        self.ensure_parent_dir(&path).await?;
        if self.path_exists(&path).await? {
            return Err(QuarryError::Conflict(format!("{path} already exists")));
        }
        self.store
            .ensure_directory(&self.library, &path, None)
            .await?;
        if let Some(mode) = mode {
            self.store
                .update_directory_metadata(
                    &self.library,
                    &path,
                    Some(i64::from(mode)),
                    None,
                    DocumentSource::Fuse,
                )
                .await?;
        }
        self.explicit_dirs.write().await.insert(path);
        Ok(())
    }

    pub async fn set_directory_metadata(
        &self,
        path: &str,
        mode: Option<u32>,
        mtime: Option<&str>,
    ) -> Result<()> {
        self.ensure_writable()?;
        let path = normalize_mount_path(path)?;
        if path.is_empty() {
            return Err(QuarryError::Conflict(
                "cannot update mount root metadata".to_string(),
            ));
        }
        if !self.directory_exists(&path).await? {
            return Err(QuarryError::NotFound(path));
        }
        self.store
            .update_directory_metadata(
                &self.library,
                &path,
                mode.map(i64::from),
                mtime,
                DocumentSource::Fuse,
            )
            .await
            .map(|_| ())
    }

    pub async fn rename(&self, from_path: &str, to_path: &str) -> Result<()> {
        self.ensure_writable()?;
        let from_path = normalize_mount_path(from_path)?;
        let to_path = normalize_mount_path(to_path)?;
        if from_path.is_empty() || to_path.is_empty() {
            return Err(QuarryError::InvalidPath(
                "cannot rename mount root".to_string(),
            ));
        }
        self.ensure_parent_dir(&to_path).await?;

        if self
            .store
            .head_document(&self.library, &from_path)
            .await
            .is_ok()
        {
            if self.directory_exists(&to_path).await? {
                return Err(QuarryError::IsADirectory(to_path));
            }
            let target_exists = self
                .store
                .head_document(&self.library, &to_path)
                .await
                .is_ok();
            if target_exists && is_block_document(&to_path, &content_type_for_path(&to_path)) {
                // The atomic-save pattern (vim/sed -i/emacs: write a temp
                // file, rename it over the document) is a WHOLE-FILE WRITE
                // to the TARGET document, not a replacement: the temp file's
                // content reconciles through the Phase 4 writer, preserving
                // the target's document id, block ids, review anchors, and
                // any live session; then the temp document is removed.
                // There is no open handle on the target, so no captured
                // base: the merge is the two-way degenerate case (base =
                // current canonical), the same contract as the CLI. A temp
                // file that is not UTF-8 is a content error (the target is
                // a markdown document), surfaced as an errno — never a
                // silent byte replacement that would destroy the projection.
                let source = self.store.get_document(&self.library, &from_path).await?;
                let markdown = String::from_utf8(source.content).map_err(|_| {
                    QuarryError::InvalidInput(format!(
                        "{to_path} is a markdown document; renaming {from_path} over it \
                         requires valid UTF-8 content"
                    ))
                })?;
                self.store
                    .write_block_markdown(self.block_write(&to_path, markdown, None, None))
                    .await?;
                return self
                    .store
                    .delete_document(&self.library, &from_path, DocumentSource::Fuse)
                    .await
                    .map(|_| ());
            }
            if target_exists {
                return self
                    .store
                    .replace_document(&self.library, &from_path, &to_path, DocumentSource::Fuse)
                    .await
                    .map(|_| ());
            }
            return self
                .store
                .move_document(&self.library, &from_path, &to_path, DocumentSource::Fuse)
                .await
                .map(|_| ());
        }

        if self.path_exists(&to_path).await? {
            return Err(QuarryError::Conflict(format!("{to_path} already exists")));
        }
        if !self.directory_exists(&from_path).await? {
            return Err(QuarryError::NotFound(from_path));
        }

        let from_prefix = format!("{from_path}/");
        for document in self
            .store
            .list_documents(&self.library, Some(&from_prefix), Some(10_000))
            .await?
        {
            let suffix = document.path.strip_prefix(&from_prefix).unwrap();
            self.store
                .move_document(
                    &self.library,
                    &document.path,
                    &format!("{to_path}/{suffix}"),
                    DocumentSource::Fuse,
                )
                .await?;
        }

        let mut dirs = self.explicit_dirs.write().await;
        let moved_dirs: Vec<String> = dirs
            .iter()
            .filter(|dir| *dir == &from_path || dir.starts_with(&from_prefix))
            .cloned()
            .collect();
        for dir in &moved_dirs {
            dirs.remove(dir);
        }
        for dir in moved_dirs {
            let suffix = dir.strip_prefix(&from_path).unwrap_or("");
            let suffix = suffix.trim_start_matches('/');
            dirs.insert(if suffix.is_empty() {
                to_path.clone()
            } else {
                format!("{to_path}/{suffix}")
            });
        }
        drop(dirs);
        self.store
            .move_directory(&self.library, &from_path, &to_path, DocumentSource::Fuse)
            .await?;
        Ok(())
    }

    pub async fn unlink(&self, path: &str) -> Result<()> {
        self.ensure_writable()?;
        let path = normalize_mount_path(path)?;
        if self.directory_exists(&path).await? {
            return Err(QuarryError::IsADirectory(path));
        }
        self.store
            .delete_document(&self.library, &path, DocumentSource::Fuse)
            .await
            .map(|_| ())
    }

    pub async fn rmdir(&self, path: &str) -> Result<()> {
        self.ensure_writable()?;
        let path = normalize_mount_path(path)?;
        if path.is_empty() {
            return Err(QuarryError::Conflict(
                "cannot remove mount root".to_string(),
            ));
        }
        if !self.directory_exists(&path).await? {
            return Err(QuarryError::NotFound(path));
        }
        if !self.list_dir(&path).await?.is_empty() {
            return Err(QuarryError::DirectoryNotEmpty(path));
        }
        self.store.remove_directory(&self.library, &path).await?;
        self.explicit_dirs.write().await.remove(&path);
        Ok(())
    }

    pub async fn set_len(&self, path: &str, size: u64) -> Result<()> {
        self.ensure_writable()?;
        let path = normalize_mount_path(path)?;
        let mut document = self.store.get_document(&self.library, &path).await?;
        let base_markdown = String::from_utf8(document.content.clone()).ok();
        document.content.resize(
            usize::try_from(size)
                .map_err(|_| QuarryError::InvalidPath("file size too large".to_string()))?,
            0,
        );
        if is_block_document(&path, &document.version.content_type) {
            // A path-level truncate is a whole-file write of the resized
            // text; a cut landing inside a UTF-8 sequence is a content error.
            let markdown = String::from_utf8(document.content).map_err(|_| {
                QuarryError::InvalidInput(format!("truncating {path} would split a UTF-8 sequence"))
            })?;
            self.store
                .write_block_markdown(self.block_write(
                    &path,
                    markdown,
                    base_markdown,
                    Some(document.version.id),
                ))
                .await
                .map(|_| ())
        } else {
            self.store
                .put_document(
                    &self.library,
                    &path,
                    document.content,
                    document.metadata,
                    &document.version.content_type,
                    DocumentSource::Fuse,
                    WritePrecondition::IfMatch(document.version.id),
                )
                .await
                .map(|_| ())
        }
    }

    async fn insert_handle(&self, handle: OpenHandle) -> Result<u64> {
        let handle_id = self.next_handle.fetch_add(1, Ordering::Relaxed);
        self.handles.lock().await.insert(handle_id, handle);
        Ok(handle_id)
    }

    async fn remember_parent_dirs(&self, path: &str) {
        let mut dirs = self.explicit_dirs.write().await;
        dirs.extend(parent_dirs(path));
    }

    /// Builds the Phase 4 reconciled-write request for a Markdown path.
    fn block_write(
        &self,
        path: &str,
        markdown: String,
        base_markdown: Option<String>,
        base_version_id: Option<String>,
    ) -> BlockMarkdownWrite {
        let content_type = content_type_for_path(path);
        BlockMarkdownWrite {
            scope: DocumentScopeRef::library(&self.library),
            path: path.to_string(),
            markdown,
            metadata: serde_json::json!({ "content_type": content_type }),
            base: match base_markdown {
                Some(markdown) => BlockWriteBase::Markdown {
                    markdown,
                    version_id: base_version_id,
                },
                None => BlockWriteBase::CurrentCanonical,
            },
            source: DocumentSource::Fuse,
            surface: "fuse".to_string(),
            actor_label: Some("FUSE write".to_string()),
        }
    }

    async fn commit_handle(&self, handle: &OpenHandle) -> Result<quarry_core::WriteOutcome> {
        let content_type = content_type_for_path(&handle.path);
        let started = Instant::now();
        tracing::debug!(
            event = "fuse.write.started",
            library = %self.library,
            path = %handle.path,
            content_type,
            content_bytes = handle.content.len(),
            created = handle.created,
            "FUSE write started"
        );
        let outcome = if is_block_document(&handle.path, &content_type) {
            // Markdown flushes reconcile via diff3 against the handle's base
            // (Phase 4): merges never fail; non-UTF-8 bytes or CriticMarkup
            // are content errors that surface as an errno.
            let markdown = String::from_utf8(handle.content.clone()).map_err(|_| {
                QuarryError::InvalidInput(format!(
                    "{} is a markdown document; FUSE writes must be valid UTF-8",
                    handle.path
                ))
            })?;
            self.store
                .write_block_markdown(self.block_write(
                    &handle.path,
                    markdown,
                    handle.base_markdown.clone(),
                    handle.base_version_id.clone(),
                ))
                .await?
                .outcome
        } else {
            let precondition = if handle.created {
                WritePrecondition::IfNoneMatch
            } else if let Some(version_id) = &handle.base_version_id {
                WritePrecondition::IfMatch(version_id.clone())
            } else {
                WritePrecondition::None
            };
            self.store
                .put_document(
                    &self.library,
                    &handle.path,
                    handle.content.clone(),
                    serde_json::json!({ "content_type": content_type }),
                    &content_type,
                    DocumentSource::Fuse,
                    precondition,
                )
                .await?
        };
        self.remember_parent_dirs(&handle.path).await;
        tracing::debug!(
            event = "fuse.write.published",
            library = %self.library,
            library_id = %outcome.transaction.library_id,
            path = %outcome.document.path,
            tx_id = %outcome.transaction.id,
            doc_id = %outcome.document.id,
            version_id = %outcome.version.id,
            content_bytes = outcome.version.byte_size,
            duration_ms = started.elapsed().as_millis() as u64,
            "FUSE write published"
        );
        Ok(outcome)
    }

    fn ensure_writable(&self) -> Result<()> {
        if self.read_only {
            return Err(QuarryError::ReadOnly("FUSE mount".to_string()));
        }
        Ok(())
    }

    async fn ensure_parent_dir(&self, path: &str) -> Result<()> {
        let parent = parent_path(path);
        if self.directory_exists(parent).await? {
            Ok(())
        } else {
            Err(QuarryError::NotFound(parent.to_string()))
        }
    }

    async fn path_exists(&self, path: &str) -> Result<bool> {
        if self.directory_exists(path).await? {
            return Ok(true);
        }
        match self.store.head_document(&self.library, path).await {
            Ok(_) => Ok(true),
            Err(QuarryError::NotFound(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn directory_exists(&self, path: &str) -> Result<bool> {
        if path.is_empty() {
            return Ok(true);
        }
        let dirs = self.explicit_dirs.read().await;
        if dirs.contains(path) {
            return Ok(true);
        }
        let prefix = format!("{path}/");
        if dirs.iter().any(|dir| dir.starts_with(&prefix)) {
            return Ok(true);
        }
        drop(dirs);
        Ok(!self
            .store
            .list_documents(&self.library, Some(&prefix), Some(1))
            .await?
            .is_empty())
    }

    async fn directory_metadata(
        &self,
        path: &str,
    ) -> Result<Option<quarry_storage::DirectoryMetadata>> {
        if path.is_empty() {
            return Ok(None);
        }
        Ok(self
            .store
            .list_directories(&self.library, Some(path))
            .await?
            .into_iter()
            .find(|metadata| metadata.path == path))
    }
}

pub async fn mount_library_with_shutdown<F>(
    store: QuarryStore,
    library: &str,
    mountpoint: &Path,
    read_only: bool,
    shutdown: F,
) -> Result<()>
where
    F: Future<Output = ()>,
{
    #[cfg(target_os = "linux")]
    {
        linux_mount::mount_library_with_shutdown(store, library, mountpoint, read_only, shutdown)
            .await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, library, mountpoint, read_only, shutdown);
        Err(QuarryError::Unsupported(
            "Quarry phase-one FUSE mounts are Linux-only".to_string(),
        ))
    }
}

fn watch_store_events(
    mut events: tokio::sync::broadcast::Receiver<quarry_storage::StoreEvent>,
    library_id: String,
    invalidation_generation: Arc<AtomicU64>,
    invalidation_notify: Arc<Notify>,
) {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) if event.library_id == library_id => {
                    let generation = invalidation_generation.fetch_add(1, Ordering::SeqCst) + 1;
                    invalidation_notify.notify_waiters();
                    tracing::debug!(
                        event = "fuse.invalidate.received",
                        library_id = %library_id,
                        path = event.path.as_deref().unwrap_or(""),
                        new_path = event.new_path.as_deref().unwrap_or(""),
                        tx_id = event.tx_id.as_deref().unwrap_or(""),
                        doc_id = event.doc_id.as_deref().unwrap_or(""),
                        version_id = event.version_id.as_deref().unwrap_or(""),
                        generation,
                        "FUSE invalidation received"
                    );
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    let generation = invalidation_generation.fetch_add(1, Ordering::SeqCst) + 1;
                    invalidation_notify.notify_waiters();
                    tracing::warn!(
                        event = "fuse.invalidate.lagged",
                        library_id = %library_id,
                        skipped,
                        generation,
                        "FUSE invalidation stream lagged"
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn normalize_mount_path(path: &str) -> Result<String> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    normalize_path(trimmed)
}

fn parent_path(path: &str) -> &str {
    path.rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("")
}

fn child_name<'a>(parent: &str, child: &'a str) -> Option<&'a str> {
    if parent.is_empty() {
        return child.split_once('/').map(|(name, _)| name).or(Some(child));
    }
    let prefix = format!("{parent}/");
    let remainder = child.strip_prefix(&prefix)?;
    if remainder.is_empty() {
        return None;
    }
    Some(
        remainder
            .split_once('/')
            .map(|(name, _)| name)
            .unwrap_or(remainder),
    )
}

fn read_slice(content: &[u8], offset: u64, size: u32) -> Vec<u8> {
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    if offset >= content.len() {
        return Vec::new();
    }
    let end = offset.saturating_add(size as usize).min(content.len());
    content[offset..end].to_vec()
}

fn is_block_document(path: &str, content_type: &str) -> bool {
    quarry_storage::document_kind(path, content_type) == DocumentKind::BlockDocument
}

fn content_type_for_path(path: &str) -> String {
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_string()
}

#[cfg(target_os = "linux")]
mod linux_mount {
    use super::{normalize_mount_path, FuseAttr, FuseNodeKind, FuseProjection};
    use bytes::Bytes;
    use fuse3::raw::prelude::*;
    use fuse3::{Errno, Inode, MountOptions, Timestamp};
    use futures_util::stream;
    use quarry_core::{QuarryError, Result};
    use quarry_storage::QuarryStore;
    use std::ffi::{OsStr, OsString};
    use std::future::Future;
    use std::num::NonZeroU32;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    const TTL: Duration = Duration::from_secs(0);
    const MAX_WRITE_BYTES: NonZeroU32 =
        NonZeroU32::new(1_024 * 1_024).expect("FUSE max_write constant is non-zero");

    pub async fn mount_library_with_shutdown<F>(
        store: QuarryStore,
        library: &str,
        mountpoint: &Path,
        read_only: bool,
        shutdown: F,
    ) -> Result<()>
    where
        F: Future<Output = ()>,
    {
        tokio::fs::create_dir_all(mountpoint).await?;
        let projection = FuseProjection::open(store, library, read_only).await?;
        let mut options = MountOptions::default();
        options
            .fs_name(format!("quarry-{library}"))
            .read_only(read_only)
            .default_permissions(false);

        tracing::info!(
            event = "fuse.mount.started",
            library,
            mountpoint = %mountpoint.display(),
            read_only,
            "FUSE mount started"
        );
        let handle = match Session::new(options.clone())
            .mount_with_unprivileged(projection.clone(), mountpoint)
            .await
        {
            Ok(handle) => handle,
            Err(unprivileged_error) => Session::new(options)
                .mount(projection, mountpoint)
                .await
                .map_err(|privileged_error| {
                    QuarryError::Unsupported(format!(
                        "failed to mount FUSE with fusermount3 ({unprivileged_error}) or privileged mount ({privileged_error})"
                    ))
                })?,
        };

        tracing::info!(
            event = "fuse.mount.established",
            library,
            mountpoint = %mountpoint.display(),
            read_only,
            "FUSE mount established"
        );
        shutdown.await;
        handle.unmount().await?;
        tracing::info!(
            event = "fuse.mount.unmounted",
            library,
            mountpoint = %mountpoint.display(),
            "FUSE mount unmounted"
        );
        Ok(())
    }

    impl Filesystem for FuseProjection {
        async fn init(&self, _req: Request) -> fuse3::Result<ReplyInit> {
            Ok(ReplyInit {
                max_write: MAX_WRITE_BYTES,
            })
        }

        async fn destroy(&self, _req: Request) {}

        async fn lookup(
            &self,
            _req: Request,
            parent: Inode,
            name: &OsStr,
        ) -> fuse3::Result<ReplyEntry> {
            let path = self.child_path(parent, name).await?;
            let attr = self.attr(&path).await.map_err(to_errno)?;
            Ok(ReplyEntry {
                ttl: TTL,
                attr: to_fuse_attr(attr, self.read_only()),
                generation: 0,
            })
        }

        async fn getattr(
            &self,
            _req: Request,
            inode: Inode,
            _fh: Option<u64>,
            _flags: u32,
        ) -> fuse3::Result<ReplyAttr> {
            let path = self.path_for_inode(inode).await?;
            let attr = self.attr(&path).await.map_err(to_errno)?;
            Ok(ReplyAttr {
                ttl: TTL,
                attr: to_fuse_attr(attr, self.read_only()),
            })
        }

        async fn setattr(
            &self,
            _req: Request,
            inode: Inode,
            fh: Option<u64>,
            set_attr: SetAttr,
        ) -> fuse3::Result<ReplyAttr> {
            let path = self.path_for_inode(inode).await?;
            if let Some(size) = set_attr.size {
                if let Some(fh) = fh {
                    self.set_handle_len(fh, size).await.map_err(to_errno)?;
                } else {
                    self.set_len(&path, size).await.map_err(to_errno)?;
                }
            }
            let mode = set_attr.mode.map(|mode| mode & 0o7777);
            let mtime = set_attr.mtime.map(timestamp_to_rfc3339);
            if mode.is_some() || mtime.is_some() {
                let attr = self.attr(&path).await.map_err(to_errno)?;
                if matches!(attr.kind, FuseNodeKind::Directory) {
                    self.set_directory_metadata(&path, mode, mtime.as_deref())
                        .await
                        .map_err(to_errno)?;
                }
            }
            let attr = self.attr(&path).await.map_err(to_errno)?;
            Ok(ReplyAttr {
                ttl: TTL,
                attr: to_fuse_attr(attr, self.read_only()),
            })
        }

        async fn mkdir(
            &self,
            _req: Request,
            parent: Inode,
            name: &OsStr,
            mode: u32,
            umask: u32,
        ) -> fuse3::Result<ReplyEntry> {
            let path = self.child_path(parent, name).await?;
            self.mkdir_with_mode(&path, mode & !umask & 0o7777)
                .await
                .map_err(to_errno)?;
            let attr = self.attr(&path).await.map_err(to_errno)?;
            Ok(ReplyEntry {
                ttl: TTL,
                attr: to_fuse_attr(attr, self.read_only()),
                generation: 0,
            })
        }

        async fn unlink(&self, _req: Request, parent: Inode, name: &OsStr) -> fuse3::Result<()> {
            let path = self.child_path(parent, name).await?;
            self.unlink(&path).await.map_err(to_errno)
        }

        async fn rmdir(&self, _req: Request, parent: Inode, name: &OsStr) -> fuse3::Result<()> {
            let path = self.child_path(parent, name).await?;
            self.rmdir(&path).await.map_err(to_errno)
        }

        async fn rename(
            &self,
            _req: Request,
            origin_parent: Inode,
            origin_name: &OsStr,
            parent: Inode,
            name: &OsStr,
        ) -> fuse3::Result<()> {
            let from_path = self.child_path(origin_parent, origin_name).await?;
            let to_path = self.child_path(parent, name).await?;
            self.rename(&from_path, &to_path).await.map_err(to_errno)
        }

        async fn open(&self, _req: Request, inode: Inode, flags: u32) -> fuse3::Result<ReplyOpen> {
            let path = self.path_for_inode(inode).await?;
            let accmode = flags & libc::O_ACCMODE as u32;
            if accmode == libc::O_RDONLY as u32 {
                self.attr(&path).await.map_err(to_errno)?;
                return Ok(ReplyOpen { fh: 0, flags: 0 });
            }
            let fh = if flags & libc::O_TRUNC as u32 != 0 {
                self.open_file_for_write_truncating(&path)
                    .await
                    .map_err(to_errno)?
            } else {
                self.open_file_for_write(&path).await.map_err(to_errno)?
            };
            Ok(ReplyOpen { fh, flags: 0 })
        }

        async fn read(
            &self,
            _req: Request,
            inode: Inode,
            fh: u64,
            offset: u64,
            size: u32,
        ) -> fuse3::Result<ReplyData> {
            let data = if fh == 0 {
                let path = self.path_for_inode(inode).await?;
                self.read_file(&path, offset, size)
                    .await
                    .map_err(to_errno)?
            } else {
                self.read_handle(fh, offset, size).await.map_err(to_errno)?
            };
            Ok(ReplyData {
                data: Bytes::from(data),
            })
        }

        async fn write(
            &self,
            _req: Request,
            _inode: Inode,
            fh: u64,
            offset: u64,
            data: &[u8],
            _write_flags: u32,
            _flags: u32,
        ) -> fuse3::Result<ReplyWrite> {
            let written = self
                .write_handle(fh, offset, data)
                .await
                .map_err(to_errno)?;
            Ok(ReplyWrite {
                written: written as u32,
            })
        }

        async fn flush(
            &self,
            _req: Request,
            _inode: Inode,
            fh: u64,
            _lock_owner: u64,
        ) -> fuse3::Result<()> {
            if fh == 0 {
                return Ok(());
            }
            self.flush_handle(fh).await.map_err(to_errno)
        }

        async fn release(
            &self,
            _req: Request,
            _inode: Inode,
            fh: u64,
            _flags: u32,
            _lock_owner: u64,
            _flush: bool,
        ) -> fuse3::Result<()> {
            if fh == 0 {
                return Ok(());
            }
            self.release_handle(fh).await.map_err(to_errno)
        }

        async fn opendir(
            &self,
            _req: Request,
            inode: Inode,
            _flags: u32,
        ) -> fuse3::Result<ReplyOpen> {
            let path = self.path_for_inode(inode).await?;
            self.attr(&path).await.map_err(to_errno)?;
            Ok(ReplyOpen { fh: 0, flags: 0 })
        }

        async fn readdir<'a>(
            &'a self,
            _req: Request,
            inode: Inode,
            _fh: u64,
            offset: i64,
        ) -> fuse3::Result<
            ReplyDirectory<
                impl futures_util::Stream<Item = fuse3::Result<DirectoryEntry>> + Send + 'a,
            >,
        > {
            let entries = self
                .directory_entries(inode, offset)
                .await?
                .into_iter()
                .map(|(entry, _)| Ok(entry))
                .collect::<Vec<_>>();
            Ok(ReplyDirectory {
                entries: stream::iter(entries),
            })
        }

        async fn readdirplus<'a>(
            &'a self,
            _req: Request,
            inode: Inode,
            _fh: u64,
            offset: u64,
            _lock_owner: u64,
        ) -> fuse3::Result<
            ReplyDirectoryPlus<
                impl futures_util::Stream<Item = fuse3::Result<DirectoryEntryPlus>> + Send + 'a,
            >,
        > {
            let entries = self
                .directory_entries(inode, i64::try_from(offset).unwrap_or(i64::MAX))
                .await?
                .into_iter()
                .map(|(entry, attr)| {
                    Ok(DirectoryEntryPlus {
                        inode: entry.inode,
                        generation: 0,
                        kind: entry.kind,
                        name: entry.name,
                        offset: entry.offset,
                        attr,
                        entry_ttl: TTL,
                        attr_ttl: TTL,
                    })
                })
                .collect::<Vec<_>>();
            Ok(ReplyDirectoryPlus {
                entries: stream::iter(entries),
            })
        }

        async fn create(
            &self,
            _req: Request,
            parent: Inode,
            name: &OsStr,
            _mode: u32,
            _flags: u32,
        ) -> fuse3::Result<ReplyCreated> {
            let path = self.child_path(parent, name).await?;
            let fh = self.create_file(&path).await.map_err(to_errno)?;
            let attr = self.attr(&path).await.map_err(to_errno)?;
            Ok(ReplyCreated {
                ttl: TTL,
                attr: to_fuse_attr(attr, self.read_only()),
                generation: 0,
                fh,
                flags: 0,
            })
        }

        async fn access(&self, _req: Request, inode: Inode, _mask: u32) -> fuse3::Result<()> {
            let path = self.path_for_inode(inode).await?;
            self.attr(&path).await.map_err(to_errno)?;
            Ok(())
        }

        async fn statfs(&self, _req: Request, _inode: Inode) -> fuse3::Result<ReplyStatFs> {
            Ok(ReplyStatFs {
                blocks: 0,
                bfree: 0,
                bavail: 0,
                files: 0,
                ffree: 0,
                bsize: 4096,
                namelen: 255,
                frsize: 4096,
            })
        }
    }

    impl FuseProjection {
        async fn path_for_inode(&self, inode: Inode) -> fuse3::Result<String> {
            let inode = i64::try_from(inode).map_err(|_| Errno::from(libc::EINVAL))?;
            self.store
                .path_for_inode(&self.library, inode)
                .await
                .map_err(to_errno)
        }

        async fn child_path(&self, parent: Inode, name: &OsStr) -> fuse3::Result<String> {
            let parent = self.path_for_inode(parent).await?;
            join_child_path(&parent, name)
        }

        async fn parent_inode(&self, path: &str) -> fuse3::Result<Inode> {
            if path.is_empty() {
                return Ok(1);
            }
            let parent = super::parent_path(path);
            let inode = self
                .store
                .inode_for_path(&self.library, parent)
                .await
                .map_err(to_errno)?;
            u64::try_from(inode).map_err(|_| Errno::from(libc::EIO))
        }

        async fn directory_entries(
            &self,
            inode: Inode,
            offset: i64,
        ) -> fuse3::Result<Vec<(DirectoryEntry, FileAttr)>> {
            let path = self.path_for_inode(inode).await?;
            let parent_path = if path.is_empty() {
                ""
            } else {
                super::parent_path(&path)
            };
            let parent_inode = self.parent_inode(&path).await?;
            let current_attr =
                to_fuse_attr(self.attr(&path).await.map_err(to_errno)?, self.read_only());
            let parent_attr = to_fuse_attr(
                self.attr(parent_path).await.map_err(to_errno)?,
                self.read_only(),
            );
            let mut entries = vec![
                (
                    DirectoryEntry {
                        inode,
                        kind: fuse3::FileType::Directory,
                        name: OsString::from("."),
                        offset: 1,
                    },
                    current_attr,
                ),
                (
                    DirectoryEntry {
                        inode: parent_inode,
                        kind: fuse3::FileType::Directory,
                        name: OsString::from(".."),
                        offset: 2,
                    },
                    parent_attr,
                ),
            ];
            for (index, entry) in self
                .list_dir(&path)
                .await
                .map_err(to_errno)?
                .into_iter()
                .enumerate()
            {
                let child_path = join_child_path(&path, OsStr::new(&entry.name))?;
                let attr = self.attr(&child_path).await.map_err(to_errno)?;
                let inode = attr.inode as u64;
                let kind = to_file_type(&entry.kind);
                entries.push((
                    DirectoryEntry {
                        inode,
                        kind,
                        name: OsString::from(entry.name),
                        offset: index as i64 + 3,
                    },
                    to_fuse_attr(attr, self.read_only()),
                ));
            }
            Ok(entries
                .into_iter()
                .filter(move |(entry, _)| entry.offset > offset)
                .collect())
        }
    }

    fn join_child_path(parent: &str, name: &OsStr) -> fuse3::Result<String> {
        let name = name.to_str().ok_or_else(|| Errno::from(libc::EINVAL))?;
        if name == "." || name == ".." || name.contains('/') {
            return Err(Errno::from(libc::EINVAL));
        }
        let joined = if parent.is_empty() {
            name.to_string()
        } else {
            format!("{parent}/{name}")
        };
        normalize_mount_path(&joined).map_err(to_errno)
    }

    fn to_fuse_attr(attr: FuseAttr, read_only: bool) -> FileAttr {
        let kind = to_file_type(&attr.kind);
        let now = Timestamp::from(SystemTime::now());
        let mtime = attr
            .mtime
            .as_deref()
            .and_then(timestamp_from_rfc3339)
            .unwrap_or(now);
        FileAttr {
            ino: attr.inode as u64,
            size: attr.size,
            blocks: attr.size.div_ceil(512),
            atime: mtime,
            mtime,
            ctime: now,
            kind,
            perm: attr
                .mode
                .map(|mode| (mode & 0o7777) as u16)
                .unwrap_or(match attr.kind {
                    FuseNodeKind::Directory => 0o555,
                    FuseNodeKind::File if read_only => 0o444,
                    FuseNodeKind::File => 0o644,
                }),
            nlink: if matches!(attr.kind, FuseNodeKind::Directory) {
                2
            } else {
                1
            },
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 4096,
        }
    }

    fn timestamp_to_rfc3339(timestamp: Timestamp) -> String {
        chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp.sec, timestamp.nsec)
            .unwrap_or_else(chrono::Utc::now)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn timestamp_from_rfc3339(value: &str) -> Option<Timestamp> {
        let timestamp = chrono::DateTime::parse_from_rfc3339(value).ok()?;
        let timestamp = timestamp.with_timezone(&chrono::Utc);
        Some(Timestamp::new(
            timestamp.timestamp(),
            timestamp.timestamp_subsec_nanos(),
        ))
    }

    fn to_file_type(kind: &FuseNodeKind) -> fuse3::FileType {
        match kind {
            FuseNodeKind::Directory => fuse3::FileType::Directory,
            FuseNodeKind::File => fuse3::FileType::RegularFile,
        }
    }

    fn to_errno(error: QuarryError) -> Errno {
        match error {
            QuarryError::NotFound(_) => Errno::from(libc::ENOENT),
            // Expired tmp documents are gone for good; to a filesystem
            // caller that is indistinguishable from not-found.
            QuarryError::Gone(_) => Errno::from(libc::ENOENT),
            QuarryError::InvalidPath(_) => Errno::from(libc::EINVAL),
            QuarryError::PreconditionFailed(_) => Errno::from(libc::EIO),
            QuarryError::DirectoryNotEmpty(_) => Errno::from(libc::ENOTEMPTY),
            QuarryError::IsADirectory(_) => Errno::from(libc::EISDIR),
            QuarryError::Conflict(_) => Errno::from(libc::EEXIST),
            QuarryError::Busy(_) => Errno::from(libc::EAGAIN),
            QuarryError::ReadOnly(_) => Errno::from(libc::EROFS),
            QuarryError::UnsupportedMediaType(_) => Errno::from(libc::ENOTSUP),
            QuarryError::PayloadTooLarge(_) => Errno::from(libc::EFBIG),
            QuarryError::Unsupported(_) => Errno::from(libc::ENOTSUP),
            // Content errors from the Phase 4 reconciled markdown write
            // (CriticMarkup, invalid frontmatter): the CONTENT is wrong, the
            // write can never succeed as stated. Never used for merge
            // outcomes — those become conflict review items and succeed.
            QuarryError::UnsupportedMarkdown(_) => Errno::from(libc::EIO),
            QuarryError::InvalidInput(_) => Errno::from(libc::EINVAL),
            QuarryError::Io(error) => Errno::from(error),
            QuarryError::Json(_)
            | QuarryError::Yaml(_)
            | QuarryError::Invariant(_)
            | QuarryError::StorageSource { .. }
            | QuarryError::GitSource { .. } => Errno::from(libc::EIO),
        }
    }
}
