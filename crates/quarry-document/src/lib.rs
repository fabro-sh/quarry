//! Durable document identity independent of an editor's tree.
//!
//! Text stays in Automerge text objects. Blocks own ordered references to
//! segments delimited by native block markers. Splitting adds a marker; moving
//! and joining change ownership. None of these operations copy characters.
//! Structural commands require one authority. `merge` is for validated changes,
//! not an endpoint for accepting arbitrary client-controlled document state.

mod block_capabilities;
mod builder;
mod commands;
mod edit;
mod error;
mod projection;
mod request;
mod review;
mod schema;
mod structure;
mod types;
mod undo;

pub use block_capabilities::*;
pub use builder::CommandBuilder;
pub use commands::Command;
pub use edit::{EditAction, EditMode};
pub use error::{DocumentError, Result};
pub use request::{CommandBatch, CommandRequest, DocumentActor};
pub use schema::INLINE_BOOLEAN_MARKS;
pub use types::*;

use automerge::{
    AutoCommit, ChangeHash, Cursor, LoadOptions, ObjId, ObjType, ROOT, ReadDoc, ScalarValue,
    TextEncoding, Value, transaction::Transactable,
};
use serde::{Serialize, de::DeserializeOwned};

const SOURCES: &str = "sources";
const BLOCKS: &str = "blocks";
const COMMENTS: &str = "comments";
const PROPOSALS: &str = "proposals";
const CONFLICTS: &str = "conflicts";

type SegmentCache = std::collections::BTreeMap<String, std::sync::Arc<Vec<projection::Segment>>>;
struct BlockCache {
    by_id: std::collections::BTreeMap<String, Block>,
    ordered: Vec<Block>,
}

pub struct Document {
    crdt: AutoCommit,
    segments: std::sync::RwLock<SegmentCache>,
    block_cache: std::sync::RwLock<Option<std::sync::Arc<BlockCache>>>,
    command_ids: Option<(String, usize)>,
    command_time: Option<String>,
    batch_active: bool,
}

impl Document {
    pub fn new() -> Result<Self> {
        Self::with_id(&new_id())
    }

    pub fn with_id(id: &str) -> Result<Self> {
        if id.is_empty() {
            return Err(DocumentError::Invalid("Empty document ID".into()));
        }
        let mut crdt = AutoCommit::new_with_encoding(TextEncoding::Utf16CodeUnit);
        crdt.put(ROOT, "document_id", id)?;
        crdt.put(ROOT, "schema_version", SCHEMA_VERSION)?;
        for name in [SOURCES, BLOCKS, COMMENTS, PROPOSALS, CONFLICTS] {
            crdt.put_object(ROOT, name, ObjType::Map)?;
        }
        crdt.commit();
        Ok(Self {
            crdt,
            segments: Default::default(),
            block_cache: Default::default(),
            command_ids: None,
            command_time: None,
            batch_active: false,
        })
    }

    pub fn from_blocks(blocks: &[SeedBlock]) -> Result<Self> {
        let mut doc = Self::new()?;
        doc.import_blocks(blocks)?;
        Ok(doc)
    }

    /// One-time import preserves supplied IDs and order, including children
    /// that precede their parents in a stored projection.
    pub fn import_blocks(&mut self, seeds: &[SeedBlock]) -> Result<()> {
        self.atomic(|d| {
            for seed in seeds {
                d.require_new(BLOCKS, &seed.id)?;
                let segment = d.create_source(&seed.text)?;
                d.put_record(
                    BLOCKS,
                    &seed.id,
                    &Block {
                        id: seed.id.clone(),
                        kind: seed.kind.clone(),
                        attrs: crate::schema::normalize_attrs(&seed.kind, seed.attrs.clone())?,
                        parent: seed.parent.clone(),
                        position: seed.position,
                        segments: vec![segment],
                        deleted: false,
                    },
                )?;
            }
            Ok(())
        })
    }

