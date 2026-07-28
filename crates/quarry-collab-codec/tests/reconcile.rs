//! Production diff3 reconciliation — the Gate C spike scenarios
#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for reconciliation fixtures"
)]
//! (`phase_zero_gate_c.rs`, kept untouched) promoted against
//! the `quarry_collab_codec::reconcile` facade export, plus the production additions:
//! `set_block_type` pairing (approved improvement over the spike, which
//! lacked the op), the container and raw_markdown pairing boundaries, the
//! canonical empty-paragraph rule, equality-semantics pins (marks/attrs), and
//! the bounded-LCS degraded mode end to end.
//!
//! Every test cross-checks the reconciler's positions with `apply_ops`, an
//! independent implementation of the documented rule-8 application
//! semantics: ID-addressed ops, then deletes, then detach every move target,
//! then place moves and inserts at their stated final indices ascending.

use quarry_collab_codec::attrs;
use quarry_collab_codec::{
    Attrs, BlockRow, MarkRun, ReconcileBase, ReconcileConflict, ReconcileOp, ReconcileOutcome,
    markdown_to_block_rows, reconcile,
};
use serde_json::json;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Fixtures and helpers.
// ---------------------------------------------------------------------------

const BASE: &str = "# Title\n\nAlpha paragraph.\n\nBravo paragraph.\n\nCharlie paragraph.\n";
const BASE_IDS: [&str; 4] = ["b-title", "b-alpha", "b-bravo", "b-charlie"];

fn parse_rows(markdown: &str) -> Vec<BlockRow> {
    let mut next = 0usize;
    markdown_to_block_rows(markdown, || {
        next += 1;
        format!("tmp-{next}")
    })
    .expect("fixtures use supported markdown")
}

/// Canonical rows for `markdown` with the given top-level block ids.
fn canonical_doc(markdown: &str, ids: &[&str]) -> Vec<BlockRow> {
    let rows = parse_rows(markdown);
    let top_ids: Vec<String> = rows
        .iter()
        .filter(|row| row.parent_block_id.is_none())
        .map(|row| row.block_id.clone())
        .collect();
    assert_eq!(top_ids.len(), ids.len(), "fixture block/id count mismatch");
    let renames: HashMap<String, String> = top_ids
        .into_iter()
        .zip(ids.iter().map(|id| id.to_string()))
        .collect();
    rows.into_iter()
        .map(|mut row| {
            if let Some(new_id) = renames.get(&row.block_id) {
                row.block_id = new_id.clone();
            }
            if let Some(parent) = &row.parent_block_id
                && let Some(new_id) = renames.get(parent)
            {
                row.parent_block_id = Some(new_id.clone());
            }
            row
        })
        .collect()
}

fn base_canonical() -> Vec<BlockRow> {
    canonical_doc(BASE, &BASE_IDS)
}

fn run(base: ReconcileBase<'_>, incoming: &str, canonical: &[BlockRow]) -> ReconcileOutcome {
    let mut fresh = 0usize;
    reconcile(base, incoming, canonical, || {
        fresh += 1;
        format!("fresh-{fresh}")
    })
    .expect("fixtures use supported markdown")
}

fn fresh_block(block_id: &str, block_type: &str, text: &str) -> Vec<BlockRow> {
    vec![BlockRow {
        block_id: block_id.to_string(),
        parent_block_id: None,
        position: 0,
        block_type: block_type.to_string(),
        attrs: Attrs::new(),
        text: text.to_string(),
        marks: Vec::new(),
        links: Vec::new(),
    }]
}

fn fresh_paragraph(block_id: &str, text: &str) -> Vec<BlockRow> {
    fresh_block(block_id, "p", text)
}

fn replace(block_id: &str, text: &str) -> ReconcileOp {
    ReconcileOp::ReplaceBlockContent {
        block_id: block_id.to_string(),
        text: text.to_string(),
        marks: Vec::new(),
        links: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Test-side op applicator: an independent interpretation of the rule-8
// application semantics, used to cross-check every emitted position.
// ---------------------------------------------------------------------------

/// One merged top-level block: its id plus subtree rows.
#[derive(Debug, Clone)]
struct MergedBlock {
    id: String,
    rows: Vec<BlockRow>,
}

fn group_blocks(rows: &[BlockRow]) -> Vec<MergedBlock> {
    let mut tops: Vec<MergedBlock> = rows
        .iter()
        .filter(|row| row.parent_block_id.is_none())
        .map(|row| MergedBlock {
            id: row.block_id.clone(),
            rows: vec![row.clone()],
        })
        .collect();
    tops.sort_by_key(|top| top.rows[0].position);
    for top in &mut tops {
        let mut frontier = vec![top.id.clone()];
        while let Some(parent) = frontier.pop() {
            let mut children: Vec<BlockRow> = rows
                .iter()
                .filter(|row| row.parent_block_id.as_deref() == Some(parent.as_str()))
                .cloned()
                .collect();
            children.sort_by_key(|row| row.position);
            frontier.extend(children.iter().map(|child| child.block_id.clone()));
            top.rows.extend(children);
        }
    }
    tops
}

/// ID-addressed ops may target any row (rule 6a recurses into containers),
/// so the lookup spans every subtree, not just top-level rows.
fn row_mut<'a>(blocks: &'a mut [MergedBlock], block_id: &str) -> &'a mut BlockRow {
    blocks
        .iter_mut()
        .flat_map(|block| block.rows.iter_mut())
        .find(|row| row.block_id == block_id)
        .expect("op targets an existing row")
}

/// Removes a child row and its descendants from its subtree, closing the
/// sibling position gap (rule 8: deletes apply before placements).
fn remove_child_row(block: &mut MergedBlock, block_id: &str) {
    let removed = block
        .rows
        .iter()
        .find(|row| row.block_id == block_id)
        .cloned()
        .expect("deleted child row exists");
    let mut doomed: Vec<String> = vec![removed.block_id.clone()];
    let mut frontier = vec![removed.block_id.clone()];
    while let Some(parent) = frontier.pop() {
        for row in &block.rows {
            if row.parent_block_id.as_deref() == Some(parent.as_str()) {
                doomed.push(row.block_id.clone());
                frontier.push(row.block_id.clone());
            }
        }
    }
    block.rows.retain(|row| !doomed.contains(&row.block_id));
    for row in &mut block.rows {
        if row.parent_block_id == removed.parent_block_id && row.position > removed.position {
            row.position -= 1;
        }
    }
}

