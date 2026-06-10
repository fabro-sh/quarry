//! Session-document projections — the seed/checkpoint mechanics of the
//! session-scoped collaboration rewrite (Phase 3).
//!
//! A live editing session is an ephemeral Yjs document seeded from canonical
//! block rows and checkpointed back to rows on a debounce. This module owns
//! the two pure projections plus the in-place reconciliation the semantic
//! mutation gateway uses to write into a live session as a collaborator:
//!
//! - [`seed_session_nodes`]: rows + review anchors → Slate nodes carrying the
//!   browser's review marks. Built once per session start and whenever the
//!   gateway needs the doc-shaped image of a row set.
//! - [`project_session_nodes`]: Slate nodes (as observed in the live doc) →
//!   rows + review anchors. Run at every checkpoint.
//! - [`reconcile_session_children`]: minimally edits the live doc so it
//!   projects to a desired node tree, preserving element identity (and
//!   therefore peer cursors) for untouched blocks.
//! - [`read_review_meta_from_map`]: the live doc's `review` map → [`ReviewMeta`]
//!   (comment/suggestion bodies, authors, resolution state).
//!
//! ## Anchors ride as marks (deliberate deviation from the Gate A spike)
//!
//! The Gate A spike proved anchor exactness with server-side
//! `StickyIndex` pairs. Production uses the browser's own representation
//! instead: review anchors live in the session doc as text marks
//! (`comment`/`comment_<id>` booleans, `suggestion`/`suggestion_<id>`
//! objects), exactly as the unmodified browser renders and edits them.
//! Marks give the same CRDT guarantees the spike proved for sticky indices —
//! interior inserts grow the range, edits move it, deleting the range kills
//! it — and they additionally:
//!
//! - survive client-side block moves (delete + reinsert clones content,
//!   stranding sticky indices but carrying marks), eliminating the move
//!   transplant and the design-delta-1 re-placement pass;
//! - keep a single source of truth with what the browser displays; and
//! - let browser-created comments/suggestions reach rows: the checkpoint
//!   discovers new mark ids it never seeded.
//!
//! ## Boundary-insert semantics (rows-mode vs session-mode divergence)
//!
//! Marks follow Yjs format-marker semantics, which differ from the Gate A
//! sticky-index rules at exactly one boundary:
//!
//! - Insert at the anchor's START: excluded — the anchor shifts right and
//!   never grows leftward (same as Gate A `Assoc::After`).
//! - Insert strictly INSIDE the anchor: included — the anchor grows (same
//!   as Gate A).
//! - Insert at the anchor's END: **included — the anchor grows rightward**,
//!   diverging from Gate A's `Assoc::Before` exclusion. A plain insert at a
//!   mark's end boundary inherits the mark, for review marks exactly as for
//!   bold runs (and for a suggestion's remove-range, which grows to cover
//!   the new text).
//!
//! This divergence is ACCEPTED as the session-mode semantics: during a live
//! session the editor's mark behavior is WYSIWYG-authoritative — when a
//! user types at the end of a highlighted range, Plate extends the
//! highlight, and persisting what the browser displays beats persisting an
//! invisible sticky range that disagrees with it. Rows-mode (the gateway's
//! `replace_block_content` anchor math in `gateway.rs`) keeps the Gate A
//! exclusion at BOTH boundaries. Pinned by the boundary-insert tests in
//! `tests/session_doc.rs`.
//!
//! ## Coordinate system
//!
//! Row offsets are UTF-16 code units over a block's flat text. Suggestion
//! *insert* leaves (the proposed replacement text shown inline) are part of
//! the doc's text but NOT part of the canonical row text; the projection
//! excludes them from `BlockRow::text` and accumulates them into the
//! suggestion's `replacement`. A suggestion with no `remove` segments is an
//! insertion suggestion and projects as a collapsed (`start == end`) anchor
//! with a non-empty replacement.
//!
//! ## Block identity and degradation
//!
//! Block identity rides as the `id` element attribute. The projection keeps
//! existing ids and mints fresh ones for browser-created blocks that lack
//! one (or that duplicate an id already seen). Trailing empty paragraphs —
//! the editor's runtime scaffold — are stripped, ignoring their runtime id.
//! A leaf block containing inline elements the row model cannot represent
//! (wikilinks, inline images) degrades to a `raw_markdown` row that
//! preserves the block's Markdown source and its id; review marks inside
//! such a block are dropped (their anchors orphan at the row layer).

use crate::markdown_writer::{is_known_inline_mark, slate_to_markdown};
use crate::rows::{is_utf16_boundary, utf16_len, BlockRow, LinkRange, MarkRun};
use crate::slate::{Attrs, Node};
use crate::yjs_builder::{apply_built, xmltext_to_slate};
use crate::Unsupported;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use yrs::{Map, MapRef, Out, ReadTxn, Text, TransactionMut, Xml, XmlTextRef};

