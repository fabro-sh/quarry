//! Phase Zero — Gate A: rows ↔ session round-trip exactness.
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap to keep CRDT fixture setup readable"
)]
//!
//! Proves the two projections of the session-scoped collab design
//! (`docs/superpowers/specs/2026-06-09-session-scoped-collab-design.md`) are
//! lossless inverses, including review anchors:
//!
//! - seed:       block rows (+ anchors) → fresh Yjs session doc
//! - checkpoint: Yjs session doc → block rows (+ anchors)
//!
//! The `blocks` SQL tables do not exist yet (Phase 1), so this spike defines a
//! minimal in-test row model: `BlockRow` (stable `block_id`, parent, sibling
//! position, type, attrs, flat text, inline marks/links as UTF-16 ranges) and
//! `AnchorRow` (`{block_id, start_offset, end_offset}` in UTF-16 code units).
//!
//! Anchor conversion rules proven by these tests:
//!
//! - All offsets are UTF-16 code units. The doc is created with
//!   `OffsetKind::Utf16`, matching Yjs clock lengths, so sticky-index IDs land
//!   on code-unit boundaries and emoji (surrogate pairs) round-trip exactly.
//! - `start_offset` converts to a sticky index with `Assoc::After` placed at
//!   the first anchored character. Text inserted exactly at the start boundary
//!   is excluded: the anchor shifts right and never grows leftward.
//! - `end_offset` (exclusive) converts to a sticky index with `Assoc::Before`
//!   placed at `last anchored character + 1`. Text inserted exactly at the end
//!   boundary is excluded: the anchor never grows rightward. `Assoc::After`
//!   cannot be used for ends because `StickyIndex::at` returns `None` for
//!   `Assoc::After` at end-of-text.
//! - Inline element embeds (links) occupy one index unit in their parent text,
//!   so flat row offsets are mapped piecewise to (branch, local index) pairs;
//!   anchor endpoints may live inside link text and still resolve exactly.
//! - A fully deleted anchor range checkpoints as a collapsed range
//!   (`start == end`) at the deletion site; the row layer is expected to mark
//!   such anchors orphaned.
//! - Block identity rides as an `id` attribute on each block-level element
//!   embed. The slate-yjs doc shape preserves arbitrary element attributes, so
//!   ids survive seed → concurrent edits → checkpoint byte-for-byte.
//! - Yjs has no element move in this doc shape. A move MUST be performed as
//!   read-element → delete-embed → reinsert-clone (which preserves the `id`
//!   attribute and therefore block identity) PLUS anchor transplantation:
//!   resolve every anchor in the moved subtree to plain offsets before the
//!   move and re-create their sticky indices inside the new element after it.
//!   A naive delete+reinsert keeps block identity but strands sticky indices
//!   on the dead branch — see
//!   `naive_yjs_move_without_anchor_transplant_loses_anchor_resolution`.

use quarry_collab_codec::attrs;
use quarry_collab_codec::{Attrs as SlateAttrs, Node, apply_built, build_nodes, xmltext_to_slate};
use serde_json::{Value, json};
use std::collections::HashMap;
use yrs::branch::{Branch, BranchID};
use yrs::types::text::YChange;
use yrs::updates::decoder::Decode;
use yrs::{
    Any, Assoc, ClientID, Doc, IndexedSequence, Offset, OffsetKind, Options, Out, ReadTxn,
    StateVector, StickyIndex, Text, Transact, TransactionMut, Update, WriteTxn, Xml, XmlTextPrelim,
    XmlTextRef,
};

const ROOT: &str = "content";

/// Element types that are inline content inside a block, not blocks themselves.
const INLINE_TYPES: [&str; 1] = ["a"];

// ---------------------------------------------------------------------------
// In-test row model (stand-in for the Phase 1 `blocks` / `review_items` SQL).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct BlockRow {
    block_id: String,
    parent_block_id: Option<String>,
    position: u32,
    block_type: String,
    attrs: SlateAttrs,
    /// Flat block text: concatenation of all text descendants, UTF-16 offsets.
    text: String,
    /// Disjoint, ordered formatting runs over `text` (UTF-16 offsets).
    marks: Vec<MarkRun>,
    /// Disjoint, ordered link ranges over `text` (UTF-16 offsets).
    links: Vec<LinkRange>,
}

#[derive(Debug, Clone, PartialEq)]
struct MarkRun {
    start: u32,
    end: u32,
    marks: SlateAttrs,
}

#[derive(Debug, Clone, PartialEq)]
struct LinkRange {
    start: u32,
    end: u32,
    url: String,
}

#[derive(Debug, Clone, PartialEq)]
struct AnchorRow {
    anchor_id: String,
    block_id: String,
    start_offset: u32,
    end_offset: u32,
}

/// The live-session representation of an anchor: two yrs sticky indices.
struct SessionAnchor {
    anchor_id: String,
    start: StickyIndex,
    end: StickyIndex,
}

fn block(block_id: &str, parent: Option<&str>, position: u32, ty: &str, text: &str) -> BlockRow {
    BlockRow {
        block_id: block_id.to_string(),
        parent_block_id: parent.map(str::to_string),
        position,
        block_type: ty.to_string(),
        attrs: SlateAttrs::new(),
        text: text.to_string(),
        marks: Vec::new(),
        links: Vec::new(),
    }
}