fn apply_ops(canonical: &[BlockRow], ops: &[ReconcileOp]) -> Vec<MergedBlock> {
    let mut blocks = group_blocks(canonical);
    for op in ops {
        match op {
            ReconcileOp::ReplaceBlockContent {
                block_id,
                text,
                marks,
                links,
            } => {
                let row = row_mut(&mut blocks, block_id);
                row.text = text.clone();
                row.marks = marks.clone();
                row.links = links.clone();
            }
            ReconcileOp::SetBlockType {
                block_id,
                block_type,
                attrs,
            } => {
                let row = row_mut(&mut blocks, block_id);
                row.block_type = block_type.clone();
                if let Some(attrs) = attrs {
                    row.attrs = attrs.clone();
                }
            }
            ReconcileOp::SetBlockAttrs { block_id, attrs } => {
                row_mut(&mut blocks, block_id).attrs = attrs.clone();
            }
            ReconcileOp::DeleteBlock { block_id } => {
                if let Some(index) = blocks.iter().position(|block| block.id == *block_id) {
                    blocks.remove(index);
                } else {
                    let block = blocks
                        .iter_mut()
                        .find(|block| block.rows.iter().any(|row| row.block_id == *block_id))
                        .expect("deleted block exists");
                    remove_child_row(block, block_id);
                }
            }
            ReconcileOp::InsertBlock { .. }
            | ReconcileOp::MoveBlock { .. }
            | ReconcileOp::InsertChild { .. } => {}
        }
    }
    let mut placements: Vec<(usize, MergedBlock)> = Vec::new();
    for op in ops {
        if let ReconcileOp::MoveBlock { block_id, position } = op {
            let index = blocks
                .iter()
                .position(|block| block.id == *block_id)
                .expect("moved block exists");
            placements.push((*position, blocks.remove(index)));
        }
    }
    for op in ops {
        if let ReconcileOp::InsertBlock { position, rows } = op {
            placements.push((
                *position,
                MergedBlock {
                    id: rows[0].block_id.clone(),
                    rows: rows.clone(),
                },
            ));
        }
    }
    placements.sort_by_key(|(position, _)| *position);
    for (position, block) in placements {
        blocks.insert(position, block);
    }
    // Child inserts last (rule 8), ascending per parent: bump existing
    // siblings at or past the final index, then splice the subtree in with
    // its root re-parented to the op-level parent.
    let mut child_inserts: Vec<(&String, usize, &Vec<BlockRow>)> = ops
        .iter()
        .filter_map(|op| match op {
            ReconcileOp::InsertChild {
                parent_block_id,
                position,
                rows,
            } => Some((parent_block_id, *position, rows)),
            _ => None,
        })
        .collect();
    child_inserts.sort_by_key(|(parent, position, _)| (parent.as_str(), *position));
    for (parent_block_id, position, rows) in child_inserts {
        let block = blocks
            .iter_mut()
            .find(|block| {
                block
                    .rows
                    .iter()
                    .any(|row| row.block_id == *parent_block_id)
            })
            .expect("child insert targets a surviving parent");
        for row in &mut block.rows {
            if row.parent_block_id.as_deref() == Some(parent_block_id.as_str())
                && (row.position as usize) >= position
            {
                row.position += 1;
            }
        }
        let mut inserted = rows.clone();
        inserted[0].parent_block_id = Some(parent_block_id.clone());
        inserted[0].position = position as u32;
        block.rows.extend(inserted);
    }
    blocks
}

/// The merged child sequence of `parent_id` as `(block_id, text)` pairs in
/// sibling order.
fn children_of(blocks: &[MergedBlock], parent_id: &str) -> Vec<(String, String)> {
    let mut children: Vec<&BlockRow> = blocks
        .iter()
        .flat_map(|block| block.rows.iter())
        .filter(|row| row.parent_block_id.as_deref() == Some(parent_id))
        .collect();
    children.sort_by_key(|row| row.position);
    children
        .iter()
        .map(|row| (row.block_id.clone(), row.text.clone()))
        .collect()
}

fn ids_of(blocks: &[MergedBlock]) -> Vec<&str> {
    blocks.iter().map(|block| block.id.as_str()).collect()
}

/// Every surviving row id, descendants included. `ids_of` only reports
/// top-level blocks, but review anchors attach to any row — a code line, a
/// table cell — so identity questions have to look at the whole subtree.
fn surviving_ids(blocks: &[MergedBlock]) -> Vec<String> {
    blocks
        .iter()
        .flat_map(|block| block.rows.iter())
        .map(|row| row.block_id.clone())
        .collect()
}

/// The id of the canonical row carrying exactly `text`, for anchoring
/// identity assertions to content rather than to fixture id bookkeeping.
fn surviving_id_of(canonical: &[BlockRow], text: &str) -> String {
    let matches: Vec<&BlockRow> = canonical.iter().filter(|row| row.text == text).collect();
    assert_eq!(
        matches.len(),
        1,
        "fixture must contain exactly one row with text {text:?}"
    );
    matches[0].block_id.clone()
}

fn text_of(blocks: &[MergedBlock], index: usize) -> &str {
    &blocks[index].rows[0].text
}

fn type_of(blocks: &[MergedBlock], index: usize) -> &str {
    &blocks[index].rows[0].block_type
}

// ---------------------------------------------------------------------------
// Anchor fate at block granularity (the Gate C rule the gateway's
// finer-grained minimal-diff anchor handling subsumes).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum ReviewKind {
    Comment,
    Suggestion,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AnchorFate {
    Untouched,
    Orphaned,
    Invalidated,
}

fn anchor_fate(kind: ReviewKind, block_id: &str, ops: &[ReconcileOp]) -> AnchorFate {
    let block_changed = ops.iter().any(|op| match op {
        ReconcileOp::ReplaceBlockContent { block_id: id, .. }
        | ReconcileOp::DeleteBlock { block_id: id } => id == block_id,
        ReconcileOp::SetBlockType { .. }
        | ReconcileOp::SetBlockAttrs { .. }
        | ReconcileOp::InsertBlock { .. }
        | ReconcileOp::MoveBlock { .. }
        | ReconcileOp::InsertChild { .. } => false,
    });
    if !block_changed {
        return AnchorFate::Untouched;
    }
    match kind {
        ReviewKind::Comment => AnchorFate::Orphaned,
        ReviewKind::Suggestion => AnchorFate::Invalidated,
    }
}

// ---------------------------------------------------------------------------
// Identity preservation through positional base mapping.
// ---------------------------------------------------------------------------

#[test]
fn unchanged_document_produces_no_ops_and_preserves_all_block_ids() {
    let canonical = base_canonical();

    let result = run(ReconcileBase::Markdown(BASE), BASE, &canonical);

    assert_eq!(result.ops, []);
    assert_eq!(result.conflicts, []);
    assert_eq!(ids_of(&apply_ops(&canonical, &result.ops)), BASE_IDS);
}

/// Canonical-only drift, the most common production case: a surface writes
/// the file back unchanged (incoming == base) after a browser session edited
/// one block and inserted another. The whole canonical state — edits, the new
/// block, and every ID — is kept verbatim; no ops, no conflict.
#[test]
fn canonical_only_drift_keeps_canonical_with_no_ops_and_no_conflicts() {
    let canonical = canonical_doc(
        "# Title\n\nAlpha paragraph.\n\nBravo paragraph, canonical edit.\n\nNew canonical paragraph.\n\nCharlie paragraph.\n",
        &["b-title", "b-alpha", "b-bravo", "b-new", "b-charlie"],
    );

    let result = run(ReconcileBase::Markdown(BASE), BASE, &canonical);

    assert_eq!(result.ops, []);
    assert_eq!(result.conflicts, []);
    assert_eq!(
        ids_of(&apply_ops(&canonical, &result.ops)),
        ["b-title", "b-alpha", "b-bravo", "b-new", "b-charlie"]
    );
}