const INLINE_LINK_TYPE: &str = "a";
/// Marks that are transient editor state, never persisted to rows.
const TRANSIENT_MARKS: [&str; 1] = ["comment_draft"];

// ---------------------------------------------------------------------------
// Anchor model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionAnchorKind {
    Comment,
    Suggestion,
}

/// A review anchor as the session layer sees it: offsets in row coordinates
/// (UTF-16, suggestion-insert text excluded). `by`/`at_ms` feed the
/// `suggestion_<id>` mark payload the browser expects (`userId`/`createdAt`).
#[derive(Clone, Debug, PartialEq)]
pub struct SessionAnchor {
    pub id: String,
    pub kind: SessionAnchorKind,
    pub block_id: String,
    pub start: u32,
    pub end: u32,
    /// `Some` for suggestions (empty string = deletion suggestion).
    pub replacement: Option<String>,
    pub by: Option<String>,
    pub at_ms: i64,
}

/// The checkpoint projection of a live session doc.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionProjection {
    pub rows: Vec<BlockRow>,
    pub anchors: Vec<SessionAnchor>,
    /// Inline mark keys the projection DROPPED because the Markdown writer
    /// cannot render them (see `is_known_inline_mark`). Dropping them keeps
    /// every checkpoint exportable — an unknown mark must never wedge a
    /// session into unpersistable state. Callers log these.
    pub dropped_marks: BTreeSet<String>,
}

// ---------------------------------------------------------------------------
// Inline segments: a flat, splittable view of a leaf block's inline content.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Segment {
    text: String,
    marks: Attrs,
    /// `Some(url)` when this run lives inside a link element.
    link: Option<String>,
}

fn segment_len(segment: &Segment) -> u32 {
    utf16_len(&segment.text)
}