fn mark(start: u32, end: u32, key: &str) -> MarkRun {
    MarkRun {
        start,
        end,
        marks: attrs([(key, json!(true))]),
    }
}

fn anchor(anchor_id: &str, block_id: &str, start_offset: u32, end_offset: u32) -> AnchorRow {
    AnchorRow {
        anchor_id: anchor_id.to_string(),
        block_id: block_id.to_string(),
        start_offset,
        end_offset,
    }
}

// ---------------------------------------------------------------------------
// Fixture: paragraphs, heading, (flat-Plate-style) nested list items, code
// block with nested code lines, link, inline marks, raw_markdown, emoji.
// ---------------------------------------------------------------------------

fn fixture_rows() -> Vec<BlockRow> {
    let heading = block("b-heading", None, 0, "h1", "Gate A 👍 heading");
    let intro = BlockRow {
        marks: vec![mark(11, 15, "bold"), mark(20, 26, "italic")],
        ..block("b-intro", None, 1, "p", "Intro with bold and italic runs.")
    };
    let link = BlockRow {
        links: vec![LinkRange {
            start: 8,
            end: 17,
            url: "https://example.test/docs".to_string(),
        }],
        ..block("b-link", None, 2, "p", "See the docs site for details.")
    };
    let list_one = BlockRow {
        attrs: attrs([("indent", json!(1)), ("listStyleType", json!("disc"))]),
        ..block("b-list-1", None, 3, "p", "First item")
    };
    let list_two = BlockRow {
        attrs: attrs([("indent", json!(2)), ("listStyleType", json!("disc"))]),
        ..block("b-list-2", None, 4, "p", "Nested item")
    };
    let code = BlockRow {
        attrs: attrs([("lang", json!("rust"))]),
        ..block("b-code", None, 5, "code_block", "")
    };
    let code_one = block("b-code-1", Some("b-code"), 0, "code_line", "fn main() {");
    let code_two = block(
        "b-code-2",
        Some("b-code"),
        1,
        "code_line",
        "    println!(\"hi\");",
    );
    let code_three = block("b-code-3", Some("b-code"), 2, "code_line", "}");
    let raw = BlockRow {
        attrs: attrs([("markdown", json!("::: warning\nNot supported yet.\n:::"))]),
        ..block("b-raw", None, 6, "raw_markdown", "")
    };
    let outro = block("b-outro", None, 7, "p", "Outro paragraph for end anchors.");
    vec![
        heading, intro, link, list_one, list_two, code, code_one, code_two, code_three, raw, outro,
    ]
}

fn fixture_anchors() -> Vec<AnchorRow> {
    vec![
        anchor("a-start", "b-heading", 0, 4),   // "Gate" — block start
        anchor("a-emoji", "b-heading", 7, 9),   // "👍" — surrogate pair, UTF-16
        anchor("a-mid", "b-intro", 11, 15),     // "bold" — block middle
        anchor("a-span-link", "b-link", 4, 21), // "the docs site for" — spans a link embed
        anchor("a-in-link", "b-link", 13, 17),  // "site" — both endpoints inside link text
        anchor("a-code", "b-code-2", 4, 12),    // "println!" — inside a nested child block
        anchor("a-end", "b-outro", 20, 32),     // "end anchors." — spans to block end
    ]
}

// ---------------------------------------------------------------------------
// Seed: rows → slate nodes → Yjs doc (via the production yjs_builder), then
// anchors → sticky indices.
// ---------------------------------------------------------------------------

/// Deterministic client IDs: yrs tie-breaks concurrent edits by client ID, so
/// random IDs (the `Options` default) would make interleaved-edit tests flaky.
const SERVER_CLIENT_ID: ClientID = ClientID::new(1);
const BROWSER_CLIENT_ID: ClientID = ClientID::new(2);

fn new_session_doc(client_id: ClientID) -> Doc {
    Doc::with_options(Options {
        client_id,
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    })
}

fn seed(rows: &[BlockRow], anchors: &[AnchorRow]) -> (Doc, Vec<SessionAnchor>) {
    let doc = new_session_doc(SERVER_CLIENT_ID);
    let built = build_nodes(&rows_to_nodes(rows)).expect("fixture rows build");
    {
        let mut txn = doc.transact_mut();
        let root = content_root_mut(&mut txn);
        apply_built(&mut txn, &root, 0, &built);
    }
    let session = {
        let txn = doc.transact();
        let layouts = anchor_layouts(&txn, &content_root(&txn));
        anchors
            .iter()
            .map(|anchor| place_anchor(&txn, &layouts, anchor))
            .collect()
    };
    (doc, session)
}

fn seeded_fixture() -> (Doc, Vec<SessionAnchor>) {
    seed(&fixture_rows(), &fixture_anchors())
}

fn rows_to_nodes(rows: &[BlockRow]) -> Vec<Node> {
    children_nodes(rows, None)
}