    pub fn load(bytes: &[u8]) -> Result<Self> {
        let crdt = AutoCommit::load_with_options(
            bytes,
            LoadOptions::new().text_encoding(TextEncoding::Utf16CodeUnit),
        )?;
        let doc = Self {
            crdt,
            segments: Default::default(),
            block_cache: Default::default(),
            command_ids: None,
            command_time: None,
            batch_active: false,
        };
        doc.validate()?;
        Ok(doc)
    }

    pub fn save(&self) -> Vec<u8> {
        self.crdt.clone().save()
    }

    /// Append these native changes to an archive at the supplied heads. The
    /// resulting concatenation is itself a loadable Automerge archive.
    pub fn save_after(&self, heads: &[ChangeHash]) -> Result<Vec<u8>> {
        if !self.contains_history(heads) {
            return Err(DocumentError::Conflict(
                "Archive base is not in this document's history".into(),
            ));
        }
        Ok(self.crdt.clone().save_after(heads))
    }

    pub fn heads(&self) -> Vec<ChangeHash> {
        self.crdt.clone().get_heads()
    }

    /// Loaded changes retain their causal ancestors. Checking their hashes
    /// proves history inclusion without replaying every historical operation.
    pub fn contains_history(&self, heads: &[ChangeHash]) -> bool {
        let mut history = self.crdt.clone();
        heads
            .iter()
            .all(|head| history.get_change_meta_by_hash(head).is_some())
    }

    pub fn id(&self) -> Result<String> {
        let Some((Value::Scalar(value), _)) = self.crdt.get(ROOT, "document_id")? else {
            return Err(DocumentError::Invalid("Missing document ID".into()));
        };
        let ScalarValue::Str(id) = value.as_ref() else {
            return Err(DocumentError::Invalid("Invalid document ID".into()));
        };
        if id.is_empty() {
            return Err(DocumentError::Invalid("Empty document ID".into()));
        }
        Ok(id.to_string())
    }

    pub fn fork(&self) -> Self {
        Self {
            crdt: self.crdt.clone().fork(),
            segments: std::sync::RwLock::new(
                self.segments
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            ),
            block_cache: std::sync::RwLock::new(
                self.block_cache
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            ),
            command_ids: None,
            command_time: None,
            batch_active: false,
        }
    }

    /// Copy a document into a separate authority while retaining text identity.
    /// The new document cannot merge writes addressed to its source document.
    pub fn copy_as(&self, id: &str) -> Result<Self> {
        if id.is_empty() || id == self.id()? {
            return Err(DocumentError::Invalid(
                "A copy requires a new document ID".into(),
            ));
        }
        let mut copy = self.fork();
        copy.crdt.put(ROOT, "document_id", id)?;
        copy.validate()?;
        Ok(copy)
    }

    pub fn fork_at(&self, heads: &[ChangeHash]) -> Result<Self> {
        Ok(Self {
            crdt: self.crdt.clone().fork_at(heads)?,
            segments: Default::default(),
            block_cache: Default::default(),
            command_ids: None,
            command_time: None,
            batch_active: false,
        })
    }

    /// Merge on a candidate so invalid ownership or records cannot leak into
    /// the authority. Transport and permission validation belongs upstream.
    pub fn merge(&mut self, other: &Self) -> Result<()> {
        if self.id()? != other.id()? {
            return Err(DocumentError::Conflict(
                "Cannot merge different documents".into(),
            ));
        }
        for collection in [SOURCES, BLOCKS, COMMENTS, PROPOSALS, CONFLICTS] {
            if self.root_map(collection)? != other.root_map(collection)? {
                return Err(DocumentError::Conflict(
                    "Documents do not share the same native history".into(),
                ));
            }
        }
        self.atomic(|candidate| {
            *candidate
                .block_cache
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            candidate
                .segments
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
            candidate.crdt.merge(&mut other.crdt.clone())?;
            Ok(())
        })
    }