fn row_segments(row: &BlockRow) -> Vec<Segment> {
    let len = utf16_len(&row.text);
    let mut boundaries: Vec<u32> = vec![0, len];
    for run in &row.marks {
        boundaries.push(run.start);
        boundaries.push(run.end);
    }
    for link in &row.links {
        boundaries.push(link.start);
        boundaries.push(link.end);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut segments = Vec::new();
    for window in boundaries.windows(2) {
        let (from, to) = (window[0], window[1]);
        if from == to {
            continue;
        }
        let marks = row
            .marks
            .iter()
            .find(|run| run.start <= from && to <= run.end)
            .map(|run| run.marks.clone())
            .unwrap_or_default();
        let link = row
            .links
            .iter()
            .find(|link| link.start <= from && to <= link.end)
            .map(|link| link.url.clone());
        segments.push(Segment {
            text: utf16_slice(&row.text, from, to),
            marks,
            link,
        });
    }
    segments
}

/// Splits `segments` so `offset` (flat coordinates) falls on a segment
/// boundary, returning the index of the first segment at/after the offset.
fn split_at(segments: &mut Vec<Segment>, offset: u32) -> usize {
    let mut cursor = 0u32;
    for index in 0..segments.len() {
        let len = segment_len(&segments[index]);
        if cursor == offset {
            return index;
        }
        if cursor + len > offset {
            let local = offset - cursor;
            let segment = segments[index].clone();
            let head_text = utf16_slice(&segment.text, 0, local);
            let tail_text = utf16_slice(&segment.text, local, segment_len(&segment));
            segments[index].text = head_text;
            segments.insert(
                index + 1,
                Segment {
                    text: tail_text,
                    ..segment
                },
            );
            return index + 1;
        }
        cursor += len;
    }
    segments.len()
}

fn add_marks_over(segments: &mut Vec<Segment>, start: u32, end: u32, marks: &Attrs) {
    let from = split_at(segments, start);
    let to = split_at(segments, end);
    for segment in &mut segments[from..to] {
        for (key, value) in marks {
            segment.marks.insert(key.clone(), value.clone());
        }
    }
}

/// Inserts a non-link segment at `offset` (original flat coordinates),
/// clamped outward so it never splits a link element.
fn insert_segment_at(segments: &mut Vec<Segment>, offset: u32, segment: Segment) {
    let mut index = split_at(segments, offset);
    // If the boundary falls between two runs of the same link, hop past the
    // remainder of that link rather than splitting it into two elements.
    while index < segments.len()
        && index > 0
        && segments[index].link.is_some()
        && segments[index].link == segments[index - 1].link
    {
        index += 1;
    }
    segments.insert(index, segment);
}

fn segments_to_nodes(segments: &[Segment]) -> Vec<Node> {
    let mut children: Vec<Node> = Vec::new();
    let mut index = 0;
    while index < segments.len() {
        match &segments[index].link {
            None => {
                children.push(Node::text(
                    segments[index].text.clone(),
                    segments[index].marks.clone(),
                ));
                index += 1;
            }
            Some(url) => {
                let url = url.clone();
                let mut inner = Vec::new();
                while index < segments.len() && segments[index].link.as_deref() == Some(&url) {
                    inner.push(Node::text(
                        segments[index].text.clone(),
                        segments[index].marks.clone(),
                    ));
                    index += 1;
                }
                children.push(Node::element(
                    INLINE_LINK_TYPE,
                    crate::slate::attrs([("url", json!(url))]),
                    inner,
                ));
            }
        }
    }
    if children.is_empty() {
        children.push(Node::text("", Attrs::new()));
    }
    children
}

fn utf16_slice(text: &str, from: u32, to: u32) -> String {
    text[byte_of_utf16(text, from)..byte_of_utf16(text, to)].to_string()
}

fn byte_of_utf16(text: &str, target: u32) -> usize {
    let mut seen = 0u32;
    for (byte_index, ch) in text.char_indices() {
        if seen >= target {
            return byte_index;
        }
        seen += ch.len_utf16() as u32;
    }
    text.len()
}

// ---------------------------------------------------------------------------
// Seed: rows + anchors → Slate nodes with review marks.
// ---------------------------------------------------------------------------

/// Builds the doc-shaped Slate tree for a row set, overlaying review anchors
/// as the browser's comment/suggestion marks. Callers pass only anchors that
/// belong in a live doc (open or resolved comments, open suggestions) with
/// offsets valid for their block's current text.
pub fn seed_session_nodes(
    rows: &[BlockRow],
    anchors: &[SessionAnchor],
) -> Result<Vec<Node>, Unsupported> {
    let mut nodes = build_block_nodes(rows, None, anchors)?;
    if nodes.is_empty() {
        nodes = build_block_nodes(&[empty_paragraph_row()], None, &[])?;
    }
    Ok(nodes)
}

fn empty_paragraph_row() -> BlockRow {
    BlockRow {
        block_id: "seed-empty".to_string(),
        parent_block_id: None,
        position: 0,
        block_type: "p".to_string(),
        attrs: Attrs::new(),
        text: String::new(),
        marks: Vec::new(),
        links: Vec::new(),
    }
}

fn build_block_nodes(
    rows: &[BlockRow],
    parent: Option<&str>,
    anchors: &[SessionAnchor],
) -> Result<Vec<Node>, Unsupported> {
    let mut children: Vec<&BlockRow> = rows
        .iter()
        .filter(|row| row.parent_block_id.as_deref() == parent)
        .collect();
    children.sort_by_key(|row| row.position);
    children
        .iter()
        .map(|row| block_node(rows, row, anchors))
        .collect()
}

fn block_node(
    rows: &[BlockRow],
    row: &BlockRow,
    anchors: &[SessionAnchor],
) -> Result<Node, Unsupported> {
    let mut node_attrs = Attrs::new();
    node_attrs.insert("id".to_string(), json!(row.block_id));
    node_attrs.extend(row.attrs.clone());
    let nested = build_block_nodes(rows, Some(&row.block_id), anchors)?;
    let children = if nested.is_empty() {
        inline_children_with_anchors(row, anchors)?
    } else {
        if !row.text.is_empty() || !row.marks.is_empty() || !row.links.is_empty() {
            return Err(Unsupported::new(format!(
                "container block {} must not carry inline content",
                row.block_id
            )));
        }
        nested
    };
    Ok(Node::element(row.block_type.clone(), node_attrs, children))
}

fn inline_children_with_anchors(
    row: &BlockRow,
    anchors: &[SessionAnchor],
) -> Result<Vec<Node>, Unsupported> {
    let mut segments = row_segments(row);
    let len = utf16_len(&row.text);
    let block_anchors: Vec<&SessionAnchor> = anchors
        .iter()
        .filter(|anchor| anchor.block_id == row.block_id)
        .collect();
    for anchor in &block_anchors {
        if anchor.end > len
            || anchor.start > anchor.end
            || !is_utf16_boundary(&row.text, anchor.start)
            || !is_utf16_boundary(&row.text, anchor.end)
        {
            return Err(Unsupported::new(format!(
                "anchor {} offsets [{}, {}) do not fit block {}",
                anchor.id, anchor.start, anchor.end, row.block_id
            )));
        }
        if anchor.start < anchor.end {
            add_marks_over(
                &mut segments,
                anchor.start,
                anchor.end,
                &anchor_marks(anchor),
            );
        }
    }
    // Insert leaves are spliced in descending position so earlier splices do
    // not shift the original flat coordinates of later ones.
    let mut inserts: Vec<(u32, Segment)> = block_anchors
        .iter()
        .filter_map(|anchor| insert_leaf(anchor))
        .collect();
    inserts.sort_by_key(|(position, _)| std::cmp::Reverse(*position));
    for (position, segment) in inserts {
        insert_segment_at(&mut segments, position, segment);
    }
    Ok(segments_to_nodes(&segments))
}

fn anchor_marks(anchor: &SessionAnchor) -> Attrs {
    match anchor.kind {
        SessionAnchorKind::Comment => crate::slate::attrs([
            ("comment".to_string(), json!(true)),
            (format!("comment_{}", anchor.id), json!(true)),
        ]),
        SessionAnchorKind::Suggestion => suggestion_marks(anchor, "remove"),
    }
}

fn suggestion_marks(anchor: &SessionAnchor, ty: &str) -> Attrs {
    crate::slate::attrs([
        ("suggestion".to_string(), json!(true)),
        (
            format!("suggestion_{}", anchor.id),
            json!({
                "id": anchor.id,
                "type": ty,
                "userId": anchor.by.clone().unwrap_or_else(|| "unknown".to_string()),
                "createdAt": anchor.at_ms,
            }),
        ),
    ])
}

fn insert_leaf(anchor: &SessionAnchor) -> Option<(u32, Segment)> {
    if anchor.kind != SessionAnchorKind::Suggestion {
        return None;
    }
    let replacement = anchor.replacement.as_deref().unwrap_or_default();
    if replacement.is_empty() {
        return None;
    }
    Some((
        anchor.end,
        Segment {
            text: replacement.to_string(),
            marks: suggestion_marks(anchor, "insert"),
            link: None,
        },
    ))
}

// ---------------------------------------------------------------------------
// Checkpoint: Slate nodes (live doc) → rows + anchors.
// ---------------------------------------------------------------------------

/// Projects the live doc's top-level children to rows and review anchors.
/// `mint_id` supplies block ids for elements that lack one (or that reuse an
/// id already taken in this projection).
pub fn project_session_nodes(
    children: &[Node],
    mut mint_id: impl FnMut() -> String,
) -> Result<SessionProjection, Unsupported> {
    let children = strip_trailing_scaffold(children);
    let mut projection = SessionProjection {
        rows: Vec::new(),
        anchors: Vec::new(),
        dropped_marks: BTreeSet::new(),
    };
    let mut taken = HashSet::new();
    for (position, child) in children.iter().enumerate() {
        collect_block(
            child,
            None,
            position as u32,
            &mut mint_id,
            &mut taken,
            &mut projection,
        )?;
    }
    if projection.rows.is_empty() {
        projection.rows.push(BlockRow {
            block_id: mint_id(),
            parent_block_id: None,
            position: 0,
            block_type: "p".to_string(),
            attrs: Attrs::new(),
            text: String::new(),
            marks: Vec::new(),
            links: Vec::new(),
        });
    }
    Ok(projection)
}

/// Trailing empty paragraphs are the editor's runtime scaffold; their runtime
/// `id` attribute does not make them content.
fn strip_trailing_scaffold(children: &[Node]) -> &[Node] {
    let mut end = children.len();
    while end > 1 && is_scaffold_paragraph(&children[end - 1]) {
        end -= 1;
    }
    &children[..end]
}

fn is_scaffold_paragraph(node: &Node) -> bool {
    match node {
        Node::Element {
            ty,
            attrs,
            children,
        } if ty == "p" && attrs.keys().all(|key| key == "id") => children.iter().all(|child| {
            matches!(child, Node::Text { text, marks } if text.is_empty() && marks.is_empty())
        }),
        _ => false,
    }
}

fn collect_block(
    node: &Node,
    parent: Option<&str>,
    position: u32,
    mint_id: &mut impl FnMut() -> String,
    taken: &mut HashSet<String>,
    out: &mut SessionProjection,
) -> Result<(), Unsupported> {
    let Node::Element {
        ty,
        attrs,
        children,
    } = node
    else {
        return Err(Unsupported::new("bare text node at block level"));
    };
    let mut attrs = attrs.clone();
    let block_id = match attrs.shift_remove("id") {
        Some(Value::String(id)) if !id.is_empty() && taken.insert(id.clone()) => id,
        _ => {
            let id = mint_id();
            taken.insert(id.clone());
            id
        }
    };
    let is_container = children.iter().any(is_block_element);
    let mut row = BlockRow {
        block_id: block_id.clone(),
        parent_block_id: parent.map(str::to_string),
        position,
        block_type: ty.clone(),
        attrs,
        text: String::new(),
        marks: Vec::new(),
        links: Vec::new(),
    };
    if ty == "raw_markdown" {
        out.rows.push(row);
        return Ok(());
    }
    if is_container {
        out.rows.push(row);
        for (child_position, child) in children.iter().enumerate() {
            if !is_block_element(child) {
                return Err(Unsupported::new(format!(
                    "block <{ty}> mixes inline and block children"
                )));
            }
            collect_block(
                child,
                Some(&block_id),
                child_position as u32,
                mint_id,
                taken,
                out,
            )?;
        }
        return Ok(());
    }
    match extract_inline(&block_id, children) {
        Ok(extraction) => {
            row.text = extraction.text;
            row.marks = extraction.marks;
            row.links = extraction.links;
            out.rows.push(row);
            out.anchors.extend(extraction.anchors);
            out.dropped_marks.extend(extraction.dropped_marks);
        }
        Err(_) => {
            // Inline content the row model cannot represent (wikilinks,
            // inline images, unknown elements): degrade to a raw_markdown
            // row preserving the block's Markdown source. Review marks are
            // dropped; their anchors orphan at the row layer.
            out.rows.push(degrade_to_raw(row, node)?);
        }
    }
    Ok(())
}

fn is_block_element(child: &Node) -> bool {
    matches!(child, Node::Element { ty, .. }
        if ty != INLINE_LINK_TYPE && !is_inline_void(ty))
}

fn is_inline_void(ty: &str) -> bool {
    matches!(ty, "wikilink" | "img")
}

fn degrade_to_raw(row: BlockRow, node: &Node) -> Result<BlockRow, Unsupported> {
    let cleaned = strip_review_content(node)
        .ok_or_else(|| Unsupported::new("block reduced to nothing after review-mark cleanup"))?;
    let markdown = slate_to_markdown(std::slice::from_ref(&without_id(&cleaned)))
        .map_err(|error| error.context(format!("degrading block {} to raw", row.block_id)))?;
    let markdown = markdown.trim_end_matches('\n');
    Ok(BlockRow {
        block_type: "raw_markdown".to_string(),
        attrs: crate::slate::attrs([("markdown", json!(markdown))]),
        text: String::new(),
        marks: Vec::new(),
        links: Vec::new(),
        ..row
    })
}

fn without_id(node: &Node) -> Node {
    match node {
        Node::Element {
            ty,
            attrs,
            children,
        } => {
            let mut attrs = attrs.clone();
            attrs.shift_remove("id");
            Node::element(ty.clone(), attrs, children.clone())
        }
        text => text.clone(),
    }
}

/// Removes review marks and suggestion-insert leaves from a subtree.
fn strip_review_content(node: &Node) -> Option<Node> {
    match node {
        Node::Element {
            ty,
            attrs,
            children,
        } => {
            let children: Vec<Node> = children.iter().filter_map(strip_review_content).collect();
            Some(Node::element(ty.clone(), attrs.clone(), children))
        }
        Node::Text { text, marks } => {
            let classified = classify_marks(marks);
            if classified.insert_suggestion_ids.is_some() {
                return None;
            }
            Some(Node::text(text.clone(), classified.formatting))
        }
    }
}

// ---------------------------------------------------------------------------
// Inline extraction with review-mark partitioning.
// ---------------------------------------------------------------------------

struct InlineExtraction {
    text: String,
    marks: Vec<MarkRun>,
    links: Vec<LinkRange>,
    anchors: Vec<SessionAnchor>,
    dropped_marks: BTreeSet<String>,
}

#[derive(Default)]
struct AnchorAccumulator {
    /// id → (kind, range over the flat text, by, at_ms)
    ranges: BTreeMap<String, (SessionAnchorKind, u32, u32, Option<String>, i64)>,
    /// suggestion id → (replacement text, first insert position, by, at_ms)
    inserts: BTreeMap<String, (String, u32, Option<String>, i64)>,
    /// Unknown mark keys dropped from this block's leaves.
    dropped: BTreeSet<String>,
}

struct ClassifiedMarks {
    formatting: Attrs,
    /// Unknown formatting keys, dropped so the row export always renders.
    dropped: Vec<String>,
    comment_ids: Vec<String>,
    /// suggestion id → (is_insert, by, at_ms)
    suggestions: Vec<(String, bool, Option<String>, i64)>,
    /// `Some` when every suggestion mark on the leaf is an insert segment.
    insert_suggestion_ids: Option<Vec<(String, Option<String>, i64)>>,
}

fn classify_marks(marks: &Attrs) -> ClassifiedMarks {
    let mut formatting = Attrs::new();
    let mut dropped = Vec::new();
    let mut comment_ids = Vec::new();
    let mut suggestions: Vec<(String, bool, Option<String>, i64)> = Vec::new();
    for (key, value) in marks {
        if TRANSIENT_MARKS.contains(&key.as_str()) || key == "comment" || key == "suggestion" {
            continue;
        }
        if let Some(id) = key.strip_prefix("comment_") {
            if value == &json!(true) {
                comment_ids.push(id.to_string());
                continue;
            }
        }
        if let Some(id) = key.strip_prefix("suggestion_") {
            let ty = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("remove");
            let by = value
                .get("userId")
                .and_then(Value::as_str)
                .map(str::to_string);
            let at_ms = value.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
            suggestions.push((id.to_string(), ty == "insert", by, at_ms));
            continue;
        }
        if is_known_inline_mark(key) {
            formatting.insert(key.clone(), value.clone());
        } else {
            dropped.push(key.clone());
        }
    }
    let insert_suggestion_ids = (!suggestions.is_empty()
        && suggestions.iter().all(|(_, insert, _, _)| *insert))
    .then(|| {
        suggestions
            .iter()
            .map(|(id, _, by, at_ms)| (id.clone(), by.clone(), *at_ms))
            .collect()
    });
    ClassifiedMarks {
        formatting,
        dropped,
        comment_ids,
        suggestions,
        insert_suggestion_ids,
    }
}

fn extract_inline(block_id: &str, children: &[Node]) -> Result<InlineExtraction, Unsupported> {
    let mut text = String::new();
    let mut runs: Vec<MarkRun> = Vec::new();
    let mut links = Vec::new();
    let mut accumulator = AnchorAccumulator::default();
    for child in children {
        match child {
            Node::Text { text: chunk, marks } => {
                append_leaf(&mut text, &mut runs, &mut accumulator, chunk, marks)
            }
            Node::Element {
                ty,
                attrs: link_attrs,
                children,
            } if ty == INLINE_LINK_TYPE => {
                let url = link_attrs
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Unsupported::new("link element without a string url"))?
                    .to_string();
                let start = utf16_len(&text);
                for inner in children {
                    let Node::Text { text: chunk, marks } = inner else {
                        return Err(Unsupported::new("link with non-text children"));
                    };
                    append_leaf(&mut text, &mut runs, &mut accumulator, chunk, marks);
                }
                let end = utf16_len(&text);
                if start < end {
                    links.push(LinkRange { start, end, url });
                }
            }
            Node::Element { ty, .. } => {
                return Err(Unsupported::new(format!(
                    "inline <{ty}> has no block row representation"
                )))
            }
        }
    }
    let dropped_marks = std::mem::take(&mut accumulator.dropped);
    let anchors = accumulator.into_anchors(block_id);
    Ok(InlineExtraction {
        text,
        marks: runs,
        links,
        anchors,
        dropped_marks,
    })
}