/// Convergent change: incoming and canonical both turned the same base block
/// into the same text (incoming == canonical ≠ base). Nothing to do.
#[test]
fn convergent_identical_changes_produce_no_ops_and_no_conflicts() {
    let canonical = canonical_doc(
        "# Title\n\nAlpha paragraph.\n\nBravo paragraph, same edit.\n\nCharlie paragraph.\n",
        &BASE_IDS,
    );
    let incoming =
        "# Title\n\nAlpha paragraph.\n\nBravo paragraph, same edit.\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(result.ops, []);
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(text_of(&merged, 2), "Bravo paragraph, same edit.");
}

#[test]
fn edited_block_emits_one_replace_on_the_base_mapped_id_and_leaves_siblings_untouched() {
    let canonical = base_canonical();
    let incoming =
        "# Title\n\nAlpha paragraph.\n\nBravo paragraph, edited.\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(result.ops, [replace("b-bravo", "Bravo paragraph, edited.")]);
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(ids_of(&merged), BASE_IDS);
    assert_eq!(text_of(&merged, 2), "Bravo paragraph, edited.");
    assert_eq!(text_of(&merged, 1), "Alpha paragraph.");
    assert_eq!(text_of(&merged, 3), "Charlie paragraph.");
}

#[test]
fn attrs_only_change_emits_set_block_attrs_without_touching_content() {
    let base = "- First item\n- Second item\n";
    let canonical = canonical_doc(base, &["b-first", "b-second"]);
    let incoming = "- First item\n  - Second item\n";

    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);

    assert_eq!(
        result.ops,
        [ReconcileOp::SetBlockAttrs {
            block_id: "b-second".to_string(),
            attrs: attrs([("indent", json!(2)), ("listStyleType", json!("disc"))]),
        }]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(ids_of(&merged), ["b-first", "b-second"]);
    assert_eq!(text_of(&merged, 1), "Second item");
    assert_eq!(merged[1].rows[0].attrs["indent"], json!(2));
}

/// A block whose content AND attrs both changed emits both ops on the same
/// mapped ID, pinned in this order: `replace_block_content`, then
/// `set_block_attrs`.
#[test]
fn content_and_attrs_both_changed_emit_replace_then_set_block_attrs_on_the_same_id() {
    let base = "- First item\n- Second item\n";
    let canonical = canonical_doc(base, &["b-first", "b-second"]);
    let incoming = "- First item\n  - Second item, edited\n";

    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);

    assert_eq!(
        result.ops,
        [
            replace("b-second", "Second item, edited"),
            ReconcileOp::SetBlockAttrs {
                block_id: "b-second".to_string(),
                attrs: attrs([("indent", json!(2)), ("listStyleType", json!("disc"))]),
            },
        ]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(ids_of(&merged), ["b-first", "b-second"]);
    assert_eq!(text_of(&merged, 1), "Second item, edited");
}

#[test]
fn block_inserted_in_the_middle_gets_a_fresh_id_and_neighbors_keep_ids() {
    let canonical = base_canonical();
    let incoming = "# Title\n\nAlpha paragraph.\n\nInserted between.\n\nBravo paragraph.\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [ReconcileOp::InsertBlock {
            position: 2,
            rows: fresh_paragraph("fresh-1", "Inserted between."),
        }]
    );
    assert_eq!(result.conflicts, []);
    assert_eq!(
        ids_of(&apply_ops(&canonical, &result.ops)),
        ["b-title", "b-alpha", "fresh-1", "b-bravo", "b-charlie"]
    );
}

#[test]
fn block_appended_at_the_end_gets_a_fresh_id() {
    let canonical = base_canonical();
    let incoming = "# Title\n\nAlpha paragraph.\n\nBravo paragraph.\n\nCharlie paragraph.\n\nDelta paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [ReconcileOp::InsertBlock {
            position: 4,
            rows: fresh_paragraph("fresh-1", "Delta paragraph."),
        }]
    );
    assert_eq!(result.conflicts, []);
    assert_eq!(
        ids_of(&apply_ops(&canonical, &result.ops)),
        ["b-title", "b-alpha", "b-bravo", "b-charlie", "fresh-1"]
    );
}

#[test]
fn deleted_block_emits_delete_block_and_other_ids_survive() {
    let canonical = base_canonical();
    let incoming = "# Title\n\nAlpha paragraph.\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [ReconcileOp::DeleteBlock {
            block_id: "b-bravo".to_string(),
        }]
    );
    assert_eq!(result.conflicts, []);
    assert_eq!(
        ids_of(&apply_ops(&canonical, &result.ops)),
        ["b-title", "b-alpha", "b-charlie"]
    );
}

// ---------------------------------------------------------------------------
// Reorders: exact-equality move pairing, never similarity.
// ---------------------------------------------------------------------------

#[test]
fn block_moved_later_emits_one_move_with_preserved_id_and_no_replace() {
    let canonical = base_canonical();
    let incoming = "# Title\n\nAlpha paragraph.\n\nCharlie paragraph.\n\nBravo paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [ReconcileOp::MoveBlock {
            block_id: "b-bravo".to_string(),
            position: 3,
        }]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        ids_of(&merged),
        ["b-title", "b-alpha", "b-charlie", "b-bravo"]
    );
    assert_eq!(text_of(&merged, 3), "Bravo paragraph.");
    // The content is untouched, so anchors on the moved block survive.
    assert_eq!(
        anchor_fate(ReviewKind::Comment, "b-bravo", &result.ops),
        AnchorFate::Untouched
    );
    assert_eq!(
        anchor_fate(ReviewKind::Suggestion, "b-bravo", &result.ops),
        AnchorFate::Untouched
    );
}

/// Moving Bravo before Alpha is textually identical to moving Alpha after
/// Bravo. The deterministic LCS tie-break (deletions advance first) attributes
/// the move to Alpha; the merged order is exact and every ID is preserved, so
/// the two interpretations are semantically equivalent. Recorded as a Gate C
/// attribution rule, not a defect.
#[test]
fn block_moved_earlier_emits_a_deterministic_equivalent_move() {
    let canonical = base_canonical();
    let incoming = "# Title\n\nBravo paragraph.\n\nAlpha paragraph.\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [ReconcileOp::MoveBlock {
            block_id: "b-alpha".to_string(),
            position: 2,
        }]
    );
    assert_eq!(result.conflicts, []);
    assert_eq!(
        ids_of(&apply_ops(&canonical, &result.ops)),
        ["b-title", "b-bravo", "b-alpha", "b-charlie"]
    );
}

