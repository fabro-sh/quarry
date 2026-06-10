//! Canonical block rows: the flat relational representation of a Markdown
//! document, as stored in the `blocks` table (see `quarry-storage`).
//!
//! Vocabulary (proven by the Phase Zero Gate A spike):
//!
//! - A row is one block element. Sibling order is `position` under a shared
//!   `parent_block_id` (`None` = top level). Real nesting exists only for
//!   container blocks (`code_block` → `code_line`, `table` → `tr` → `th`/`td`
//!   → `p`); Plate-style lists are flat `p` rows with `indent`/`listStyleType`
//!   attrs.
//! - Leaf rows carry flat `text` (UTF-8 storage) plus inline `marks` and
//!   `links` as ranges measured in **UTF-16 code units** to match Yjs.
//! - Block identity rides as the `id` attribute on Slate elements;
//!   `block_rows_to_nodes` places it and import mints fresh ids.
//! - Inline elements with zero flat text (wikilinks, inline images) have no
//!   row representation yet (known Phase Zero flag); a top-level block that
//!   contains one falls back to a `raw_markdown` row preserving the source.

use crate::markdown::{
    browser_compatible_markdown_options, slate_from_block_events, CRITIC_MARKERS,
};
use crate::markdown_writer::slate_to_markdown;
use crate::slate::{Attrs, Node};
use crate::Unsupported;
use pulldown_cmark::{Event, Parser};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::ops::Range;