fn append_leaf(
    text: &mut String,
    runs: &mut Vec<MarkRun>,
    accumulator: &mut AnchorAccumulator,
    chunk: &str,
    marks: &Attrs,
) {
    let classified = classify_marks(marks);
    accumulator.dropped.extend(classified.dropped.clone());
    let position = utf16_len(text);
    if let Some(ids) = classified.insert_suggestion_ids {
        for (id, by, at_ms) in ids {
            let entry = accumulator
                .inserts
                .entry(id)
                .or_insert_with(|| (String::new(), position, by.clone(), at_ms));
            entry.0.push_str(chunk);
        }
        return;
    }
    let start = position;
    text.push_str(chunk);
    let end = utf16_len(text);
    if start < end {
        for id in classified.comment_ids {
            extend_range(
                accumulator,
                id,
                SessionAnchorKind::Comment,
                start,
                end,
                None,
                0,
            );
        }
        for (id, _, by, at_ms) in classified.suggestions {
            extend_range(
                accumulator,
                id,
                SessionAnchorKind::Suggestion,
                start,
                end,
                by,
                at_ms,
            );
        }
    }
    if chunk.is_empty() || classified.formatting.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut() {
        if last.end == start && last.marks == classified.formatting {
            last.end = end;
            return;
        }
    }
    runs.push(MarkRun {
        start,
        end,
        marks: classified.formatting,
    });
}