fn children_nodes(rows: &[BlockRow], parent: Option<&str>) -> Vec<Node> {
    let mut children: Vec<&BlockRow> = rows
        .iter()
        .filter(|row| row.parent_block_id.as_deref() == parent)
        .collect();
    children.sort_by_key(|row| row.position);
    children.iter().map(|row| row_to_node(rows, row)).collect()
}

fn row_to_node(rows: &[BlockRow], row: &BlockRow) -> Node {
    let mut node_attrs = SlateAttrs::new();
    node_attrs.insert("id".to_string(), json!(row.block_id));
    node_attrs.extend(row.attrs.clone());
    let nested = children_nodes(rows, Some(&row.block_id));
    let children = if nested.is_empty() {
        inline_children(&row.text, &row.marks, &row.links)
    } else {
        assert!(
            row.text.is_empty() && row.marks.is_empty() && row.links.is_empty(),
            "container block {} must not carry inline content",
            row.block_id
        );
        nested
    };
    Node::element(row.block_type.clone(), node_attrs, children)
}

fn inline_children(text: &str, marks: &[MarkRun], links: &[LinkRange]) -> Vec<Node> {
    let len = utf16_len(text);
    if len == 0 && links.is_empty() {
        return vec![Node::text("", SlateAttrs::new())];
    }
    let mut children = Vec::new();
    let mut cursor = 0;
    for link in links {
        children.extend(marked_runs(text, marks, cursor, link.start));
        children.push(Node::element(
            "a",
            attrs([("url", json!(link.url))]),
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

// ---------------------------------------------------------------------------
// Checkpoint: Yjs doc → rows (via the production xmltext_to_slate projection)
// and sticky indices → anchor offsets.
// ---------------------------------------------------------------------------

/// The slate node tree currently observable in the session doc.
fn observable_children(doc: &Doc) -> Vec<Node> {
    let txn = doc.transact();
    let fragment = xmltext_to_slate(&txn, &content_root(&txn)).expect("session doc projects");
    let Node::Element { children, .. } = fragment else {
        panic!("root projection must be a fragment");
    };
    children
}

fn checkpoint_blocks(doc: &Doc) -> Vec<BlockRow> {
    let children = observable_children(doc);
    let mut rows = Vec::new();
    for (position, child) in children.iter().enumerate() {
        collect_row(child, None, position as u32, &mut rows);
    }
    rows
}

fn collect_row(node: &Node, parent: Option<&str>, position: u32, rows: &mut Vec<BlockRow>) {
    let Node::Element {
        ty,
        attrs: node_attrs,
        children,
    } = node
    else {
        panic!("found a bare text node at block level");
    };
    let mut node_attrs = node_attrs.clone();
    let Some(Value::String(block_id)) = node_attrs.shift_remove("id") else {
        panic!("block element <{ty}> is missing a string `id` attribute");
    };
    let is_container = children.iter().any(
        |child| matches!(child, Node::Element { ty, .. } if !INLINE_TYPES.contains(&ty.as_str())),
    );
    let mut row = BlockRow {
        block_id: block_id.clone(),
        parent_block_id: parent.map(str::to_string),
        position,
        block_type: ty.clone(),
        attrs: node_attrs,
        text: String::new(),
        marks: Vec::new(),
        links: Vec::new(),
    };
    if is_container {
        rows.push(row);
        for (child_position, child) in children.iter().enumerate() {
            collect_row(child, Some(&block_id), child_position as u32, rows);
        }
    } else {
        let (text, marks, links) = extract_inline(children);
        row.text = text;
        row.marks = marks;
        row.links = links;
        rows.push(row);
    }
}

fn extract_inline(children: &[Node]) -> (String, Vec<MarkRun>, Vec<LinkRange>) {
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
            } if ty == "a" => {
                let url = link_attrs
                    .get("url")
                    .and_then(Value::as_str)
                    .expect("link element carries a string url")
                    .to_string();
                let start = utf16_len(&text);
                for inner in children {
                    let Node::Text { text: chunk, marks } = inner else {
                        panic!("links may contain only text children");
                    };
                    append_run(&mut text, &mut runs, chunk, marks);
                }
                links.push(LinkRange {
                    start,
                    end: utf16_len(&text),
                    url,
                });
            }
            Node::Element { ty, .. } => panic!("unexpected inline element <{ty}>"),
        }
    }
    (text, runs, links)
}

fn append_run(text: &mut String, runs: &mut Vec<MarkRun>, chunk: &str, marks: &SlateAttrs) {
    let start = utf16_len(text);
    text.push_str(chunk);
    if chunk.is_empty() || marks.is_empty() {
        return;
    }
    let end = utf16_len(text);
    if let Some(last) = runs.last_mut()
        && last.end == start
        && last.marks == *marks
    {
        last.end = end;
        return;
    }
    runs.push(MarkRun {
        start,
        end,
        marks: marks.clone(),
    });
}

// ---------------------------------------------------------------------------
// Anchor ↔ sticky-index conversion.
//
// Flat row offsets are UTF-16 code units over the concatenated text
// descendants of a block. Inside the Yjs doc, inline embeds (links) occupy a
// single index unit in their parent text, so conversion goes through a
// piecewise layout per block.
// ---------------------------------------------------------------------------

struct Piece {
    local_start: u32,
    flat_start: u32,
    flat_len: u32,
    /// `None` for a text run; `Some` for an inline link embed (1 local unit).
    link: Option<XmlTextRef>,
}

impl Piece {
    fn local_len(&self) -> u32 {
        if self.link.is_some() {
            1
        } else {
            self.flat_len
        }
    }
}

struct LeafLayout {
    text: XmlTextRef,
    pieces: Vec<Piece>,
    flat_len: u32,
}

enum BranchSpot {
    Block { block_id: String },
    Link { block_id: String, flat_base: u32 },
}

struct AnchorLayouts {
    leaves: HashMap<String, LeafLayout>,
    spots: HashMap<BranchID, BranchSpot>,
}

fn anchor_layouts<T: ReadTxn>(txn: &T, root: &XmlTextRef) -> AnchorLayouts {
    let mut layouts = AnchorLayouts {
        leaves: HashMap::new(),
        spots: HashMap::new(),
    };
    collect_layouts(txn, root, &mut layouts);
    layouts
}

fn collect_layouts<T: ReadTxn>(txn: &T, parent: &XmlTextRef, out: &mut AnchorLayouts) {
    for diff in parent.diff(txn, YChange::identity) {
        let Some(child) = embed_of(diff.insert) else {
            panic!("found raw text between block elements");
        };
        let block_id =
            embed_string_attr(txn, &child, "id").expect("block embed carries an `id` attribute");
        if has_block_element_children(txn, &child) {
            collect_layouts(txn, &child, out);
        } else {
            leaf_layout(txn, &block_id, &child, out);
        }
    }
}

fn has_block_element_children<T: ReadTxn>(txn: &T, parent: &XmlTextRef) -> bool {
    parent.diff(txn, YChange::identity).into_iter().any(|diff| {
        embed_of(diff.insert).is_some_and(|child| {
            let ty = embed_string_attr(txn, &child, "type").unwrap_or_default();
            !INLINE_TYPES.contains(&ty.as_str())
        })
    })
}

fn leaf_layout<T: ReadTxn>(txn: &T, block_id: &str, block: &XmlTextRef, out: &mut AnchorLayouts) {
    let mut pieces = Vec::new();
    let mut local = 0;
    let mut flat = 0;
    for diff in block.diff(txn, YChange::identity) {
        match diff.insert {
            Out::Any(Any::String(chunk)) => {
                let len = utf16_len(&chunk);
                pieces.push(Piece {
                    local_start: local,
                    flat_start: flat,
                    flat_len: len,
                    link: None,
                });
                local += len;
                flat += len;
            }
            other => {
                let link = embed_of(other).expect("leaf blocks contain text and inline embeds");
                let len = inner_text_utf16(txn, &link);
                out.spots.insert(
                    branch_id(&link),
                    BranchSpot::Link {
                        block_id: block_id.to_string(),
                        flat_base: flat,
                    },
                );
                pieces.push(Piece {
                    local_start: local,
                    flat_start: flat,
                    flat_len: len,
                    link: Some(link),
                });
                local += 1;
                flat += len;
            }
        }
    }
    out.spots.insert(
        branch_id(block),
        BranchSpot::Block {
            block_id: block_id.to_string(),
        },
    );
    out.leaves.insert(
        block_id.to_string(),
        LeafLayout {
            text: block.clone(),
            pieces,
            flat_len: flat,
        },
    );
}

fn inner_text_utf16<T: ReadTxn>(txn: &T, link: &XmlTextRef) -> u32 {
    link.diff(txn, YChange::identity)
        .into_iter()
        .map(|diff| match diff.insert {
            Out::Any(Any::String(chunk)) => utf16_len(&chunk),
            other => panic!("links may contain only text, found {other:?}"),
        })
        .sum()
}

/// Seed-side conversion: anchor offsets → sticky indices.
///
/// start: `Assoc::After` at the first anchored character.
/// end:   `Assoc::Before` at `last anchored character + 1`.
fn place_anchor<T: ReadTxn>(txn: &T, layouts: &AnchorLayouts, row: &AnchorRow) -> SessionAnchor {
    assert!(
        row.start_offset < row.end_offset,
        "anchor {} must cover at least one character at seed time",
        row.anchor_id
    );
    let layout = layouts
        .leaves
        .get(&row.block_id)
        .unwrap_or_else(|| panic!("anchor block {} is not a leaf block", row.block_id));
    let (start_text, start_local) = locate_char(layout, row.start_offset);
    let (end_text, end_local) = locate_char(layout, row.end_offset - 1);
    let start = start_text
        .sticky_index(txn, start_local, Assoc::After)
        .expect("start sticky index");
    let end = end_text
        .sticky_index(txn, end_local + 1, Assoc::Before)
        .expect("end sticky index");
    SessionAnchor {
        anchor_id: row.anchor_id.clone(),
        start,
        end,
    }
}

/// Maps the flat offset of a character to the branch and local index holding it.
fn locate_char(layout: &LeafLayout, flat: u32) -> (XmlTextRef, u32) {
    assert!(
        flat < layout.flat_len,
        "offset {flat} is past the block text (len {})",
        layout.flat_len
    );
    let piece = layout
        .pieces
        .iter()
        .find(|piece| piece.flat_start <= flat && flat < piece.flat_start + piece.flat_len)
        .expect("a piece holds every character");
    match &piece.link {
        None => (
            layout.text.clone(),
            piece.local_start + (flat - piece.flat_start),
        ),
        Some(link) => (link.clone(), flat - piece.flat_start),
    }
}

/// Checkpoint-side conversion: sticky indices → anchor offsets. Returns `None`
/// when an endpoint no longer resolves into any live block (e.g. its block was
/// deleted, or it was stranded by a naive move).
fn resolve_anchor<T: ReadTxn>(
    txn: &T,
    layouts: &AnchorLayouts,
    session: &SessionAnchor,
) -> Option<AnchorRow> {
    let start = session.start.get_offset(txn)?;
    let end = session.end.get_offset(txn)?;
    let (start_block, start_offset) = locate_flat(layouts, &start)?;
    let (end_block, end_offset) = locate_flat(layouts, &end)?;
    assert_eq!(
        start_block, end_block,
        "anchor {} endpoints resolved into different blocks",
        session.anchor_id
    );
    Some(AnchorRow {
        anchor_id: session.anchor_id.clone(),
        block_id: start_block,
        start_offset,
        end_offset,
    })
}

fn checkpoint_anchor(doc: &Doc, session: &SessionAnchor) -> Option<AnchorRow> {
    let txn = doc.transact();
    let layouts = anchor_layouts(&txn, &content_root(&txn));
    resolve_anchor(&txn, &layouts, session)
}

fn locate_flat(layouts: &AnchorLayouts, offset: &Offset) -> Option<(String, u32)> {
    match layouts.spots.get(&offset.branch.id())? {
        BranchSpot::Link {
            block_id,
            flat_base,
        } => Some((block_id.clone(), flat_base + offset.index)),
        BranchSpot::Block { block_id } => {
            let layout = &layouts.leaves[block_id];
            Some((block_id.clone(), local_to_flat(layout, offset.index)))
        }
    }
}

fn local_to_flat(layout: &LeafLayout, local: u32) -> u32 {
    for piece in &layout.pieces {
        if local <= piece.local_start {
            return piece.flat_start;
        }
        if local < piece.local_start + piece.local_len() {
            // Only text pieces have interior positions (links are 1 unit wide).
            return piece.flat_start + (local - piece.local_start);
        }
    }
    layout.flat_len
}

// ---------------------------------------------------------------------------
// Session plumbing shared by the tests.
// ---------------------------------------------------------------------------

fn content_root<T: ReadTxn>(txn: &T) -> XmlTextRef {
    let text = txn.get_text(ROOT).expect("content root exists");
    let root: &XmlTextRef = text.as_ref();
    root.clone()
}

fn content_root_mut(txn: &mut TransactionMut<'_>) -> XmlTextRef {
    let text = txn.get_or_insert_text(ROOT);
    let root: &XmlTextRef = text.as_ref();
    root.clone()
}

/// Simulates a concurrent browser edit: clone the server state into a second
/// Yjs client, apply the edit there, and merge the resulting update back.
fn apply_browser_edit(server: &Doc, edit: impl FnOnce(&mut TransactionMut<'_>, &XmlTextRef)) {
    let browser = new_session_doc(BROWSER_CLIENT_ID);
    let snapshot = server
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    browser
        .transact_mut()
        .apply_update(Update::decode_v1(&snapshot).expect("snapshot decodes"))
        .expect("snapshot applies");
    let server_state = server.transact().state_vector();
    {
        let mut txn = browser.transact_mut();
        let root = content_root_mut(&mut txn);
        edit(&mut txn, &root);
    }
    let delta = browser.transact().encode_state_as_update_v1(&server_state);
    server
        .transact_mut()
        .apply_update(Update::decode_v1(&delta).expect("delta decodes"))
        .expect("delta applies");
}

fn embed_of(out: Out) -> Option<XmlTextRef> {
    match out {
        Out::YXmlText(child) => Some(child),
        Out::YText(child) => {
            let child: &XmlTextRef = child.as_ref();
            Some(child.clone())
        }
        _ => None,
    }
}

fn embed_string_attr<T: ReadTxn>(txn: &T, child: &XmlTextRef, key: &str) -> Option<String> {
    match child.get_attribute(txn, key) {
        Some(Out::Any(Any::String(value))) => Some(value.to_string()),
        _ => None,
    }
}

fn branch_id(text: &XmlTextRef) -> BranchID {
    let branch: &Branch = text.as_ref();
    branch.id()
}

fn block_text<T: ReadTxn>(txn: &T, root: &XmlTextRef, block_id: &str) -> XmlTextRef {
    find_block_text(txn, root, block_id)
        .unwrap_or_else(|| panic!("block {block_id} not found in session doc"))
}

fn find_block_text<T: ReadTxn>(txn: &T, parent: &XmlTextRef, block_id: &str) -> Option<XmlTextRef> {
    for diff in parent.diff(txn, YChange::identity) {
        let Some(child) = embed_of(diff.insert) else {
            continue;
        };
        if embed_string_attr(txn, &child, "id").as_deref() == Some(block_id) {
            return Some(child);
        }
        if let Some(found) = find_block_text(txn, &child, block_id) {
            return Some(found);
        }
    }
    None
}

fn first_link_text<T: ReadTxn>(txn: &T, root: &XmlTextRef, block_id: &str) -> XmlTextRef {
    let block = block_text(txn, root, block_id);
    block
        .diff(txn, YChange::identity)
        .into_iter()
        .find_map(|diff| embed_of(diff.insert))
        .expect("block contains a link embed")
}

fn session_anchor<'a>(anchors: &'a [SessionAnchor], anchor_id: &str) -> &'a SessionAnchor {
    anchors
        .iter()
        .find(|anchor| anchor.anchor_id == anchor_id)
        .unwrap_or_else(|| panic!("unknown session anchor {anchor_id}"))
}