/// Element types that are inline content inside a block, not blocks themselves.
const INLINE_LINK_TYPE: &str = "a";
/// Inline element types with no row representation (zero flat text).
const INLINE_VOID_TYPES: [&str; 2] = ["wikilink", "img"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockRow {
    pub block_id: String,
    pub parent_block_id: Option<String>,
    pub position: u32,
    pub block_type: String,
    pub attrs: Attrs,
    /// Flat block text: concatenation of all text descendants. Stored UTF-8;
    /// all offsets into it are UTF-16 code units.
    pub text: String,
    /// Disjoint, ordered formatting runs over `text` (UTF-16 offsets).
    pub marks: Vec<MarkRun>,
    /// Disjoint, ordered link ranges over `text` (UTF-16 offsets).
    pub links: Vec<LinkRange>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarkRun {
    pub start: u32,
    pub end: u32,
    pub marks: Attrs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkRange {
    pub start: u32,
    pub end: u32,
    pub url: String,
}

/// UTF-16 code-unit length of a string (the unit of all row offsets).
pub fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// Whether `offset` (UTF-16 code units) lands on a character boundary of
/// `text` — i.e. it is within bounds and does not split a surrogate pair.
pub fn is_utf16_boundary(text: &str, offset: u32) -> bool {
    let mut seen = 0u32;
    for ch in text.chars() {
        if seen == offset {
            return true;
        }
        if seen > offset {
            return false;
        }
        seen += ch.len_utf16() as u32;
    }
    seen == offset
}

/// Import: Markdown body → block rows with freshly minted `block_id`s.
///
/// CriticMarkup is unsafe (it collides with the review codec) and rejects the
/// whole document with the typed [`Unsupported`] error. Any other top-level
/// block the codec cannot represent falls back to a `raw_markdown` row
/// carrying the original source slice.
///
/// Frontmatter is the caller's concern: pass the document body only.
pub fn markdown_to_block_rows(
    markdown: &str,
    mut mint_block_id: impl FnMut() -> String,
) -> Result<Vec<BlockRow>, Unsupported> {
    if CRITIC_MARKERS
        .iter()
        .any(|marker| markdown.contains(marker))
    {
        return Err(Unsupported::new("critic markup"));
    }

    let mut rows = Vec::new();
    let mut position = 0u32;
    for segment in top_level_segments(markdown) {
        match segment_rows(&segment, position, &mut mint_block_id) {
            Ok(segment_rows) => {
                position += segment_rows
                    .iter()
                    .filter(|row| row.parent_block_id.is_none())
                    .count() as u32;
                rows.extend(segment_rows);
            }
            Err(_) => {
                let source = markdown[segment.range.clone()].trim_end_matches('\n');
                rows.push(raw_markdown_row(mint_block_id(), position, source));
                position += 1;
            }
        }
    }
    Ok(rows)
}

/// Export: block rows → Markdown. Deterministic and idempotent after one-time
/// normalization: `export(rows) == export(import(export(rows)))`.
pub fn block_rows_to_markdown(rows: &[BlockRow]) -> Result<String, Unsupported> {
    slate_to_markdown(&block_rows_to_nodes(rows)?)
}

/// Rebuild the Slate node tree from rows. Each block element carries its
/// `block_id` as the `id` attribute (the identity contract proven by Gate A).
pub fn block_rows_to_nodes(rows: &[BlockRow]) -> Result<Vec<Node>, Unsupported> {
    let nodes = children_nodes(rows, None)?;
    let placed: usize = count_nodes(&nodes);
    if placed != rows.len() {
        return Err(Unsupported::new(
            "block rows contain orphaned parent references",
        ));
    }
    Ok(nodes)
}

fn count_nodes(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|node| match node {
            Node::Element { ty, children, .. }
                if children
                    .iter()
                    .any(|child| is_block_element(ty.as_str(), child)) =>
            {
                1 + count_nodes(children)
            }
            _ => 1,
        })
        .sum()
}

fn is_block_element(_parent_ty: &str, child: &Node) -> bool {
    matches!(child, Node::Element { ty, .. }
        if ty != INLINE_LINK_TYPE && !INLINE_VOID_TYPES.contains(&ty.as_str()))
}

fn children_nodes(rows: &[BlockRow], parent: Option<&str>) -> Result<Vec<Node>, Unsupported> {
    let mut children: Vec<&BlockRow> = rows
        .iter()
        .filter(|row| row.parent_block_id.as_deref() == parent)
        .collect();
    children.sort_by_key(|row| row.position);
    children.iter().map(|row| row_to_node(rows, row)).collect()
}

fn row_to_node(rows: &[BlockRow], row: &BlockRow) -> Result<Node, Unsupported> {
    let mut node_attrs = Attrs::new();
    node_attrs.insert("id".to_string(), json!(row.block_id));
    node_attrs.extend(row.attrs.clone());
    let nested = children_nodes(rows, Some(&row.block_id))?;
    let children = if nested.is_empty() {
        inline_children(&row.text, &row.marks, &row.links)
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

fn inline_children(text: &str, marks: &[MarkRun], links: &[LinkRange]) -> Vec<Node> {
    let len = utf16_len(text);
    if len == 0 && links.is_empty() {
        return vec![Node::text("", Attrs::new())];
    }
    let mut children = Vec::new();
    let mut cursor = 0;
    for link in links {
        children.extend(marked_runs(text, marks, cursor, link.start));
        children.push(Node::element(
            INLINE_LINK_TYPE,
            crate::slate::attrs([("url", json!(link.url))]),
            marked_runs(text, marks, link.start, link.end),
        ));
        cursor = link.end;
    }
    children.extend(marked_runs(text, marks, cursor, len));
    children
}

fn marked_runs(text: &str, marks: &[MarkRun], from: u32, to: u32) -> Vec<Node> {
    let mut out = Vec::new();
    let mut cursor = from;
    while cursor < to {
        let run = marks
            .iter()
            .find(|run| run.start <= cursor && cursor < run.end);
        let next = match run {
            Some(run) => run.end.min(to),
            None => marks
                .iter()
                .map(|run| run.start)
                .filter(|start| *start > cursor)
                .min()
                .unwrap_or(to)
                .min(to),
        };
        let run_marks = run.map(|run| run.marks.clone()).unwrap_or_default();
        out.push(Node::text(utf16_slice(text, cursor, next), run_marks));
        cursor = next;
    }
    out
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
// Import: top-level segmentation and node → row conversion.
// ---------------------------------------------------------------------------

struct Segment {
    events: Vec<Event<'static>>,
    range: Range<usize>,
}

fn top_level_segments(markdown: &str) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut depth = 0usize;
    for (event, range) in
        Parser::new_ext(markdown, browser_compatible_markdown_options()).into_offset_iter()
    {
        let event = event.into_static();
        match (&event, depth) {
            (Event::Start(_), 0) => {
                segments.push(Segment {
                    events: vec![event],
                    range,
                });
                depth = 1;
            }
            (Event::End(_), _) => {
                depth = depth.saturating_sub(1);
                extend_segment(&mut segments, event, range);
            }
            (_, 0) => {
                let skippable = matches!(&event, Event::Text(text) if text.trim().is_empty())
                    || matches!(event, Event::SoftBreak | Event::HardBreak);
                if !skippable {
                    segments.push(Segment {
                        events: vec![event],
                        range,
                    });
                }
            }
            (_, _) => {
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                }
                extend_segment(&mut segments, event, range);
            }
        }
    }
    segments
}

fn extend_segment(segments: &mut [Segment], event: Event<'static>, range: Range<usize>) {
    let Some(segment) = segments.last_mut() else {
        return;
    };
    segment.events.push(event);
    segment.range.start = segment.range.start.min(range.start);
    segment.range.end = segment.range.end.max(range.end);
}

fn segment_rows(
    segment: &Segment,
    first_position: u32,
    mint_block_id: &mut impl FnMut() -> String,
) -> Result<Vec<BlockRow>, Unsupported> {
    let nodes = slate_from_block_events(segment.events.clone())?;
    let mut rows = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        collect_rows(
            node,
            None,
            first_position + index as u32,
            mint_block_id,
            &mut rows,
        )?;
    }
    Ok(rows)
}

fn collect_rows(
    node: &Node,
    parent: Option<&str>,
    position: u32,
    mint_block_id: &mut impl FnMut() -> String,
    out: &mut Vec<BlockRow>,
) -> Result<(), Unsupported> {
    let Node::Element {
        ty,
        attrs,
        children,
    } = node
    else {
        return Err(Unsupported::new("bare text node at block level"));
    };
    let block_id = mint_block_id();
    let is_container = children
        .iter()
        .any(|child| is_block_element(ty.as_str(), child));
    let mut row = BlockRow {
        block_id: block_id.clone(),
        parent_block_id: parent.map(str::to_string),
        position,
        block_type: ty.clone(),
        attrs: attrs.clone(),
        text: String::new(),
        marks: Vec::new(),
        links: Vec::new(),
    };
    if is_container {
        out.push(row);
        for (child_position, child) in children.iter().enumerate() {
            if !is_block_element(ty.as_str(), child) {
                return Err(Unsupported::new(format!(
                    "block <{ty}> mixes inline and block children"
                )));
            }
            collect_rows(
                child,
                Some(&block_id),
                child_position as u32,
                mint_block_id,
                out,
            )?;
        }
    } else {
        let (text, marks, links) = extract_inline(children)?;
        row.text = text;
        row.marks = marks;
        row.links = links;
        out.push(row);
    }
    Ok(())
}

fn extract_inline(
    children: &[Node],
) -> Result<(String, Vec<MarkRun>, Vec<LinkRange>), Unsupported> {
    let mut text = String::new();
    let mut runs: Vec<MarkRun> = Vec::new();
    let mut links = Vec::new();
    for child in children {
        match child {
            Node::Text { text: chunk, marks } => append_run(&mut text, &mut runs, chunk, marks),
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
                    append_run(&mut text, &mut runs, chunk, marks);
                }
                links.push(LinkRange {
                    start,
                    end: utf16_len(&text),
                    url,
                });
            }
            Node::Element { ty, .. } => {
                return Err(Unsupported::new(format!(
                    "inline <{ty}> has no block row representation"
                )))
            }
        }
    }
    Ok((text, runs, links))
}

fn append_run(text: &mut String, runs: &mut Vec<MarkRun>, chunk: &str, marks: &Attrs) {
    let start = utf16_len(text);
    text.push_str(chunk);
    if chunk.is_empty() || marks.is_empty() {
        return;
    }
    let end = utf16_len(text);
    if let Some(last) = runs.last_mut() {
        if last.end == start && last.marks == *marks {
            last.end = end;
            return;
        }
    }
    runs.push(MarkRun {
        start,
        end,
        marks: marks.clone(),
    });
}

fn raw_markdown_row(block_id: String, position: u32, source: &str) -> BlockRow {
    BlockRow {
        block_id,
        parent_block_id: None,
        position,
        block_type: "raw_markdown".to_string(),
        attrs: crate::slate::attrs([("markdown", json!(source))]),
        text: String::new(),
        marks: Vec::new(),
        links: Vec::new(),
    }
}