fn extend_range(
    accumulator: &mut AnchorAccumulator,
    id: String,
    kind: SessionAnchorKind,
    start: u32,
    end: u32,
    by: Option<String>,
    at_ms: i64,
) {
    accumulator
        .ranges
        .entry(id)
        .and_modify(|(_, range_start, range_end, _, _)| {
            *range_start = (*range_start).min(start);
            *range_end = (*range_end).max(end);
        })
        .or_insert((kind, start, end, by, at_ms));
}

impl AnchorAccumulator {
    fn into_anchors(mut self, block_id: &str) -> Vec<SessionAnchor> {
        let mut anchors = Vec::new();
        for (id, (kind, start, end, by, at_ms)) in std::mem::take(&mut self.ranges) {
            let replacement = match kind {
                SessionAnchorKind::Comment => None,
                SessionAnchorKind::Suggestion => Some(
                    self.inserts
                        .remove(&id)
                        .map(|(replacement, _, _, _)| replacement)
                        .unwrap_or_default(),
                ),
            };
            anchors.push(SessionAnchor {
                id,
                kind,
                block_id: block_id.to_string(),
                start,
                end,
                replacement,
                by,
                at_ms,
            });
        }
        // Insert-only suggestions: a collapsed anchor at the insert position.
        for (id, (replacement, position, by, at_ms)) in self.inserts {
            anchors.push(SessionAnchor {
                id,
                kind: SessionAnchorKind::Suggestion,
                block_id: block_id.to_string(),
                start: position,
                end: position,
                replacement: Some(replacement),
                by,
                at_ms,
            });
        }
        anchors
    }
}