/// Recomputes sibling positions from row order (used to build expected rows
/// after structural edits).
fn renumbered(mut rows: Vec<BlockRow>) -> Vec<BlockRow> {
    let mut counters: HashMap<Option<String>, u32> = HashMap::new();
    for row in &mut rows {
        let counter = counters.entry(row.parent_block_id.clone()).or_insert(0);
        row.position = *counter;
        *counter += 1;
    }
    rows
}

// ---------------------------------------------------------------------------
// Block move. Yjs has no element move in the slate-yjs doc shape; the only
// primitive is delete + reinsert of a clone. `yjs_move_block` is that raw
// primitive (what an uncoordinated client would do); the anchor-preserving
// move resolves anchors in the moved subtree to plain offsets first and
// re-creates their sticky indices in the reinserted element afterwards.
// `to_index` is the top-level index in the document *after* removal.
// ---------------------------------------------------------------------------

fn read_top_level_block(doc: &Doc, block_id: &str) -> (Node, u32) {
    let children = observable_children(doc);
    let from_index = children
        .iter()
        .position(|child| {
            matches!(child, Node::Element { attrs, .. }
                if attrs.get("id").and_then(Value::as_str) == Some(block_id))
        })
        .unwrap_or_else(|| panic!("block {block_id} is not top-level"));
    (children[from_index].clone(), from_index as u32)
}

