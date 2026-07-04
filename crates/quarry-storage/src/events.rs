use quarry_core::DocumentSource;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreEventKind {
    DocumentPut,
    DocumentDelete,
    DocumentMove,
    LinksIndexed,
    DirectoryPut,
    DirectoryDelete,
    DirectoryMove,
    ConflictCreated,
    ConflictResolved,
    LibraryReindexed,
    GitSyncCompleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreEvent {
    kind: StoreEventKind,
    library_id: String,
    path: Option<String>,
    new_path: Option<String>,
    source: Option<DocumentSource>,
    tx_id: Option<String>,
    doc_id: Option<String>,
    version_id: Option<String>,
    conflict_id: Option<String>,
    peer_id: Option<String>,
    applied: Option<usize>,
    conflicts: Option<usize>,
    origin_id: Option<String>,
}

impl StoreEvent {
    fn new(kind: StoreEventKind, library_id: String) -> Self {
        Self {
            kind,
            library_id,
            path: None,
            new_path: None,
            source: None,
            tx_id: None,
            doc_id: None,
            version_id: None,
            conflict_id: None,
            peer_id: None,
            applied: None,
            conflicts: None,
            origin_id: None,
        }
    }

    pub fn document_put(
        library_id: String,
        path: String,
        source: DocumentSource,
        tx_id: String,
        doc_id: String,
        version_id: String,
        origin_id: Option<String>,
    ) -> Self {
        let mut event = Self::new(StoreEventKind::DocumentPut, library_id);
        event.path = Some(path);
        event.source = Some(source);
        event.tx_id = Some(tx_id);
        event.doc_id = Some(doc_id);
        event.version_id = Some(version_id);
        event.origin_id = origin_id;
        event
    }

    pub fn document_delete(
        library_id: String,
        path: String,
        source: DocumentSource,
        tx_id: String,
        doc_id: Option<String>,
        origin_id: Option<String>,
    ) -> Self {
        let mut event = Self::new(StoreEventKind::DocumentDelete, library_id);
        event.path = Some(path);
        event.source = Some(source);
        event.tx_id = Some(tx_id);
        event.doc_id = doc_id;
        event.origin_id = origin_id;
        event
    }

    pub fn document_move(
        library_id: String,
        path: String,
        new_path: String,
        source: DocumentSource,
        tx_id: String,
        doc_id: Option<String>,
        origin_id: Option<String>,
    ) -> Self {
        let mut event = Self::new(StoreEventKind::DocumentMove, library_id);
        event.path = Some(path);
        event.new_path = Some(new_path);
        event.source = Some(source);
        event.tx_id = Some(tx_id);
        event.doc_id = doc_id;
        event.origin_id = origin_id;
        event
    }

    pub fn links_indexed(library_id: String, path: String) -> Self {
        let mut event = Self::new(StoreEventKind::LinksIndexed, library_id);
        event.path = Some(path);
        event
    }

    pub fn directory_put(library_id: String, path: String, source: DocumentSource) -> Self {
        let mut event = Self::new(StoreEventKind::DirectoryPut, library_id);
        event.path = Some(path);
        event.source = Some(source);
        event
    }

    pub fn directory_delete(library_id: String, path: String, source: DocumentSource) -> Self {
        let mut event = Self::new(StoreEventKind::DirectoryDelete, library_id);
        event.path = Some(path);
        event.source = Some(source);
        event
    }

    pub fn directory_move(
        library_id: String,
        path: String,
        new_path: String,
        source: DocumentSource,
    ) -> Self {
        let mut event = Self::new(StoreEventKind::DirectoryMove, library_id);
        event.path = Some(path);
        event.new_path = Some(new_path);
        event.source = Some(source);
        event
    }

    pub fn conflict_created(library_id: String, path: String, conflict_id: String) -> Self {
        let mut event = Self::new(StoreEventKind::ConflictCreated, library_id);
        event.path = Some(path);
        event.conflict_id = Some(conflict_id);
        event
    }

    pub fn conflict_resolved(library_id: String, path: String, conflict_id: String) -> Self {
        let mut event = Self::new(StoreEventKind::ConflictResolved, library_id);
        event.path = Some(path);
        event.conflict_id = Some(conflict_id);
        event
    }

    pub fn library_reindexed(library_id: String) -> Self {
        Self::new(StoreEventKind::LibraryReindexed, library_id)
    }

    pub fn git_sync_completed(
        library_id: String,
        peer_id: String,
        applied: usize,
        conflicts: usize,
    ) -> Self {
        let mut event = Self::new(StoreEventKind::GitSyncCompleted, library_id);
        event.source = Some(DocumentSource::Git);
        event.peer_id = Some(peer_id);
        event.applied = Some(applied);
        event.conflicts = Some(conflicts);
        event
    }

    pub fn kind(&self) -> StoreEventKind {
        self.kind
    }

    pub fn library_id(&self) -> &str {
        &self.library_id
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    pub fn new_path(&self) -> Option<&str> {
        self.new_path.as_deref()
    }

    pub fn source(&self) -> Option<&DocumentSource> {
        self.source.as_ref()
    }

    pub fn tx_id(&self) -> Option<&str> {
        self.tx_id.as_deref()
    }

    pub fn doc_id(&self) -> Option<&str> {
        self.doc_id.as_deref()
    }

    pub fn version_id(&self) -> Option<&str> {
        self.version_id.as_deref()
    }

    pub fn conflict_id(&self) -> Option<&str> {
        self.conflict_id.as_deref()
    }

    pub fn peer_id(&self) -> Option<&str> {
        self.peer_id.as_deref()
    }

    pub fn applied(&self) -> Option<usize> {
        self.applied
    }

    pub fn conflicts(&self) -> Option<usize> {
        self.conflicts
    }

    pub fn origin_id(&self) -> Option<&str> {
        self.origin_id.as_deref()
    }
}

pub(crate) fn log_store_event(event: &StoreEvent) {
    tracing::debug!(
        event = "storage.event.emitted",
        store_event = %store_event_name(event.kind()),
        library_id = %event.library_id(),
        path = event.path().unwrap_or(""),
        new_path = event.new_path().unwrap_or(""),
        tx_id = event.tx_id().unwrap_or(""),
        doc_id = event.doc_id().unwrap_or(""),
        version_id = event.version_id().unwrap_or(""),
        source = event.source().map(DocumentSource::as_str).unwrap_or(""),
        conflict_id = event.conflict_id().unwrap_or(""),
        peer_id = event.peer_id().unwrap_or(""),
        applied = event.applied().unwrap_or(0),
        conflicts = event.conflicts().unwrap_or(0),
        origin_id = event.origin_id().unwrap_or(""),
        "store event emitted"
    );
}

fn store_event_name(kind: StoreEventKind) -> &'static str {
    match kind {
        StoreEventKind::DocumentPut => "document.put.committed",
        StoreEventKind::DocumentDelete => "document.delete.committed",
        StoreEventKind::DocumentMove => "document.move.committed",
        StoreEventKind::LinksIndexed => "links.indexed",
        StoreEventKind::DirectoryPut => "directory.put.committed",
        StoreEventKind::DirectoryDelete => "directory.delete.committed",
        StoreEventKind::DirectoryMove => "directory.move.committed",
        StoreEventKind::ConflictCreated => "conflict.created",
        StoreEventKind::ConflictResolved => "conflict.resolved",
        StoreEventKind::LibraryReindexed => "library.reindexed",
        StoreEventKind::GitSyncCompleted => "git.sync.completed",
    }
}