    /// Receive committed changes relative to history already present locally.
    /// Publication and permission checks remain server responsibilities.
    pub fn merge_changes(
        &mut self,
        bytes: &[u8],
        base: &[ChangeHash],
        heads: &[ChangeHash],
    ) -> Result<()> {
        if !self.contains_history(base) {
            return Err(DocumentError::Conflict("Missing incremental base".into()));
        }
        let roots = [
            "document_id",
            "schema_version",
            SOURCES,
            BLOCKS,
            COMMENTS,
            PROPOSALS,
            CONFLICTS,
        ]
        .into_iter()
        .map(|key| {
            Ok((
                key,
                self.crdt
                    .get_all(ROOT, key)?
                    .into_iter()
                    .map(|(value, id)| (value.into_owned(), id))
                    .collect::<Vec<_>>(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
        self.atomic(|candidate| {
            let sources = candidate.root_map(SOURCES)?;
            let blocks = candidate.root_map(BLOCKS)?;
            candidate.crdt.update_diff_cursor();
            candidate.crdt.load_incremental(bytes)?;
            let patches = candidate.crdt.diff_incremental();
            candidate.crdt.reset_diff_cursor();
            if !candidate.contains_history(heads)
                || !candidate.crdt.get_missing_deps(&[]).is_empty()
            {
                return Err(DocumentError::Conflict(
                    "Incomplete incremental history".into(),
                ));
            }
            for (key, values) in &roots {
                if candidate.crdt.get_all(ROOT, *key)? != *values {
                    return Err(DocumentError::Conflict(
                        "Incremental changes replaced document identity".into(),
                    ));
                }
            }
            for patch in patches {
                if patch.obj == blocks || patch.path.iter().any(|(object, _)| *object == blocks) {
                    *candidate
                        .block_cache
                        .get_mut()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                }
                if patch.obj == sources {
                    match &patch.action {
                        automerge::PatchAction::PutMap { key, .. }
                        | automerge::PatchAction::DeleteMap { key } => {
                            candidate.invalidate_source(key)
                        }
                        _ => candidate
                            .segments
                            .get_mut()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clear(),
                    }
                }
                for (object, property) in patch.path {
                    if object == sources
                        && let automerge::Prop::Map(source) = property
                    {
                        candidate.invalidate_source(&source);
                    }
                }
            }
            Ok(())
        })
    }

    pub(crate) fn atomic<T>(&mut self, apply: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.batch_active {
            return apply(self);
        }
        let mut candidate = Self {
            crdt: self.crdt.clone(),
            segments: std::sync::RwLock::new(
                self.segments
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            ),
            block_cache: std::sync::RwLock::new(
                self.block_cache
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            ),
            command_ids: self.command_ids.clone(),
            command_time: self.command_time.clone(),
            batch_active: true,
        };
        let result = apply(&mut candidate)?;
        candidate.crdt.commit();
        candidate.validate()?;
        self.crdt = candidate.crdt;
        self.segments = candidate.segments;
        self.block_cache = candidate.block_cache;
        self.command_ids = candidate.command_ids;
        Ok(result)
    }

    pub(crate) fn fresh_id(&mut self) -> String {
        match &mut self.command_ids {
            Some((request, next)) => {
                let id = format!("{request}:{next}");
                *next += 1;
                id
            }
            None => new_id(),
        }
    }

    pub(crate) fn root_map(&self, name: &str) -> Result<ObjId> {
        self.object(&ROOT, name, ObjType::Map)
    }

    fn object(&self, parent: &ObjId, key: &str, kind: ObjType) -> Result<ObjId> {
        match self.crdt.get(parent, key)? {
            Some((Value::Object(actual), id)) if actual == kind => Ok(id),
            _ => Err(DocumentError::Invalid(format!(
                "Missing {kind:?} object: {key}"
            ))),
        }
    }

    pub(crate) fn source(&self, id: &str) -> Result<ObjId> {
        self.object(&self.root_map(SOURCES)?, id, ObjType::Text)
    }

    pub(crate) fn record<T: DeserializeOwned>(
        &self,
        collection: &'static str,
        id: &str,
    ) -> Result<T> {
        let root = self.root_map(collection)?;
        let value = self
            .crdt
            .get(root, id)?
            .ok_or_else(|| DocumentError::NotFound {
                kind: collection,
                id: id.to_string(),
            })?
            .0;
        let Value::Scalar(value) = value else {
            return Err(DocumentError::Invalid(format!(
                "Expected record: {collection}/{id}"
            )));
        };
        let ScalarValue::Str(value) = value.as_ref() else {
            return Err(DocumentError::Invalid(format!(
                "Expected record string: {collection}/{id}"
            )));
        };
        Ok(serde_json::from_str(value)?)
    }

    pub(crate) fn records<T: DeserializeOwned>(&self, collection: &'static str) -> Result<Vec<T>> {
        self.crdt
            .keys(self.root_map(collection)?)
            .map(|id| self.record(collection, &id))
            .collect()
    }

    pub(crate) fn put_record(
        &mut self,
        collection: &'static str,
        id: &str,
        record: &impl Serialize,
    ) -> Result<()> {
        if collection == BLOCKS {
            *self
                .block_cache
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
        self.crdt.put(
            self.root_map(collection)?,
            id,
            serde_json::to_string(record)?,
        )?;
        Ok(())
    }

    pub(crate) fn require_new(&self, collection: &'static str, id: &str) -> Result<()> {
        if id.is_empty() {
            return Err(DocumentError::Invalid("Empty entity ID".into()));
        }
        if self.crdt.get(self.root_map(collection)?, id)?.is_some() {
            return Err(DocumentError::AlreadyExists {
                kind: collection,
                id: id.to_string(),
            });
        }
        Ok(())
    }

    pub(crate) fn require_new_review(&self, id: &str) -> Result<()> {
        for collection in [COMMENTS, PROPOSALS, CONFLICTS] {
            self.require_new(collection, id)?;
        }
        Ok(())
    }

    pub fn block(&self, id: &str) -> Result<Block> {
        self.cached_blocks()?
            .by_id
            .get(id)
            .cloned()
            .ok_or_else(|| DocumentError::NotFound {
                kind: BLOCKS,
                id: id.into(),
            })
    }

    pub fn blocks(&self) -> Result<Vec<Block>> {
        Ok(self.cached_blocks()?.ordered.clone())
    }

    fn cached_blocks(&self) -> Result<std::sync::Arc<BlockCache>> {
        if let Some(cached) = self
            .block_cache
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            return Ok(cached.clone());
        }
        let cached = std::sync::Arc::new(self.read_blocks()?);
        *self
            .block_cache
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cached.clone());
        Ok(cached)
    }

    fn read_blocks(&self) -> Result<BlockCache> {
        let by_id: std::collections::BTreeMap<String, Block> = self
            .crdt
            .keys(self.root_map(BLOCKS)?)
            .map(|id| self.record(BLOCKS, &id).map(|block| (id, block)))
            .collect::<Result<_>>()?;
        let mut blocks: Vec<Block> = by_id.values().cloned().collect();
        blocks.retain(|b| !b.deleted);
        blocks.sort_by(|a, b| (&a.parent, a.position, &a.id).cmp(&(&b.parent, b.position, &b.id)));
        let mut children: std::collections::BTreeMap<Option<&str>, Vec<&Block>> =
            Default::default();
        for block in &blocks {
            children
                .entry(block.parent.as_deref())
                .or_default()
                .push(block);
        }
        let mut stack: Vec<_> = children
            .get(&None)
            .into_iter()
            .flatten()
            .rev()
            .copied()
            .collect();
        let mut visited = std::collections::BTreeSet::new();
        let mut ordered = Vec::new();
        while let Some(block) = stack.pop() {
            if !visited.insert(block.id.as_str()) {
                continue;
            }
            ordered.push(block.clone());
            stack.extend(
                children
                    .get(&Some(block.id.as_str()))
                    .into_iter()
                    .flatten()
                    .rev()
                    .copied(),
            );
        }
        // Invalid intermediate graphs must still reach validation, which
        // reports their missing parents or cycles without recursion.
        ordered.extend(
            blocks
                .iter()
                .filter(|block| !visited.contains(block.id.as_str()))
                .cloned(),
        );
        Ok(BlockCache { by_id, ordered })
    }

    pub(crate) fn resolve_point(&self, point: &TextPoint) -> Result<usize> {
        let cursor = Cursor::try_from(point.cursor.as_str())?;
        Ok(self
            .crdt
            .get_cursor_position(self.source(&point.source)?, &cursor, None)?)
    }

    pub(crate) fn point_at(&self, source: &str, offset: usize) -> Result<TextPoint> {
        let object = self.source(source)?;
        let position = if offset == self.crdt.length(&object) {
            automerge::CursorPosition::End
        } else {
            automerge::CursorPosition::Index(offset)
        };
        Ok(TextPoint {
            source: source.into(),
            cursor: self.crdt.get_cursor(object, position, None)?.to_string(),
        })
    }

    pub fn insert_text(&mut self, at: &TextPoint, text: &str) -> Result<()> {
        self.atomic(|d| {
            let offset = d.resolve_point(at)?;
            d.invalidate_source(&at.source);
            d.crdt.splice_text(d.source(&at.source)?, offset, 0, text)?;
            Ok(())
        })
    }

    /// Hide selected characters with a distinct deletion mark. Native identity
    /// remains available for undo and delayed review; markers stay visible.
    pub fn delete_text(&mut self, ranges: &[TextRange]) -> Result<()> {
        self.atomic(|d| {
            let mut deletes = Vec::new();
            for range in ranges {
                let (start, end) = d.range_offsets(range)?;
                for segment in d.source_segments(&range.source)?.iter() {
                    let from = start.max(segment.start);
                    let to = end.min(segment.end);
                    if from < to {
                        deletes.push((range.source.clone(), from, to));
                    }
                }
            }
            deletes.sort();
            // Overlapping ranges are invalid: deleting twice could consume a marker.
            for pair in deletes.windows(2) {
                if pair[0].0 == pair[1].0 && pair[0].2 > pair[1].1 {
                    return Err(DocumentError::Invalid("Overlapping deletion ranges".into()));
                }
            }
            let name = format!("quarry:deleted:{}", d.fresh_id());
            let mut ordinal = 0;
            for (source, start, end) in deletes {
                d.invalidate_source(&source);
                let object = d.source(&source)?;
                let selected = text_slice(&d.crdt.text(&object)?, start, end)?.to_string();
                let mut offset = start;
                for ch in selected.chars() {
                    let end = offset + ch.len_utf16();
                    // Each original character has an exclusive interval. An
                    // insertion between characters remains visible, including
                    // an insertion made concurrently with this deletion.
                    d.crdt.mark(
                        &object,
                        automerge::marks::Mark::new(
                            format!("{name}:{ordinal}"),
                            d.point_at(&source, offset)?.cursor,
                            offset,
                            end,
                        ),
                        automerge::marks::ExpandMark::None,
                    )?;
                    offset = end;
                    ordinal += 1;
                }
            }
            Ok(())
        })
    }

    pub(crate) fn range_offsets(&self, range: &TextRange) -> Result<(usize, usize)> {
        let start = self.resolve_point(&TextPoint {
            source: range.source.clone(),
            cursor: range.start.clone(),
        })?;
        let end = self.resolve_point(&TextPoint {
            source: range.source.clone(),
            cursor: range.end.clone(),
        })?;
        if start > end {
            return Err(DocumentError::Conflict("Range endpoints crossed".into()));
        }
        Ok((start, end))
    }
}

pub(crate) fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(crate) fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

pub(crate) fn byte_offset(text: &str, offset: usize) -> Result<usize> {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units == offset {
            return Ok(byte);
        }
        units += ch.len_utf16();
    }
    if units == offset {
        Ok(text.len())
    } else {
        Err(DocumentError::InvalidOffset(offset))
    }
}

pub(crate) fn text_slice(text: &str, start: usize, end: usize) -> Result<&str> {
    Ok(&text[byte_offset(text, start)?..byte_offset(text, end)?])
}