fn yjs_move_block(doc: &Doc, block_id: &str, to_index: u32) {
    let (node, from_index) = read_top_level_block(doc, block_id);
    let mut txn = doc.transact_mut();
    let root = content_root_mut(&mut txn);
    root.remove_range(&mut txn, from_index, 1);
    apply_built(&mut txn, &root, to_index, std::slice::from_ref(&node));
}

fn subtree_block_ids(node: &Node) -> Vec<String> {
    let Node::Element {
        attrs, children, ..
    } = node
    else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    if let Some(id) = attrs.get("id").and_then(Value::as_str) {
        ids.push(id.to_string());
    }
    for child in children {
        ids.extend(subtree_block_ids(child));
    }
    ids
}

fn move_block_preserving_anchors(
    doc: &Doc,
    session_anchors: &mut [SessionAnchor],
    block_id: &str,
    to_index: u32,
) {
    let (node, _) = read_top_level_block(doc, block_id);
    let moved_ids = subtree_block_ids(&node);
    let transplant: Vec<(usize, AnchorRow)> = {
        let txn = doc.transact();
        let layouts = anchor_layouts(&txn, &content_root(&txn));
        session_anchors
            .iter()
            .enumerate()
            .filter_map(|(index, session)| {
                let row =
                    resolve_anchor(&txn, &layouts, session).expect("anchors resolve before move");
                moved_ids.contains(&row.block_id).then_some((index, row))
            })
            .collect()
    };
    yjs_move_block(doc, block_id, to_index);
    let txn = doc.transact();
    let layouts = anchor_layouts(&txn, &content_root(&txn));
    for (index, row) in transplant {
        session_anchors[index] = place_anchor(&txn, &layouts, &row);
    }
}