// ---------------------------------------------------------------------------
// Review meta map (the doc's `review` root).
// ---------------------------------------------------------------------------

/// Reads the live doc's review meta map into [`ReviewMeta`]. Sections the
/// browser has not created and entries that fail to parse are skipped.
pub fn read_review_meta_from_map<T: ReadTxn>(txn: &T, root: &MapRef) -> crate::ReviewMeta {
    crate::ReviewMeta {
        comments: read_review_section(txn, root, "comments"),
        suggestions: read_review_section(txn, root, "suggestions"),
    }
}

fn read_review_section<T: ReadTxn>(
    txn: &T,
    root: &MapRef,
    section: &str,
) -> BTreeMap<String, crate::ReviewMetaEntry> {
    let mut entries = BTreeMap::new();
    let Some(Out::YMap(section)) = root.get(txn, section) else {
        return entries;
    };
    for (id, value) in section.iter(txn) {
        let Out::Any(any) = value else {
            continue;
        };
        let Ok(json) = serde_json::to_value(&any) else {
            continue;
        };
        if let Ok(entry) = serde_json::from_value::<crate::ReviewMetaEntry>(json) {
            entries.insert(id.to_string(), entry);
        }
    }
    entries
}

// ---------------------------------------------------------------------------
// Reconcile: minimally edit the live doc to match a desired node tree.
// ---------------------------------------------------------------------------