/// Characterized limitation: a block that is moved AND edited in the same
/// write is not move-paired (move pairing is exact-equality only). It degrades
/// to delete + insert with a fresh ID, so its anchors orphan/invalidate like
/// any deleted block's.
#[test]
fn moved_and_edited_block_is_not_move_paired_and_degrades_to_delete_plus_insert() {
    let canonical = base_canonical();
    let incoming =
        "# Title\n\nAlpha paragraph.\n\nCharlie paragraph.\n\nBravo paragraph, edited and moved.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [
            ReconcileOp::DeleteBlock {
                block_id: "b-bravo".to_string(),
            },
            ReconcileOp::InsertBlock {
                position: 3,
                rows: fresh_paragraph("fresh-1", "Bravo paragraph, edited and moved."),
            },
        ]
    );
    assert_eq!(result.conflicts, []);
    assert_eq!(
        ids_of(&apply_ops(&canonical, &result.ops)),
        ["b-title", "b-alpha", "b-charlie", "fresh-1"]
    );
    assert_eq!(
        anchor_fate(ReviewKind::Comment, "b-bravo", &result.ops),
        AnchorFate::Orphaned
    );
}

// ---------------------------------------------------------------------------
// Duplicate blocks: deterministic rules, no guessing.
// ---------------------------------------------------------------------------

/// Two identical paragraphs, one moved: the twin that keeps its stable
/// neighborhood stays matched in place; the LCS-unmatched twin is the mover.
/// Both IDs survive. Which twin is "the mover" is decided purely by positional
/// context (LCS), never by content scoring — and since the blocks are
/// byte-identical, either attribution yields the same merged document.
#[test]
fn duplicate_twin_paragraph_move_pairs_deterministically_via_lcs_context() {
    let base = "Twin paragraph.\n\nAlpha paragraph.\n\nTwin paragraph.\n\nBravo paragraph.\n";
    let canonical = canonical_doc(base, &["b-twin-1", "b-alpha", "b-twin-2", "b-bravo"]);
    let incoming = "Alpha paragraph.\n\nTwin paragraph.\n\nBravo paragraph.\n\nTwin paragraph.\n";

    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);

    assert_eq!(
        result.ops,
        [ReconcileOp::MoveBlock {
            block_id: "b-twin-1".to_string(),
            position: 3,
        }]
    );
    assert_eq!(result.conflicts, []);
    assert_eq!(
        ids_of(&apply_ops(&canonical, &result.ops)),
        ["b-alpha", "b-twin-2", "b-bravo", "b-twin-1"]
    );
}

/// One Twin deleted, two Twins inserted elsewhere: the content is no longer
/// unique among the unmatched candidates, so move pairing refuses to guess.
/// The documented fallback applies — delete the old ID, fresh IDs for both
/// inserts. No similarity scoring, no arbitrary pick.
#[test]
fn ambiguous_duplicate_inserts_refuse_move_pairing_and_take_fresh_ids() {
    let base = "Twin paragraph.\n\nAlpha paragraph.\n\nBravo paragraph.\n";
    let canonical = canonical_doc(base, &["b-twin-1", "b-alpha", "b-bravo"]);
    let incoming = "Alpha paragraph.\n\nTwin paragraph.\n\nBravo paragraph.\n\nTwin paragraph.\n";

    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);

    assert_eq!(
        result.ops,
        [
            ReconcileOp::DeleteBlock {
                block_id: "b-twin-1".to_string(),
            },
            ReconcileOp::InsertBlock {
                position: 1,
                rows: fresh_paragraph("fresh-1", "Twin paragraph."),
            },
            ReconcileOp::InsertBlock {
                position: 3,
                rows: fresh_paragraph("fresh-2", "Twin paragraph."),
            },
        ]
    );
    assert_eq!(result.conflicts, []);
    assert_eq!(
        ids_of(&apply_ops(&canonical, &result.ops)),
        ["b-alpha", "fresh-1", "b-bravo", "fresh-2"]
    );
}

// ---------------------------------------------------------------------------
// True conflicts: conflict-as-data, the write never fails.
// ---------------------------------------------------------------------------

#[test]
fn true_conflict_keeps_canonical_emits_artifact_and_still_applies_sibling_ops() {
    // Canonical edited Bravo since the base; the incoming file edits Bravo
    // differently AND edits the (stably separated) Title block.
    let canonical = canonical_doc(
        "# Title\n\nAlpha paragraph.\n\nBravo paragraph, canonical edit.\n\nCharlie paragraph.\n",
        &BASE_IDS,
    );
    let incoming = "# Title, expanded\n\nAlpha paragraph.\n\nBravo paragraph, incoming edit.\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    // The non-conflicting hunk still applies…
    assert_eq!(result.ops, [replace("b-title", "Title, expanded")]);
    // …and the conflicting hunk becomes a structured artifact carrying the
    // incoming hunk, the canonical block ref, and the base context.
    assert_eq!(
        result.conflicts,
        [ReconcileConflict {
            block_ids: vec!["b-bravo".to_string()],
            after_block_id: Some("b-alpha".to_string()),
            base_markdown: "Bravo paragraph.\n".to_string(),
            incoming_markdown: "Bravo paragraph, incoming edit.\n".to_string(),
            canonical_markdown: "Bravo paragraph, canonical edit.\n".to_string(),
        }]
    );
    // The canonical side is retained for the conflicted block.
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(ids_of(&merged), BASE_IDS);
    assert_eq!(text_of(&merged, 0), "Title, expanded");
    assert_eq!(type_of(&merged, 0), "h1");
    assert_eq!(text_of(&merged, 2), "Bravo paragraph, canonical edit.");
    // Anchors on the conflicted block stay untouched: its canonical content
    // did not change.
    assert_eq!(
        anchor_fate(ReviewKind::Comment, "b-bravo", &result.ops),
        AnchorFate::Untouched
    );
    assert_eq!(
        anchor_fate(ReviewKind::Suggestion, "b-bravo", &result.ops),
        AnchorFate::Untouched
    );
}

#[test]
fn edit_vs_delete_conflict_keeps_canonical_block_and_records_the_delete_intent() {
    // Canonical edited Bravo since the base; the incoming file deletes it.
    let canonical = canonical_doc(
        "# Title\n\nAlpha paragraph.\n\nBravo paragraph, canonical edit.\n\nCharlie paragraph.\n",
        &BASE_IDS,
    );
    let incoming = "# Title\n\nAlpha paragraph.\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(result.ops, []);
    assert_eq!(
        result.conflicts,
        [ReconcileConflict {
            block_ids: vec!["b-bravo".to_string()],
            after_block_id: Some("b-alpha".to_string()),
            base_markdown: "Bravo paragraph.\n".to_string(),
            incoming_markdown: String::new(),
            canonical_markdown: "Bravo paragraph, canonical edit.\n".to_string(),
        }]
    );
    // The edited canonical block survives the incoming delete.
    assert_eq!(ids_of(&apply_ops(&canonical, &result.ops)), BASE_IDS);
}