// ---------------------------------------------------------------------------
// UTF-16 helpers (row offsets are UTF-16 code units, Rust strings are UTF-8).
// ---------------------------------------------------------------------------

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

fn utf16_slice(text: &str, from: u32, to: u32) -> String {
    text[byte_of_utf16(text, from)..byte_of_utf16(text, to)].to_string()
}

fn byte_of_utf16(text: &str, target: u32) -> usize {
    let mut seen = 0;
    for (byte_index, ch) in text.char_indices() {
        if seen == target {
            return byte_index;
        }
        assert!(seen < target, "offset {target} splits a surrogate pair");
        seen += ch.len_utf16() as u32;
    }
    assert_eq!(seen, target, "UTF-16 offset {target} is out of bounds");
    text.len()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn identity_round_trip_preserves_blocks_and_anchor_offsets_exactly() {
    let (doc, session_anchors) = seeded_fixture();

    // Pin the seeded slate/Yjs shape against an independent hand-written
    // literal. The row → inline encoding and its inverse are author-mirrored,
    // so a symmetric bug there would self-cancel in the round-trip assertions
    // below; this literal breaks that self-confirmation window.
    assert_eq!(
        serde_json::to_value(observable_children(&doc)).unwrap(),
        json!([
            {
                "type": "h1",
                "id": "b-heading",
                "children": [{ "text": "Gate A 👍 heading" }]
            },
            {
                "type": "p",
                "id": "b-intro",
                "children": [
                    { "text": "Intro with " },
                    { "bold": true, "text": "bold" },
                    { "text": " and " },
                    { "italic": true, "text": "italic" },
                    { "text": " runs." }
                ]
            },
            {
                "type": "p",
                "id": "b-link",
                "children": [
                    { "text": "See the " },
                    {
                        "type": "a",
                        "url": "https://example.test/docs",
                        "children": [{ "text": "docs site" }]
                    },
                    { "text": " for details." }
                ]
            },
            {
                "type": "p",
                "id": "b-list-1",
                "indent": 1,
                "listStyleType": "disc",
                "children": [{ "text": "First item" }]
            },
            {
                "type": "p",
                "id": "b-list-2",
                "indent": 2,
                "listStyleType": "disc",
                "children": [{ "text": "Nested item" }]
            },
            {
                "type": "code_block",
                "id": "b-code",
                "lang": "rust",
                "children": [
                    {
                        "type": "code_line",
                        "id": "b-code-1",
                        "children": [{ "text": "fn main() {" }]
                    },
                    {
                        "type": "code_line",
                        "id": "b-code-2",
                        "children": [{ "text": "    println!(\"hi\");" }]
                    },
                    {
                        "type": "code_line",
                        "id": "b-code-3",
                        "children": [{ "text": "}" }]
                    }
                ]
            },
            {
                "type": "raw_markdown",
                "id": "b-raw",
                "markdown": "::: warning\nNot supported yet.\n:::",
                "children": [{ "text": "" }]
            },
            {
                "type": "p",
                "id": "b-outro",
                "children": [{ "text": "Outro paragraph for end anchors." }]
            }
        ])
    );

    let rows = checkpoint_blocks(&doc);
    assert_eq!(rows, fixture_rows());
    let ids: Vec<&str> = rows.iter().map(|row| row.block_id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "b-heading",
            "b-intro",
            "b-link",
            "b-list-1",
            "b-list-2",
            "b-code",
            "b-code-1",
            "b-code-2",
            "b-code-3",
            "b-raw",
            "b-outro"
        ]
    );

    let a = |id| checkpoint_anchor(&doc, session_anchor(&session_anchors, id));
    assert_eq!(a("a-start"), Some(anchor("a-start", "b-heading", 0, 4)));
    assert_eq!(a("a-emoji"), Some(anchor("a-emoji", "b-heading", 7, 9)));
    assert_eq!(a("a-mid"), Some(anchor("a-mid", "b-intro", 11, 15)));
    assert_eq!(
        a("a-span-link"),
        Some(anchor("a-span-link", "b-link", 4, 21))
    );
    assert_eq!(a("a-in-link"), Some(anchor("a-in-link", "b-link", 13, 17)));
    assert_eq!(a("a-code"), Some(anchor("a-code", "b-code-2", 4, 12)));
    assert_eq!(a("a-end"), Some(anchor("a-end", "b-outro", 20, 32)));
}