/// Edits `parent` in place so its children project to `desired`, preserving
/// element identity for blocks whose content is unchanged (peer cursors in
/// untouched blocks survive).
///
/// `pre` is the doc-shaped image of the state the desired tree was computed
/// FROM. Blocks whose pre and desired images are identical are never
/// touched, even if their live content has drifted: concurrent keystrokes
/// that raced the caller's snapshot survive in blocks the change did not
/// target. Blocks the change did target are rewritten to the desired image
/// (a same-block race converges to the desired content — awkward merges are
/// the accepted Gate B behavior).
///
/// Elements whose id appears in neither `pre` nor `desired` are *foreign*
/// (concurrent browser-created blocks or runtime scaffold) and are left in
/// place.
pub fn reconcile_session_children(
    txn: &mut TransactionMut<'_>,
    parent: &XmlTextRef,
    pre: &[Node],
    desired: &[Node],
) -> Result<(), Unsupported> {
    let known_ids: HashSet<String> = pre.iter().filter_map(node_id).collect();
    let pre_by_id: std::collections::HashMap<String, &Node> = pre
        .iter()
        .filter_map(|node| node_id(node).map(|id| (id, node)))
        .collect();
    reconcile_children_inner(txn, parent, &pre_by_id, desired, &known_ids)
}

fn reconcile_children_inner(
    txn: &mut TransactionMut<'_>,
    parent: &XmlTextRef,
    pre_by_id: &std::collections::HashMap<String, &Node>,
    desired: &[Node],
    known_ids: &HashSet<String>,
) -> Result<(), Unsupported> {
    let desired_ids: Vec<Option<String>> = desired.iter().map(node_id).collect();
    let desired_id_set: HashSet<&str> = desired_ids.iter().filter_map(|id| id.as_deref()).collect();

    // In-place updates first (no index shifts), then structural edits.
    let mut current = current_children(txn, parent)?;
    let current_id_at =
        |entries: &[(Option<String>, Node, XmlTextRef)], index: usize| entries[index].0.clone();

    // Remove current elements whose id the desired tree dropped (and which
    // the pre-state rows knew about — everything else is foreign and stays).
    let mut removals: Vec<u32> = Vec::new();
    for (index, (id, _, _)) in current.iter().enumerate() {
        if let Some(id) = id {
            if known_ids.contains(id) && !desired_id_set.contains(id.as_str()) {
                removals.push(index as u32);
            }
        }
    }
    for index in removals.into_iter().rev() {
        parent.remove_range(txn, index, 1);
    }
    current = current_children(txn, parent)?;

    // Walk desired children, skipping foreign current elements.
    let mut cursor = 0usize;
    for (desired_index, desired_node) in desired.iter().enumerate() {
        let desired_id = desired_ids[desired_index]
            .as_deref()
            .ok_or_else(|| Unsupported::new("desired session block without an id"))?;
        // Skip foreign elements (not known, not desired).
        while cursor < current.len() {
            match current_id_at(&current, cursor) {
                Some(id) if desired_id_set.contains(id.as_str()) || known_ids.contains(&id) => {
                    break
                }
                Some(_) | None => cursor += 1,
            }
        }
        let at_cursor = (cursor < current.len())
            .then(|| current_id_at(&current, cursor))
            .flatten();
        if at_cursor.as_deref() == Some(desired_id) {
            // Untouched by this change: the pre and desired images agree, so
            // leave the live element alone (it may carry concurrent edits).
            if pre_by_id.get(desired_id).copied() == Some(desired_node) {
                cursor += 1;
                continue;
            }
            reconcile_element(
                txn,
                &current[cursor].1,
                &current[cursor].2,
                pre_by_id.get(desired_id).copied(),
                desired_node,
            )?;
            cursor += 1;
            continue;
        }
        // The desired element is elsewhere (moved) or new: if it exists
        // later, remove the old copy first, then insert the desired subtree
        // at the cursor. Marks travel with content, so reinsertion preserves
        // review anchors.
        if let Some(old_index) = current
            .iter()
            .position(|(id, _, _)| id.as_deref() == Some(desired_id))
        {
            parent.remove_range(txn, old_index as u32, 1);
            current.remove(old_index);
            if old_index < cursor {
                cursor -= 1;
            }
        }
        apply_built(
            txn,
            parent,
            cursor as u32,
            std::slice::from_ref(desired_node),
        );
        current = current_children(txn, parent)?;
        cursor += 1;
    }
    Ok(())
}