/// The mirror of edit-vs-delete: canonical deleted Bravo since the base while
/// the incoming file edits it. No canonical block survives in the region, so
/// `block_ids` and `canonical_markdown` are empty — the artifact anchors via
/// `after_block_id`, the stable block immediately preceding the region in the
/// merged document. The canonical delete stands; the incoming edit rides only
/// in the artifact.
#[test]
fn canonical_delete_vs_incoming_edit_conflicts_and_anchors_after_the_preceding_stable_block() {
    let canonical = canonical_doc(
        "# Title\n\nAlpha paragraph.\n\nCharlie paragraph.\n",
        &["b-title", "b-alpha", "b-charlie"],
    );
    let incoming =
        "# Title\n\nAlpha paragraph.\n\nBravo paragraph, incoming edit.\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(result.ops, []);
    assert_eq!(
        result.conflicts,
        [ReconcileConflict {
            block_ids: vec![],
            after_block_id: Some("b-alpha".to_string()),
            base_markdown: "Bravo paragraph.\n".to_string(),
            incoming_markdown: "Bravo paragraph, incoming edit.\n".to_string(),
            canonical_markdown: String::new(),
        }]
    );
    // The canonically deleted block stays deleted.
    assert_eq!(
        ids_of(&apply_ops(&canonical, &result.ops)),
        ["b-title", "b-alpha", "b-charlie"]
    );
}

/// Region-granularity finding: conflicts are bounded by the nearest blocks
/// that are stable on BOTH sides. An incoming edit to Charlie is absorbed into
/// the Bravo conflict when canonical edited the adjacent Bravo, because no
/// stable block separates them. The edit is not lost — it rides in the
/// artifact's incoming hunk — but it does not auto-apply.
#[test]
fn incoming_edit_adjacent_to_a_conflict_is_absorbed_into_the_conflict_region() {
    let canonical = canonical_doc(
        "# Title\n\nAlpha paragraph.\n\nBravo paragraph, canonical edit.\n\nCharlie paragraph.\n",
        &BASE_IDS,
    );
    let incoming = "# Title\n\nAlpha paragraph.\n\nBravo paragraph, incoming edit.\n\nCharlie paragraph, incoming edit.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(result.ops, []);
    assert_eq!(
        result.conflicts,
        [ReconcileConflict {
            block_ids: vec!["b-bravo".to_string(), "b-charlie".to_string()],
            after_block_id: Some("b-alpha".to_string()),
            base_markdown: "Bravo paragraph.\n\nCharlie paragraph.\n".to_string(),
            incoming_markdown:
                "Bravo paragraph, incoming edit.\n\nCharlie paragraph, incoming edit.\n".to_string(),
            canonical_markdown: "Bravo paragraph, canonical edit.\n\nCharlie paragraph.\n"
                .to_string(),
        }]
    );
    assert_eq!(ids_of(&apply_ops(&canonical, &result.ops)), BASE_IDS);
}

/// Characterized limitation: incoming moves Bravo to the end while canonical
/// edited the adjacent Alpha. Bravo's source region conflicts (Alpha and Bravo
/// share it — no stable separator), and conflict regions contribute no delete
/// candidates, so the move destination has nothing to pair with: it becomes a
/// fresh insert while the conflict region conservatively keeps the canonical
/// Bravo. The content is duplicated — deterministically and losslessly —
/// until the conflict review item is resolved.
#[test]
fn move_out_of_a_conflict_region_becomes_a_fresh_insert_and_duplicates_content() {
    let canonical = canonical_doc(
        "# Title\n\nAlpha paragraph, canonical edit.\n\nBravo paragraph.\n\nCharlie paragraph.\n",
        &BASE_IDS,
    );
    let incoming = "# Title\n\nAlpha paragraph.\n\nCharlie paragraph.\n\nBravo paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [ReconcileOp::InsertBlock {
            position: 4,
            rows: fresh_paragraph("fresh-1", "Bravo paragraph."),
        }]
    );
    assert_eq!(
        result.conflicts,
        [ReconcileConflict {
            block_ids: vec!["b-alpha".to_string(), "b-bravo".to_string()],
            after_block_id: Some("b-title".to_string()),
            base_markdown: "Alpha paragraph.\n\nBravo paragraph.\n".to_string(),
            incoming_markdown: "Alpha paragraph.\n".to_string(),
            canonical_markdown: "Alpha paragraph, canonical edit.\n\nBravo paragraph.\n"
                .to_string(),
        }]
    );
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        ids_of(&merged),
        ["b-title", "b-alpha", "b-bravo", "b-charlie", "fresh-1"]
    );
    // The duplication, pinned explicitly: the canonical Bravo copy survives in
    // the conflict region AND the moved copy lands at the end as a fresh block.
    assert_eq!(text_of(&merged, 2), "Bravo paragraph.");
    assert_eq!(text_of(&merged, 4), "Bravo paragraph.");
}

// ---------------------------------------------------------------------------
// Anchor fate.
// ---------------------------------------------------------------------------

#[test]
fn anchors_in_a_replaced_block_orphan_comments_and_invalidate_suggestions() {
    let canonical = base_canonical();
    let incoming =
        "# Title\n\nAlpha paragraph.\n\nBravo paragraph, edited.\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(
        anchor_fate(ReviewKind::Comment, "b-bravo", &result.ops),
        AnchorFate::Orphaned
    );
    assert_eq!(
        anchor_fate(ReviewKind::Suggestion, "b-bravo", &result.ops),
        AnchorFate::Invalidated
    );
    // Anchors outside the changed hunk are untouched.
    assert_eq!(
        anchor_fate(ReviewKind::Comment, "b-alpha", &result.ops),
        AnchorFate::Untouched
    );
    assert_eq!(
        anchor_fate(ReviewKind::Suggestion, "b-charlie", &result.ops),
        AnchorFate::Untouched
    );
}

#[test]
fn anchors_in_a_deleted_block_orphan_comments_and_invalidate_suggestions() {
    let canonical = base_canonical();
    let incoming = "# Title\n\nAlpha paragraph.\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(
        anchor_fate(ReviewKind::Comment, "b-bravo", &result.ops),
        AnchorFate::Orphaned
    );
    assert_eq!(
        anchor_fate(ReviewKind::Suggestion, "b-bravo", &result.ops),
        AnchorFate::Invalidated
    );
    assert_eq!(
        anchor_fate(ReviewKind::Comment, "b-charlie", &result.ops),
        AnchorFate::Untouched
    );
}

// ---------------------------------------------------------------------------
// Degenerate bases.
// ---------------------------------------------------------------------------