#[test]
fn concurrent_text_inserts_resolve_anchor_offsets_through_the_crdt() {
    let (doc, session_anchors) = seeded_fixture();

    // a-mid covers "bold" at [11, 15) in b-intro.
    apply_browser_edit(&doc, |txn, root| {
        let intro = block_text(txn, root, "b-intro");
        intro.insert(txn, 0, "XX"); // before the anchor
        intro.insert(txn, 15, "!!!"); // inside the anchor ("bo|ld", shifted by 2)
        intro.insert(txn, 37, "ZZ"); // after the anchor, at end of text
    });

    let mut expected = fixture_rows();
    expected[1].text = "XXIntro with bo!!!ld and italic runs.ZZ".to_string();
    // Plain text inserted inside a formatted span inherits its formatting
    // (Yjs format markers delimit the range), so the bold run grows.
    expected[1].marks = vec![mark(13, 20, "bold"), mark(25, 31, "italic")];
    assert_eq!(checkpoint_blocks(&doc), expected);

    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-mid")),
        Some(anchor("a-mid", "b-intro", 13, 20))
    );
    // Anchors in untouched blocks are unaffected.
    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-end")),
        Some(anchor("a-end", "b-outro", 20, 32))
    );
}

#[test]
fn anchor_at_block_start_excludes_text_inserted_at_its_start_boundary() {
    let (doc, session_anchors) = seeded_fixture();

    apply_browser_edit(&doc, |txn, root| {
        let heading = block_text(txn, root, "b-heading");
        heading.insert(txn, 0, "NEW ");
    });

    let mut expected = fixture_rows();
    expected[0].text = "NEW Gate A 👍 heading".to_string();
    assert_eq!(checkpoint_blocks(&doc), expected);

    // The anchor still covers exactly "Gate": it shifted right, it did not grow.
    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-start")),
        Some(anchor("a-start", "b-heading", 4, 8))
    );
    // Surrogate-pair offsets stay exact in UTF-16 code units.
    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-emoji")),
        Some(anchor("a-emoji", "b-heading", 11, 13))
    );
}