fn reconcile_element(
    txn: &mut TransactionMut<'_>,
    current: &Node,
    element: &XmlTextRef,
    pre: Option<&Node>,
    desired: &Node,
) -> Result<(), Unsupported> {
    if current == desired {
        return Ok(());
    }
    let Node::Element {
        ty: desired_ty,
        attrs: desired_attrs,
        children: desired_children,
    } = desired
    else {
        return Err(Unsupported::new("desired session block must be an element"));
    };
    let Node::Element {
        ty: current_ty,
        attrs: current_attrs,
        children: current_children,
    } = current
    else {
        return Err(Unsupported::new("live session block must be an element"));
    };

    if current_ty != desired_ty {
        element.insert_attribute(txn, "type", yrs::Any::from(desired_ty.clone()));
    }
    for key in current_attrs.keys() {
        if !desired_attrs.contains_key(key) {
            element.remove_attribute(txn, &key.as_str());
        }
    }
    for (key, value) in desired_attrs {
        if current_attrs.get(key) != Some(value) {
            element.insert_attribute(txn, key.clone(), json_to_any(value));
        }
    }

    let desired_is_container = desired_children.iter().any(is_block_element);
    let current_is_container = current_children.iter().any(is_block_element);
    if desired_is_container && current_is_container {
        let pre_children: &[Node] = match pre {
            Some(Node::Element { children, .. }) => children,
            _ => &[],
        };
        let known_ids: HashSet<String> = pre_children.iter().filter_map(node_id).collect();
        let pre_by_id: std::collections::HashMap<String, &Node> = pre_children
            .iter()
            .filter_map(|node| node_id(node).map(|id| (id, node)))
            .collect();
        return reconcile_children_inner(txn, element, &pre_by_id, desired_children, &known_ids);
    }
    if current_children != desired_children {
        let len = element.len(txn);
        if len > 0 {
            element.remove_range(txn, 0, len);
        }
        let mut offset = 0u32;
        for child in desired_children {
            offset += insert_inline(txn, element, offset, child);
        }
    }
    Ok(())
}

fn insert_inline(
    txn: &mut TransactionMut<'_>,
    parent: &XmlTextRef,
    index: u32,
    node: &Node,
) -> u32 {
    apply_built(txn, parent, index, std::slice::from_ref(node));
    match node {
        Node::Text { text, .. } => utf16_len(text),
        Node::Element { .. } => 1,
    }
}

fn json_to_any(value: &Value) -> yrs::Any {
    match value {
        Value::Null => yrs::Any::Null,
        Value::Bool(value) => yrs::Any::from(*value),
        Value::Number(number) => number
            .as_i64()
            .map(yrs::Any::from)
            .unwrap_or_else(|| yrs::Any::from(number.as_f64().unwrap_or_default())),
        Value::String(value) => yrs::Any::from(value.clone()),
        Value::Array(values) => yrs::Any::from(values.iter().map(json_to_any).collect::<Vec<_>>()),
        Value::Object(map) => yrs::Any::from(
            map.iter()
                .map(|(key, value)| (key.clone(), json_to_any(value)))
                .collect::<std::collections::HashMap<_, _>>(),
        ),
    }
}

fn node_id(node: &Node) -> Option<String> {
    match node {
        Node::Element { attrs, .. } => attrs.get("id").and_then(Value::as_str).map(str::to_string),
        Node::Text { .. } => None,
    }
}

#[allow(clippy::type_complexity)]
fn current_children<T: ReadTxn>(
    txn: &T,
    parent: &XmlTextRef,
) -> Result<Vec<(Option<String>, Node, XmlTextRef)>, Unsupported> {
    use yrs::types::text::YChange;
    let fragment = xmltext_to_slate(txn, parent)?;
    let Node::Element { children, .. } = fragment else {
        return Err(Unsupported::new("session root must project to a fragment"));
    };
    let mut refs = Vec::new();
    for diff in parent.diff(txn, YChange::identity) {
        match diff.insert {
            Out::YXmlText(child) => refs.push(child),
            Out::YText(child) => {
                let child: &XmlTextRef = child.as_ref();
                refs.push(child.clone());
            }
            _ => return Err(Unsupported::new("session block level contains raw text")),
        }
    }
    if refs.len() != children.len() {
        return Err(Unsupported::new(
            "session block projection does not match embed count",
        ));
    }
    Ok(children
        .into_iter()
        .zip(refs)
        .map(|(node, text_ref)| (node_id(&node), node, text_ref))
        .collect())
}