#[test]
fn base_equal_to_canonical_imports_two_way_with_preserved_ids_and_no_conflicts() {
    // base == canonical: nothing changed canonically since the export, so the
    // merge degenerates to a clean two-way import that can never conflict.
    let canonical = base_canonical();
    let incoming = "# Title\n\nAlpha paragraph, revised.\n\nBravo paragraph.\n\nCharlie paragraph.\n\nDelta paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [
            replace("b-alpha", "Alpha paragraph, revised."),
            ReconcileOp::InsertBlock {
                position: 4,
                rows: fresh_paragraph("fresh-1", "Delta paragraph."),
            },
        ]
    );
    assert_eq!(result.conflicts, []);
    assert_eq!(
        ids_of(&apply_ops(&canonical, &result.ops)),
        ["b-title", "b-alpha", "b-bravo", "b-charlie", "fresh-1"]
    );
}

/// `CurrentCanonical` is the same two-way degenerate case without an export
/// round-trip: the base IS the canonical shapes, so nothing can conflict.
#[test]
fn current_canonical_base_applies_incoming_diffs_with_preserved_ids() {
    let canonical = base_canonical();
    let incoming =
        "# Title\n\nAlpha paragraph, revised.\n\nBravo paragraph.\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::CurrentCanonical, incoming, &canonical);

    assert_eq!(
        result.ops,
        [replace("b-alpha", "Alpha paragraph, revised.")]
    );
    assert_eq!(result.conflicts, []);
    assert_eq!(ids_of(&apply_ops(&canonical, &result.ops)), BASE_IDS);
}

#[test]
fn missing_base_first_import_inserts_every_block_with_fresh_ids() {
    let incoming = "# Title\n\nAlpha paragraph.\n";

    let result = run(ReconcileBase::CurrentCanonical, incoming, &[]);

    assert_eq!(
        result.ops,
        [
            ReconcileOp::InsertBlock {
                position: 0,
                rows: fresh_block("fresh-1", "h1", "Title"),
            },
            ReconcileOp::InsertBlock {
                position: 1,
                rows: fresh_paragraph("fresh-2", "Alpha paragraph."),
            },
        ]
    );
    assert_eq!(result.conflicts, []);
    assert_eq!(ids_of(&apply_ops(&[], &result.ops)), ["fresh-1", "fresh-2"]);
}

// ---------------------------------------------------------------------------
// Block type changes: `set_block_type` pairing (production improvement over
// the spike, which degraded every type change to delete + insert).
// ---------------------------------------------------------------------------

/// A type-only change (h2-style tweaks) preserves the block id AND its
/// anchors — the improvement `set_block_type` exists for.
#[test]
fn type_only_change_emits_set_block_type_and_keeps_block_id_and_anchors() {
    let canonical = base_canonical();
    let incoming = "# Title\n\nAlpha paragraph.\n\n## Bravo paragraph.\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [ReconcileOp::SetBlockType {
            block_id: "b-bravo".to_string(),
            block_type: "h2".to_string(),
            attrs: None,
        }]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(ids_of(&merged), BASE_IDS);
    assert_eq!(type_of(&merged, 2), "h2");
    assert_eq!(text_of(&merged, 2), "Bravo paragraph.");
    assert_eq!(
        anchor_fate(ReviewKind::Comment, "b-bravo", &result.ops),
        AnchorFate::Untouched
    );
    assert_eq!(
        anchor_fate(ReviewKind::Suggestion, "b-bravo", &result.ops),
        AnchorFate::Untouched
    );
}

/// Type AND content changed: the block keeps its id — `set_block_type` then
/// `replace_block_content`, pinned in that order.
#[test]
fn type_and_content_change_keeps_block_id_with_set_type_then_replace() {
    let canonical = base_canonical();
    let incoming =
        "# Title\n\nAlpha paragraph.\n\n## Bravo heading, edited\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [
            ReconcileOp::SetBlockType {
                block_id: "b-bravo".to_string(),
                block_type: "h2".to_string(),
                attrs: None,
            },
            replace("b-bravo", "Bravo heading, edited"),
        ]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(ids_of(&merged), BASE_IDS);
    assert_eq!(type_of(&merged, 2), "h2");
    assert_eq!(text_of(&merged, 2), "Bravo heading, edited");
}

/// Conversions to or from `raw_markdown` keep the spike's delete + insert
/// (the gateway rejects those conversions): a paragraph replaced by a
/// standalone link reference definition (a raw_markdown construct) loses its
/// identity.
#[test]
fn type_change_to_raw_markdown_degrades_to_delete_plus_insert() {
    let canonical = base_canonical();
    let incoming =
        "# Title\n\nAlpha paragraph.\n\n[ref]: https://example.com\n\nCharlie paragraph.\n";

    let result = run(ReconcileBase::Markdown(BASE), incoming, &canonical);

    let parsed = parse_rows("[ref]: https://example.com\n");
    assert_eq!(parsed[0].block_type, "raw_markdown");
    let mut raw_row = parsed[0].clone();
    raw_row.block_id = "fresh-1".to_string();
    raw_row.position = 0;
    assert_eq!(
        result.ops,
        [
            ReconcileOp::DeleteBlock {
                block_id: "b-bravo".to_string(),
            },
            ReconcileOp::InsertBlock {
                position: 2,
                rows: vec![raw_row],
            },
        ]
    );
    assert_eq!(result.conflicts, []);
    assert_eq!(
        ids_of(&apply_ops(&canonical, &result.ops)),
        ["b-title", "b-alpha", "fresh-1", "b-charlie"]
    );
}

// ---------------------------------------------------------------------------
// Equality semantics and container boundaries.
// ---------------------------------------------------------------------------

/// "Exactly equal" includes inline marks: the same text with different
/// formatting pairs positionally as a replace on the mapped id, carrying the
/// new mark runs.
#[test]
fn mark_only_change_pairs_positionally_and_replaces_with_new_marks() {
    let base = "Plain text here.\n\nOther paragraph.\n";
    let canonical = canonical_doc(base, &["b-one", "b-two"]);
    let incoming = "**Plain** text here.\n\nOther paragraph.\n";

    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);

    assert_eq!(
        result.ops,
        [ReconcileOp::ReplaceBlockContent {
            block_id: "b-one".to_string(),
            text: "Plain text here.".to_string(),
            marks: vec![MarkRun {
                start: 0,
                end: 5,
                marks: attrs([("bold", json!(true))]),
            }],
            links: Vec::new(),
        }]
    );
    assert_eq!(result.conflicts, []);
}

/// Container blocks pair only for type/attrs changes over identical children:
/// an attrs-only change (code fence language) preserves the block id.
#[test]
fn container_attrs_only_change_emits_set_block_attrs_and_keeps_identity() {
    let base = "```rust\nlet x = 1;\n```\n\nAfter.\n";
    let canonical = canonical_doc(base, &["b-code", "b-after"]);
    let incoming = "```python\nlet x = 1;\n```\n\nAfter.\n";

    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);

    assert_eq!(
        result.ops,
        [ReconcileOp::SetBlockAttrs {
            block_id: "b-code".to_string(),
            attrs: attrs([("lang", json!("python"))]),
        }]
    );
    assert_eq!(result.conflicts, []);
}