#[test]
fn anchor_ending_at_block_end_excludes_text_appended_at_its_end_boundary() {
    let (doc, session_anchors) = seeded_fixture();

    apply_browser_edit(&doc, |txn, root| {
        let outro = block_text(txn, root, "b-outro");
        outro.insert(txn, 32, " MORE");
    });

    let mut expected = fixture_rows();
    expected[10].text = "Outro paragraph for end anchors. MORE".to_string();
    assert_eq!(checkpoint_blocks(&doc), expected);

    // The anchor still covers exactly "end anchors.": appended text excluded.
    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-end")),
        Some(anchor("a-end", "b-outro", 20, 32))
    );
}

#[test]
fn inserting_a_block_between_blocks_preserves_ids_positions_and_anchors() {
    let (doc, session_anchors) = seeded_fixture();

    apply_browser_edit(&doc, |txn, root| {
        let inserted = root.insert_embed(txn, 1, XmlTextPrelim::default());
        inserted.insert_attribute(txn, "type", Any::from("p"));
        inserted.insert_attribute(txn, "id", Any::from("b-inserted"));
        inserted.insert(txn, 0, "Inserted between.");
    });

    let mut expected = fixture_rows();
    expected.insert(1, block("b-inserted", None, 1, "p", "Inserted between."));
    assert_eq!(checkpoint_blocks(&doc), renumbered(expected));

    // Structural insertion does not disturb text anchors in sibling blocks.
    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-mid")),
        Some(anchor("a-mid", "b-intro", 11, 15))
    );
    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-code")),
        Some(anchor("a-code", "b-code-2", 4, 12))
    );
    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-end")),
        Some(anchor("a-end", "b-outro", 20, 32))
    );
}

#[test]
fn moving_a_block_preserves_identity_and_anchors_when_transplanted() {
    let (doc, mut session_anchors) = seeded_fixture();

    // Move b-intro (top-level index 1) to the end of the document.
    move_block_preserving_anchors(&doc, &mut session_anchors, "b-intro", 7);

    let mut expected = fixture_rows();
    let intro = expected.remove(1);
    expected.push(intro);
    assert_eq!(checkpoint_blocks(&doc), renumbered(expected));

    // Same block_id, same offsets, in the moved block.
    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-mid")),
        Some(anchor("a-mid", "b-intro", 11, 15))
    );
    // Anchors in unmoved blocks were never disturbed.
    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-end")),
        Some(anchor("a-end", "b-outro", 20, 32))
    );
}

#[test]
fn naive_yjs_move_without_anchor_transplant_loses_anchor_resolution() {
    let (doc, session_anchors) = seeded_fixture();

    // Raw delete + reinsert, the only move primitive this doc shape offers.
    yjs_move_block(&doc, "b-intro", 7);

    // Block identity survives (the `id` attribute is cloned with the element)…
    let mut expected = fixture_rows();
    let intro = expected.remove(1);
    expected.push(intro);
    assert_eq!(checkpoint_blocks(&doc), renumbered(expected));

    // …but the sticky indices are stranded on the deleted element's branch:
    // the anchor no longer resolves into any live block. This is the Gate A
    // finding that makes anchor transplantation a hard requirement of move.
    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-mid")),
        None
    );
}

#[test]
fn fully_deleted_anchor_range_collapses_to_a_point_at_the_deletion_site() {
    let (doc, session_anchors) = seeded_fixture();

    // Delete exactly the anchored range "bold" at [11, 15) in b-intro.
    apply_browser_edit(&doc, |txn, root| {
        let intro = block_text(txn, root, "b-intro");
        intro.remove_range(txn, 11, 4);
    });

    let mut expected = fixture_rows();
    expected[1].text = "Intro with  and italic runs.".to_string();
    expected[1].marks = vec![mark(16, 22, "italic")];
    assert_eq!(checkpoint_blocks(&doc), expected);

    // Rule: a fully deleted range checkpoints as a collapsed (start == end)
    // anchor at the deletion site; the row layer marks it orphaned.
    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-mid")),
        Some(anchor("a-mid", "b-intro", 11, 11))
    );
}

#[test]
fn insert_inside_link_text_keeps_anchors_exact_across_inline_embeds() {
    let (doc, session_anchors) = seeded_fixture();

    // b-link text: "See the docs site for details." with link "docs site" at
    // [8, 17). Insert at the start of the link's own text.
    apply_browser_edit(&doc, |txn, root| {
        let link = first_link_text(txn, root, "b-link");
        link.insert(txn, 0, "best ");
    });

    let mut expected = fixture_rows();
    expected[2].text = "See the best docs site for details.".to_string();
    expected[2].links = vec![LinkRange {
        start: 8,
        end: 22,
        url: "https://example.test/docs".to_string(),
    }];
    assert_eq!(checkpoint_blocks(&doc), expected);

    // a-span-link covered "the docs site for" at [4, 21): its start (parent
    // text) holds, its end (parent text after the link) shifts by the insert.
    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-span-link")),
        Some(anchor("a-span-link", "b-link", 4, 26))
    );
    // a-in-link covered "site" inside the link at [13, 17): both endpoints
    // live inside the link branch and shift together.
    assert_eq!(
        checkpoint_anchor(&doc, session_anchor(&session_anchors, "a-in-link")),
        Some(anchor("a-in-link", "b-link", 18, 22))
    );
}