/// Rule 6a: a changed child of a same-type container pairs positionally on
/// the mapped canonical child id — the container and its child keep their
/// identity (the pre-6a behavior, delete + reinsert of the whole subtree,
/// orphaned review anchors on children that never changed).
#[test]
fn container_child_edit_replaces_on_the_mapped_child_id() {
    let base = "```rust\nlet x = 1;\n```\n\nAfter.\n";
    let canonical = canonical_doc(base, &["b-code", "b-after"]);
    let incoming = "```rust\nlet x = 2;\n```\n\nAfter.\n";

    let child_id = surviving_id_of(&canonical, "let x = 1;");
    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);

    assert_eq!(
        result.ops,
        [ReconcileOp::ReplaceBlockContent {
            block_id: child_id.clone(),
            text: "let x = 2;".to_string(),
            marks: Vec::new(),
            links: Vec::new(),
        }]
    );
    assert_eq!(result.conflicts, []);

    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(ids_of(&merged), ["b-code", "b-after"]);
    assert_eq!(
        children_of(&merged, "b-code"),
        [(child_id, "let x = 2;".to_string())]
    );
}

/// Rule 6a: a line added to a code block becomes one `insert_child` at its
/// final merged child index; the container and both existing lines keep
/// their ids.
#[test]
fn line_added_to_code_block_inserts_a_child_and_keeps_sibling_ids() {
    let base = "```toml\n[section]\nkey = \"one\"\n```\n";
    let canonical = canonical_doc(base, &["b-code"]);
    let incoming = "```toml\n[section]\nadded = true\nkey = \"one\"\n```\n";

    let first = surviving_id_of(&canonical, "[section]");
    let second = surviving_id_of(&canonical, "key = \"one\"");
    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);

    let [
        ReconcileOp::InsertChild {
            parent_block_id,
            position,
            rows,
        },
    ] = &result.ops[..]
    else {
        panic!("expected one insert_child, got {:?}", result.ops);
    };
    assert_eq!(parent_block_id, "b-code");
    assert_eq!(*position, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].block_id, "fresh-1");
    assert_eq!(rows[0].block_type, "code_line");
    assert_eq!(rows[0].text, "added = true");
    assert_eq!(rows[0].parent_block_id, None);

    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        children_of(&merged, "b-code"),
        [
            (first, "[section]".to_string()),
            ("fresh-1".to_string(), "added = true".to_string()),
            (second, "key = \"one\"".to_string()),
        ]
    );
}

/// Rule 6a: a line removed from a code block deletes the mapped child id and
/// nothing else; siblings keep their ids and close the gap.
#[test]
fn line_deleted_from_code_block_deletes_the_child_and_keeps_sibling_ids() {
    let base = "```toml\n[section]\nstale = true\nkey = \"one\"\n```\n";
    let canonical = canonical_doc(base, &["b-code"]);
    let incoming = "```toml\n[section]\nkey = \"one\"\n```\n";

    let first = surviving_id_of(&canonical, "[section]");
    let stale = surviving_id_of(&canonical, "stale = true");
    let last = surviving_id_of(&canonical, "key = \"one\"");
    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);

    assert_eq!(result.ops, [ReconcileOp::DeleteBlock { block_id: stale }]);

    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        children_of(&merged, "b-code"),
        [
            (first, "[section]".to_string()),
            (last, "key = \"one\"".to_string()),
        ]
    );
}

/// Rule 6a composes with container attrs: a fence-language change plus one
/// edited line emit `set_block_attrs` on the container and a replace on the
/// mapped child, in that order.
#[test]
fn code_fence_lang_and_line_change_pair_attrs_and_child_replace() {
    let base = "```rust\nkeep();\nold();\n```\n";
    let canonical = canonical_doc(base, &["b-code"]);
    let incoming = "```python\nkeep();\nnew();\n```\n";

    let keep = surviving_id_of(&canonical, "keep();");
    let old = surviving_id_of(&canonical, "old();");
    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);

    assert_eq!(
        result.ops,
        [
            ReconcileOp::SetBlockAttrs {
                block_id: "b-code".to_string(),
                attrs: attrs([("lang", json!("python"))]),
            },
            ReconcileOp::ReplaceBlockContent {
                block_id: old.clone(),
                text: "new();".to_string(),
                marks: Vec::new(),
                links: Vec::new(),
            },
        ]
    );

    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        children_of(&merged, "b-code"),
        [(keep, "keep();".to_string()), (old, "new();".to_string())]
    );
}

/// Rule 6a recurses: a table row added between existing rows inserts one
/// child subtree (`tr` + cells) with fresh ids while every existing row and
/// cell keeps its id.
#[test]
fn table_row_added_inserts_a_child_subtree_with_fresh_ids() {
    let base = "| a | b |\n|---|---|\n| one | two |\n";
    let canonical = canonical_doc(base, &["b-table"]);
    let incoming = "| a | b |\n|---|---|\n| one | two |\n| three | four |\n";

    let one = surviving_id_of(&canonical, "one");
    let two = surviving_id_of(&canonical, "two");
    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);

    let [
        ReconcileOp::InsertChild {
            parent_block_id,
            position,
            rows,
        },
    ] = &result.ops[..]
    else {
        panic!("expected one insert_child, got {:?}", result.ops);
    };
    assert_eq!(parent_block_id, "b-table");
    assert_eq!(*position, 2);
    // The inserted subtree nests the full cell shape: tr → td → p.
    assert_eq!(rows[0].block_type, "tr");
    assert_eq!(rows[0].parent_block_id, None);
    let types: Vec<&str> = rows.iter().map(|row| row.block_type.as_str()).collect();
    assert_eq!(types, ["tr", "td", "p", "td", "p"]);
    assert_eq!(
        rows[1].parent_block_id.as_deref(),
        Some(rows[0].block_id.as_str())
    );
    assert_eq!(rows[2].text, "three");
    assert_eq!(rows[4].text, "four");

    let merged = apply_ops(&canonical, &result.ops);
    let surviving = surviving_ids(&merged);
    assert!(surviving.contains(&one), "existing cell ids must survive");
    assert!(surviving.contains(&two), "existing cell ids must survive");
}

/// A container whose type changed along with its children still degrades to
/// delete + insert with fresh ids — rule 6a only recurses for same-type
/// containers.
#[test]
fn container_type_change_with_changed_children_still_degrades() {
    let base = "```rust\nlet x = 1;\n```\n\nAfter.\n";
    let canonical = canonical_doc(base, &["b-code", "b-after"]);
    let incoming = "| a |\n|---|\n| one |\n\nAfter.\n";

    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);

    assert_eq!(result.conflicts, []);
    assert_eq!(
        result.ops[0],
        ReconcileOp::DeleteBlock {
            block_id: "b-code".to_string(),
        }
    );
    let ReconcileOp::InsertBlock { position, rows } = &result.ops[1] else {
        panic!("expected an insert, got {:?}", result.ops[1]);
    };
    assert_eq!(*position, 0);
    assert_eq!(rows[0].block_type, "table");
    assert_eq!(rows[0].block_id, "fresh-1");
}

/// Review anchors live on individual rows, and a container's children are
/// rows: a comment can be anchored to one line of a code fence. When only
/// some children change, the untouched ones must keep their ids. Otherwise,
/// every anchor inside the container dies, including anchors on text that
/// never changed.
///
/// This guards against replacing the whole container and minting fresh ids
/// for every descendant. The single-line fixture above cannot detect that
/// regression because its only child is the one that changed.
#[test]
fn partially_changed_container_keeps_unchanged_child_ids() {
    let base = "```toml\n[section.one]\nsha256 = \"abc\"\n```\n\nAfter.\n";
    let canonical = canonical_doc(base, &["b-code", "b-after"]);
    let incoming = "```toml\n[section.two]\nsha256 = \"abc\"\n```\n\nAfter.\n";

    let unchanged_child = surviving_id_of(&canonical, "sha256 = \"abc\"");

    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);
    let merged = apply_ops(&canonical, &result.ops);

    assert_eq!(result.conflicts, []);
    assert!(
        surviving_ids(&merged).contains(&unchanged_child),
        "the untouched code line lost its identity, so any review anchor on \
         it orphans; surviving ids: {:?}",
        surviving_ids(&merged)
    );
}

/// The same identity rule one nesting level deeper. Table cells hang off `tr`
/// rows, so reconciliation must recurse beyond direct children to preserve
/// anchors on untouched cells.
#[test]
fn partially_changed_table_keeps_unchanged_cell_ids() {
    let base = "| a | b |\n|---|---|\n| keep | one |\n";
    let canonical = canonical_doc(base, &["b-table"]);
    let incoming = "| a | b |\n|---|---|\n| keep | two |\n";

    let unchanged_cell = surviving_id_of(&canonical, "keep");

    let result = run(ReconcileBase::Markdown(base), incoming, &canonical);
    let merged = apply_ops(&canonical, &result.ops);

    assert_eq!(result.conflicts, []);
    assert!(
        surviving_ids(&merged).contains(&unchanged_cell),
        "the untouched table cell lost its identity, so any review anchor on \
         it orphans; surviving ids: {:?}",
        surviving_ids(&merged)
    );
}

// ---------------------------------------------------------------------------
// The canonical empty-paragraph shape.
// ---------------------------------------------------------------------------

/// Writing real content over the canonical empty document (one empty
/// paragraph row) deletes the placeholder instead of conflicting with it.
#[test]
fn first_write_over_the_canonical_empty_document_replaces_the_placeholder() {
    let canonical = canonical_doc("", &[]);
    let empty_para = vec![BlockRow {
        block_id: "b-empty".to_string(),
        parent_block_id: None,
        position: 0,
        block_type: "p".to_string(),
        attrs: Attrs::new(),
        text: String::new(),
        marks: Vec::new(),
        links: Vec::new(),
    }];
    assert_eq!(canonical, []);

    let result = run(
        ReconcileBase::CurrentCanonical,
        "# Hello\n\nWorld.\n",
        &empty_para,
    );

    assert_eq!(
        result.ops,
        [
            ReconcileOp::DeleteBlock {
                block_id: "b-empty".to_string(),
            },
            ReconcileOp::InsertBlock {
                position: 0,
                rows: fresh_block("fresh-1", "h1", "Hello"),
            },
            ReconcileOp::InsertBlock {
                position: 1,
                rows: fresh_paragraph("fresh-2", "World."),
            },
        ]
    );
    assert_eq!(result.conflicts, []);
}

/// A no-op write leaves trailing empty paragraphs (session leftovers)
/// untouched: no gratuitous deletes, no version churn.
#[test]
fn unchanged_write_leaves_trailing_empty_paragraphs_alone() {
    let mut canonical = canonical_doc("Alpha paragraph.\n", &["b-alpha"]);
    canonical.push(BlockRow {
        block_id: "b-trailing".to_string(),
        parent_block_id: None,
        position: 1,
        block_type: "p".to_string(),
        attrs: Attrs::new(),
        text: String::new(),
        marks: Vec::new(),
        links: Vec::new(),
    });

    let result = run(
        ReconcileBase::CurrentCanonical,
        "Alpha paragraph.\n",
        &canonical,
    );

    assert_eq!(result.ops, []);
    assert_eq!(result.conflicts, []);
}

// ---------------------------------------------------------------------------
// Content errors and the perf bound.
// ---------------------------------------------------------------------------

/// CriticMarkup on the INCOMING text is a content error (typed `Unsupported`),
/// never a merge conflict — the same typed-error contract as the import path.
#[test]
fn critic_markup_incoming_is_a_typed_content_error_not_a_conflict() {
    let canonical = base_canonical();

    let error = reconcile(
        ReconcileBase::Markdown(BASE),
        "Some {++inserted++} text.\n",
        &canonical,
        || "fresh-1".to_string(),
    )
    .unwrap_err();

    assert_eq!(error.0, "critic markup");
}

/// End-to-end pin of the bounded degraded mode: a document whose changed
/// middle exceeds the LCS cell limit still reconciles deterministically via
/// positional pairing — every untouched block keeps its id, the aligned edits
/// apply, and nothing conflicts. (Move detection is what degraded mode gives
/// up; see the module docs.)
#[test]
fn oversized_documents_degrade_to_positional_pairing_with_preserved_ids() {
    let block_count = 1_100usize; // 1100² middle cells > the 2^20 limit
    let paragraphs: Vec<String> = (0..block_count)
        .map(|index| format!("Paragraph number {index}."))
        .collect();
    let base_markdown = format!("{}\n", paragraphs.join("\n\n"));
    let ids: Vec<String> = (0..block_count).map(|index| format!("b-{index}")).collect();
    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let canonical = canonical_doc(&base_markdown, &id_refs);

    // Edit the first and an interior block so neither prefix nor suffix
    // trimming can shrink the middle under the limit.
    let mut edited = paragraphs.clone();
    edited[0] = "Paragraph number 0, edited.".to_string();
    edited[700] = "Paragraph number 700, edited.".to_string();
    let incoming = format!("{}\n", edited.join("\n\n"));

    let result = run(
        ReconcileBase::Markdown(&base_markdown),
        &incoming,
        &canonical,
    );

    assert_eq!(
        result.ops,
        [
            replace("b-0", "Paragraph number 0, edited."),
            replace("b-700", "Paragraph number 700, edited."),
        ]
    );
    assert_eq!(result.conflicts, []);
    assert!(
        result.degraded,
        "an over-limit reconcile must report its degraded mode"
    );
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(merged.len(), block_count);
    assert_eq!(ids_of(&merged), id_refs);
}

#[test]
fn in_budget_reconciles_do_not_report_degradation() {
    let canonical = base_canonical();

    let result = run(ReconcileBase::Markdown(BASE), BASE, &canonical);

    assert!(!result.degraded);
}
