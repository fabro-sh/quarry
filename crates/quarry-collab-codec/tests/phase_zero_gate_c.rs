//! Phase Zero — Gate C: diff3 identity mapping for whole-file Markdown writes.
//!
//! Proves the reconciliation model of the session-scoped collab design
//! (`docs/superpowers/specs/2026-06-09-session-scoped-collab-design.md`): a
//! whole-file Markdown write merges into the canonical block tree via a
//! three-way merge at block granularity against a stored base — the same trust
//! model as `git merge` — with zero similarity scoring. Block identity flows
//! through positional base mapping; true conflicts never fail the write, they
//! become structured artifacts while every non-conflicting hunk still applies.
//!
//! The reconciler here is Phase-Zero spike code, test-local on purpose. The
//! diff is a hand-rolled LCS over exact block (`Node`) equality rather than a
//! diff crate: block counts are tiny, the tie-breaks must be pinned for the
//! identity rules below, and the spike must demonstrably contain no similarity
//! heuristics.
//!
//! ## Hunk-to-operation mapping rules (the Gate C deliverable)
//!
//! Block-ify all three texts (base, incoming, canonical) with the production
//! Markdown codec, then:
//!
//! 1. Compute exact-equality LCS alignments `base ↔ incoming` and
//!    `base ↔ canonical`. Ties are deterministic: equal blocks always match
//!    (front to back), and on equal-score delete/insert ties the base side
//!    advances first (deletions before insertions).
//! 2. Base indices matched in BOTH alignments are *stable anchors*. The spans
//!    between consecutive stable anchors form regions with a base slice, an
//!    incoming slice, and a canonical slice.
//! 3. Region classification by sequence equality, in this order:
//!    - incoming == base      → only canonical changed → keep canonical, no ops
//!    - incoming == canonical → both made the same change → no ops
//!    - canonical == base     → only incoming changed → emit ops (rule 4)
//!    - otherwise             → true conflict: keep the canonical slice and
//!      emit `ConflictArtifact { block_ids, base, incoming, canonical }`; the
//!      write still succeeds and ops from other regions still apply.
//!      Edit-vs-delete lands here naturally: the incoming slice is empty, the
//!      canonical block stays, and the artifact records the delete intent.
//! 4. Inside an incoming-only region, canonical == base positionally, so every
//!    base index maps 1:1 to a canonical `block_id`. Within-region LCS matches
//!    are unchanged blocks (no op). The remaining gaps yield delete candidates
//!    (unmatched base blocks) and insert candidates (unmatched incoming
//!    blocks), processed by rules 5–7.
//! 5. *Move pairing first (global, exact):* an unmatched deleted base block
//!    pairs with an unmatched inserted incoming block as `move_block` iff
//!    their content is exactly equal AND that content occurs exactly once
//!    among all unmatched deletes and exactly once among all unmatched
//!    inserts. Ambiguous duplicates (multiplicity ≥ 2 on either side) are
//!    never paired; they fall through to delete + insert with fresh IDs.
//! 6. *Positional replace pairing per gap:* the k-th remaining deleted base
//!    block pairs with the k-th remaining inserted incoming block. Same block
//!    type → `replace_block_content` (children changed) and/or
//!    `set_block_attrs` (attrs changed, replaced wholesale) on the mapped ID.
//!    A changed block *type* is not representable in the op vocabulary, so it
//!    degrades to `delete_block` + `insert_block` (fresh ID).
//! 7. Leftover deletes → `delete_block`; leftover inserts → `insert_block`
//!    with a fresh ID.
//! 8. Op order and application semantics (positions are indices into the final
//!    merged top-level sequence): replaces/attr-sets (ID-addressed) first,
//!    then deletes, then placements. Application: apply ID-addressed ops, then
//!    deletes, then remove every `move_block` target, then place moves and
//!    inserts at their stated positions in ascending order.
//!
//! Anchor fate at block granularity: anchors on blocks targeted by
//! `replace_block_content` or `delete_block` orphan (comments) or invalidate
//! (suggestions); everything else — untouched blocks, `set_block_attrs`,
//! `move_block`, and conflicted blocks (canonical side retained) — stays
//! untouched. Minimal-edit anchor preservation *within* a replaced block
//! (common prefix/suffix) is a later refinement, deliberately not built here.
//!
//! Characterized limitations (each pinned by a test below):
//! - Conflicts are detected at region granularity: an incoming edit adjacent
//!   to a conflicted block, with no stable block between them, is absorbed
//!   into the conflict artifact instead of applying.
//! - Reorder attribution between equivalent interpretations ("A moved later"
//!   vs "B moved earlier") follows the deterministic LCS tie-break; the merged
//!   order is always exact and all IDs are preserved either way.
//! - A block that is simultaneously moved and edited is not move-paired (move
//!   pairing is exact-equality only); it degrades to delete + insert with a
//!   fresh ID.

use quarry_collab_codec::slate::attrs;
use quarry_collab_codec::{block_markdown_to_slate, Attrs as SlateAttrs, Node};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

// ---------------------------------------------------------------------------
// In-test row model (stand-in for the Phase 1 `blocks` / `review_items` SQL),
// matching the Gate A vocabulary: canonical state is rows with stable IDs.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct CanonicalBlock {
    block_id: String,
    node: Node,
}

/// The reconciler's output vocabulary (plain structs for the spike; fed to the
/// mutation gateway in Phase 2). `position` / `new_position` are indices into
/// the final merged top-level sequence — see the application semantics in the
/// module docs and `apply_ops`.
#[derive(Debug, PartialEq)]
enum Op {
    ReplaceBlockContent {
        block_id: String,
        content: Vec<Node>,
    },
    SetBlockAttrs {
        block_id: String,
        attrs: SlateAttrs,
    },
    InsertBlock {
        block_id: String,
        node: Node,
        position: usize,
    },
    DeleteBlock {
        block_id: String,
    },
    MoveBlock {
        block_id: String,
        new_position: usize,
    },
}

/// Conflict-as-data: the canonical side is retained in the merge; this
/// artifact (a `kind = conflict` review item in Phase 2) carries the incoming
/// hunk, the canonical block refs, and the base context.
#[derive(Debug, PartialEq)]
struct ConflictArtifact {
    block_ids: Vec<String>,
    base: Vec<Node>,
    incoming: Vec<Node>,
    canonical: Vec<Node>,
}

struct MergeResult {
    ops: Vec<Op>,
    conflicts: Vec<ConflictArtifact>,
}

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

struct ReviewAnchor {
    kind: ReviewKind,
    block_id: String,
}

// ---------------------------------------------------------------------------
// The spike reconciler.
// ---------------------------------------------------------------------------

fn parse_blocks(markdown: &str) -> Vec<Node> {
    block_markdown_to_slate(markdown).expect("spike fixtures use supported markdown")
}

/// Exact-equality LCS matching between two block sequences. Deterministic tie
/// rules: equal blocks always match (front to back); on equal-score ties the
/// left (base) side advances first, i.e. deletions before insertions.
fn lcs_matching(a: &[Node], b: &[Node]) -> Vec<(usize, usize)> {
    let mut score = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            score[i][j] = if a[i] == b[j] {
                1 + score[i + 1][j + 1]
            } else {
                score[i + 1][j].max(score[i][j + 1])
            };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if score[i + 1][j] >= score[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// A span between two stable anchors, with the three aligned slices.
#[derive(Debug, Clone)]
struct UnstableRegion {
    base: Range<usize>,
    incoming: Range<usize>,
    canonical: Range<usize>,
}

#[derive(Debug)]
enum Segment {
    /// A base block matched in both alignments: unchanged everywhere, no ops.
    Stable,
    Unstable(UnstableRegion),
}

/// Base indices matched in BOTH alignments, with their partners.
fn stable_anchors(
    base_to_incoming: &[(usize, usize)],
    base_to_canonical: &[(usize, usize)],
) -> Vec<(usize, usize, usize)> {
    base_to_incoming
        .iter()
        .filter_map(|&(b, i)| {
            base_to_canonical
                .iter()
                .find(|&&(b2, _)| b2 == b)
                .map(|&(_, c)| (b, i, c))
        })
        .collect()
}

fn build_segments(
    base_len: usize,
    incoming_len: usize,
    canonical_len: usize,
    anchors: &[(usize, usize, usize)],
) -> Vec<Segment> {
    let mut out = Vec::new();
    let (mut b, mut i, mut c) = (0, 0, 0);
    for &(sb, si, sc) in anchors {
        if (b, i, c) != (sb, si, sc) {
            out.push(Segment::Unstable(UnstableRegion {
                base: b..sb,
                incoming: i..si,
                canonical: c..sc,
            }));
        }
        out.push(Segment::Stable);
        b = sb + 1;
        i = si + 1;
        c = sc + 1;
    }
    if b < base_len || i < incoming_len || c < canonical_len {
        out.push(Segment::Unstable(UnstableRegion {
            base: b..base_len,
            incoming: i..incoming_len,
            canonical: c..canonical_len,
        }));
    }
    out
}

/// A gap between within-region matches: base blocks not in incoming (delete
/// candidates) and incoming blocks not in base (insert candidates).
struct Gap {
    deletes: Vec<usize>,
    inserts: Vec<usize>,
}

/// An incoming-only region: canonical == base there, so ops apply.
struct IncomingRegion {
    region: UnstableRegion,
    /// incoming idx → base idx, for the LCS matches inside the region.
    matched: HashMap<usize, usize>,
    gaps: Vec<Gap>,
}

enum SegmentPlan {
    Stable,
    /// Canonical-only change, converged change, or conflict: keep canonical.
    KeepCanonical {
        canonical: Range<usize>,
    },
    ApplyIncoming(IncomingRegion),
}

fn incoming_region(region: &UnstableRegion, base_to_incoming: &[(usize, usize)]) -> IncomingRegion {
    let matches: Vec<(usize, usize)> = base_to_incoming
        .iter()
        .copied()
        .filter(|(b, _)| region.base.contains(b))
        .collect();
    let mut gaps = Vec::new();
    let (mut b, mut i) = (region.base.start, region.incoming.start);
    for &(mb, mi) in &matches {
        gaps.push(Gap {
            deletes: (b..mb).collect(),
            inserts: (i..mi).collect(),
        });
        b = mb + 1;
        i = mi + 1;
    }
    gaps.push(Gap {
        deletes: (b..region.base.end).collect(),
        inserts: (i..region.incoming.end).collect(),
    });
    IncomingRegion {
        region: region.clone(),
        matched: matches.into_iter().map(|(b, i)| (i, b)).collect(),
        gaps,
    }
}

fn element_parts(node: &Node) -> (&str, &SlateAttrs, &[Node]) {
    let Node::Element {
        ty,
        attrs,
        children,
    } = node
    else {
        panic!("top-level markdown blocks are elements");
    };
    (ty, attrs, children)
}

fn reconcile(
    base_markdown: Option<&str>,
    incoming_markdown: &str,
    canonical: &[CanonicalBlock],
) -> MergeResult {
    let base = base_markdown.map(parse_blocks).unwrap_or_default();
    let incoming = parse_blocks(incoming_markdown);
    let canonical_nodes: Vec<Node> = canonical.iter().map(|block| block.node.clone()).collect();
    let base_to_incoming = lcs_matching(&base, &incoming);
    let base_to_canonical = lcs_matching(&base, &canonical_nodes);
    let anchors = stable_anchors(&base_to_incoming, &base_to_canonical);
    let segments = build_segments(base.len(), incoming.len(), canonical.len(), &anchors);

    // Pass 1: classify every segment; collect conflict artifacts and, for
    // incoming-only regions, the positional base → canonical ID mapping.
    let mut plans = Vec::new();
    let mut conflicts = Vec::new();
    let mut canonical_id_by_base: HashMap<usize, String> = HashMap::new();
    for segment in segments {
        match segment {
            Segment::Stable => plans.push(SegmentPlan::Stable),
            Segment::Unstable(region) => {
                let base_slice = &base[region.base.clone()];
                let incoming_slice = &incoming[region.incoming.clone()];
                let canonical_slice = &canonical_nodes[region.canonical.clone()];
                if incoming_slice == base_slice || incoming_slice == canonical_slice {
                    plans.push(SegmentPlan::KeepCanonical {
                        canonical: region.canonical,
                    });
                } else if canonical_slice == base_slice {
                    for b in region.base.clone() {
                        let c = region.canonical.start + (b - region.base.start);
                        canonical_id_by_base.insert(b, canonical[c].block_id.clone());
                    }
                    plans.push(SegmentPlan::ApplyIncoming(incoming_region(
                        &region,
                        &base_to_incoming,
                    )));
                } else {
                    conflicts.push(ConflictArtifact {
                        block_ids: canonical[region.canonical.clone()]
                            .iter()
                            .map(|block| block.block_id.clone())
                            .collect(),
                        base: base_slice.to_vec(),
                        incoming: incoming_slice.to_vec(),
                        canonical: canonical_slice.to_vec(),
                    });
                    plans.push(SegmentPlan::KeepCanonical {
                        canonical: region.canonical,
                    });
                }
            }
        }
    }

    // Pass 2: global move pairing over the unmatched candidates — exact
    // equality, and only when the content is unique on both sides.
    let mut delete_candidates: Vec<usize> = Vec::new();
    let mut insert_candidates: Vec<usize> = Vec::new();
    for plan in &plans {
        if let SegmentPlan::ApplyIncoming(region_plan) = plan {
            for gap in &region_plan.gaps {
                delete_candidates.extend(&gap.deletes);
                insert_candidates.extend(&gap.inserts);
            }
        }
    }
    let mut moved_by_insert: HashMap<usize, usize> = HashMap::new();
    for &insert_idx in &insert_candidates {
        let node = &incoming[insert_idx];
        let equal_deletes: Vec<usize> = delete_candidates
            .iter()
            .copied()
            .filter(|&b| base[b] == *node)
            .collect();
        let equal_inserts = insert_candidates
            .iter()
            .filter(|&&other| incoming[other] == *node)
            .count();
        if equal_deletes.len() == 1 && equal_inserts == 1 {
            moved_by_insert.insert(insert_idx, equal_deletes[0]);
        }
    }
    let moved_bases: HashSet<usize> = moved_by_insert.values().copied().collect();

    // Pass 3: per-gap positional replace pairing on whatever move pairing left
    // behind; leftovers become deletes and fresh inserts.
    let mut replace_ops: Vec<(usize, Vec<Op>)> = Vec::new();
    let mut delete_bases: Vec<usize> = Vec::new();
    let mut replaced_by_insert: HashMap<usize, String> = HashMap::new();
    for plan in &plans {
        let SegmentPlan::ApplyIncoming(region_plan) = plan else {
            continue;
        };
        for gap in &region_plan.gaps {
            let deletes: Vec<usize> = gap
                .deletes
                .iter()
                .copied()
                .filter(|b| !moved_bases.contains(b))
                .collect();
            let inserts: Vec<usize> = gap
                .inserts
                .iter()
                .copied()
                .filter(|i| !moved_by_insert.contains_key(i))
                .collect();
            for k in 0..deletes.len().max(inserts.len()) {
                match (deletes.get(k), inserts.get(k)) {
                    (Some(&b), Some(&i)) => {
                        let block_id = canonical_id_by_base[&b].clone();
                        let (base_ty, base_attrs, base_children) = element_parts(&base[b]);
                        let (inc_ty, inc_attrs, inc_children) = element_parts(&incoming[i]);
                        if base_ty != inc_ty {
                            // No `set_block_type` in the op vocabulary: a type
                            // change degrades to delete + insert (fresh ID).
                            delete_bases.push(b);
                            continue;
                        }
                        let mut ops = Vec::new();
                        if base_children != inc_children {
                            ops.push(Op::ReplaceBlockContent {
                                block_id: block_id.clone(),
                                content: inc_children.to_vec(),
                            });
                        }
                        if base_attrs != inc_attrs {
                            ops.push(Op::SetBlockAttrs {
                                block_id: block_id.clone(),
                                attrs: inc_attrs.clone(),
                            });
                        }
                        replace_ops.push((b, ops));
                        replaced_by_insert.insert(i, block_id);
                    }
                    (Some(&b), None) => delete_bases.push(b),
                    (None, Some(_)) => {} // stays a fresh insert
                    (None, None) => unreachable!("k < max(len, len)"),
                }
            }
        }
    }

    // Pass 4: walk the merged sequence to assign final positions to moves and
    // fresh inserts (fresh IDs are deterministic, in document order).
    let mut merged_len = 0usize;
    let mut placement_ops: Vec<Op> = Vec::new();
    let mut fresh = 0usize;
    for plan in &plans {
        match plan {
            SegmentPlan::Stable => merged_len += 1,
            SegmentPlan::KeepCanonical { canonical } => merged_len += canonical.len(),
            SegmentPlan::ApplyIncoming(region_plan) => {
                for i in region_plan.region.incoming.clone() {
                    if region_plan.matched.contains_key(&i) || replaced_by_insert.contains_key(&i) {
                        merged_len += 1;
                    } else if let Some(b) = moved_by_insert.get(&i) {
                        placement_ops.push(Op::MoveBlock {
                            block_id: canonical_id_by_base[b].clone(),
                            new_position: merged_len,
                        });
                        merged_len += 1;
                    } else {
                        fresh += 1;
                        placement_ops.push(Op::InsertBlock {
                            block_id: format!("fresh-{fresh}"),
                            node: incoming[i].clone(),
                            position: merged_len,
                        });
                        merged_len += 1;
                    }
                }
            }
        }
    }

    replace_ops.sort_by_key(|(b, _)| *b);
    delete_bases.sort_unstable();
    let mut ops: Vec<Op> = Vec::new();
    for (_, mut pair_ops) in replace_ops {
        ops.append(&mut pair_ops);
    }
    for b in delete_bases {
        ops.push(Op::DeleteBlock {
            block_id: canonical_id_by_base[&b].clone(),
        });
    }
    ops.extend(placement_ops);
    MergeResult { ops, conflicts }
}

// ---------------------------------------------------------------------------
// Anchor fate (block granularity; see module docs).
// ---------------------------------------------------------------------------

fn anchor_fate(anchor: &ReviewAnchor, ops: &[Op]) -> AnchorFate {
    let block_changed = ops.iter().any(|op| match op {
        Op::ReplaceBlockContent { block_id, .. } | Op::DeleteBlock { block_id } => {
            *block_id == anchor.block_id
        }
        Op::SetBlockAttrs { .. } | Op::InsertBlock { .. } | Op::MoveBlock { .. } => false,
    });
    if !block_changed {
        return AnchorFate::Untouched;
    }
    match anchor.kind {
        ReviewKind::Comment => AnchorFate::Orphaned,
        ReviewKind::Suggestion => AnchorFate::Invalidated,
    }
}

fn comment(block_id: &str) -> ReviewAnchor {
    ReviewAnchor {
        kind: ReviewKind::Comment,
        block_id: block_id.to_string(),
    }
}

fn suggestion(block_id: &str) -> ReviewAnchor {
    ReviewAnchor {
        kind: ReviewKind::Suggestion,
        block_id: block_id.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Test-side op applicator: implements the documented application semantics so
// every test cross-checks the reconciler's positions against an independent
// interpretation of the op stream.
// ---------------------------------------------------------------------------

fn block_mut<'a>(blocks: &'a mut [CanonicalBlock], block_id: &str) -> &'a mut CanonicalBlock {
    blocks
        .iter_mut()
        .find(|block| block.block_id == block_id)
        .expect("op targets an existing block")
}

fn apply_ops(canonical: &[CanonicalBlock], ops: &[Op]) -> Vec<CanonicalBlock> {
    let mut blocks = canonical.to_vec();
    for op in ops {
        match op {
            Op::ReplaceBlockContent { block_id, content } => {
                let Node::Element { children, .. } = &mut block_mut(&mut blocks, block_id).node
                else {
                    panic!("blocks are elements");
                };
                *children = content.clone();
            }
            Op::SetBlockAttrs {
                block_id,
                attrs: new_attrs,
            } => {
                let Node::Element { attrs, .. } = &mut block_mut(&mut blocks, block_id).node else {
                    panic!("blocks are elements");
                };
                *attrs = new_attrs.clone();
            }
            Op::DeleteBlock { block_id } => {
                let index = blocks
                    .iter()
                    .position(|block| block.block_id == *block_id)
                    .expect("deleted block exists");
                blocks.remove(index);
            }
            Op::InsertBlock { .. } | Op::MoveBlock { .. } => {}
        }
    }
    let mut placements: Vec<(usize, CanonicalBlock)> = Vec::new();
    for op in ops {
        if let Op::MoveBlock {
            block_id,
            new_position,
        } = op
        {
            let index = blocks
                .iter()
                .position(|block| block.block_id == *block_id)
                .expect("moved block exists");
            placements.push((*new_position, blocks.remove(index)));
        }
    }
    for op in ops {
        if let Op::InsertBlock {
            block_id,
            node,
            position,
        } = op
        {
            placements.push((
                *position,
                CanonicalBlock {
                    block_id: block_id.clone(),
                    node: node.clone(),
                },
            ));
        }
    }
    placements.sort_by_key(|(position, _)| *position);
    for (position, block) in placements {
        blocks.insert(position, block);
    }
    blocks
}

// ---------------------------------------------------------------------------
// Fixtures and expectation helpers.
// ---------------------------------------------------------------------------

const BASE: &str = "# Title\n\nAlpha paragraph.\n\nBravo paragraph.\n\nCharlie paragraph.\n";
const BASE_IDS: [&str; 4] = ["b-title", "b-alpha", "b-bravo", "b-charlie"];

fn canonical_doc(markdown: &str, ids: &[&str]) -> Vec<CanonicalBlock> {
    let nodes = parse_blocks(markdown);
    assert_eq!(nodes.len(), ids.len(), "fixture block/id count mismatch");
    ids.iter()
        .zip(nodes)
        .map(|(id, node)| CanonicalBlock {
            block_id: id.to_string(),
            node,
        })
        .collect()
}

fn base_canonical() -> Vec<CanonicalBlock> {
    canonical_doc(BASE, &BASE_IDS)
}

fn text_content(text: &str) -> Vec<Node> {
    vec![Node::text(text, SlateAttrs::new())]
}

fn block_node(ty: &str, text: &str) -> Node {
    Node::element(ty, SlateAttrs::new(), text_content(text))
}

fn paragraph(text: &str) -> Node {
    block_node("p", text)
}

fn list_item(indent: u64, text: &str) -> Node {
    Node::element(
        "p",
        attrs([("indent", json!(indent)), ("listStyleType", json!("disc"))]),
        text_content(text),
    )
}

fn entry(block_id: &str, node: Node) -> CanonicalBlock {
    CanonicalBlock {
        block_id: block_id.to_string(),
        node,
    }
}

fn ids_of(blocks: &[CanonicalBlock]) -> Vec<&str> {
    blocks.iter().map(|block| block.block_id.as_str()).collect()
}

// ---------------------------------------------------------------------------
// Identity preservation through positional base mapping.
// ---------------------------------------------------------------------------

#[test]
fn unchanged_document_produces_no_ops_and_preserves_all_block_ids() {
    let canonical = base_canonical();

    let result = reconcile(Some(BASE), BASE, &canonical);

    assert_eq!(result.ops, []);
    assert_eq!(result.conflicts, []);
    assert_eq!(apply_ops(&canonical, &result.ops), canonical);
}

#[test]
fn edited_block_emits_one_replace_on_the_base_mapped_id_and_leaves_siblings_untouched() {
    let canonical = base_canonical();
    let incoming =
        "# Title\n\nAlpha paragraph.\n\nBravo paragraph, edited.\n\nCharlie paragraph.\n";

    let result = reconcile(Some(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [Op::ReplaceBlockContent {
            block_id: "b-bravo".to_string(),
            content: text_content("Bravo paragraph, edited."),
        }]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        ids_of(&merged),
        ["b-title", "b-alpha", "b-bravo", "b-charlie"]
    );
    assert_eq!(
        merged[2],
        entry("b-bravo", paragraph("Bravo paragraph, edited."))
    );
    assert_eq!(merged[1], entry("b-alpha", paragraph("Alpha paragraph.")));
    assert_eq!(
        merged[3],
        entry("b-charlie", paragraph("Charlie paragraph."))
    );
}

#[test]
fn attrs_only_change_emits_set_block_attrs_without_touching_content() {
    let base = "- First item\n- Second item\n";
    let canonical = canonical_doc(base, &["b-first", "b-second"]);
    let incoming = "- First item\n  - Second item\n";

    let result = reconcile(Some(base), incoming, &canonical);

    assert_eq!(
        result.ops,
        [Op::SetBlockAttrs {
            block_id: "b-second".to_string(),
            attrs: attrs([("indent", json!(2)), ("listStyleType", json!("disc"))]),
        }]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(ids_of(&merged), ["b-first", "b-second"]);
    assert_eq!(merged[1], entry("b-second", list_item(2, "Second item")));
}

#[test]
fn block_inserted_in_the_middle_gets_a_fresh_id_and_neighbors_keep_ids() {
    let canonical = base_canonical();
    let incoming = "# Title\n\nAlpha paragraph.\n\nInserted between.\n\nBravo paragraph.\n\nCharlie paragraph.\n";

    let result = reconcile(Some(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [Op::InsertBlock {
            block_id: "fresh-1".to_string(),
            node: paragraph("Inserted between."),
            position: 2,
        }]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        ids_of(&merged),
        ["b-title", "b-alpha", "fresh-1", "b-bravo", "b-charlie"]
    );
}

#[test]
fn block_appended_at_the_end_gets_a_fresh_id() {
    let canonical = base_canonical();
    let incoming =
        "# Title\n\nAlpha paragraph.\n\nBravo paragraph.\n\nCharlie paragraph.\n\nDelta paragraph.\n";

    let result = reconcile(Some(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [Op::InsertBlock {
            block_id: "fresh-1".to_string(),
            node: paragraph("Delta paragraph."),
            position: 4,
        }]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        ids_of(&merged),
        ["b-title", "b-alpha", "b-bravo", "b-charlie", "fresh-1"]
    );
}

#[test]
fn deleted_block_emits_delete_block_and_other_ids_survive() {
    let canonical = base_canonical();
    let incoming = "# Title\n\nAlpha paragraph.\n\nCharlie paragraph.\n";

    let result = reconcile(Some(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [Op::DeleteBlock {
            block_id: "b-bravo".to_string(),
        }]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(ids_of(&merged), ["b-title", "b-alpha", "b-charlie"]);
}

// ---------------------------------------------------------------------------
// Reorders: exact-equality move pairing, never similarity.
// ---------------------------------------------------------------------------

#[test]
fn block_moved_later_emits_one_move_with_preserved_id_and_no_replace() {
    let canonical = base_canonical();
    let incoming = "# Title\n\nAlpha paragraph.\n\nCharlie paragraph.\n\nBravo paragraph.\n";

    let result = reconcile(Some(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [Op::MoveBlock {
            block_id: "b-bravo".to_string(),
            new_position: 3,
        }]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        ids_of(&merged),
        ["b-title", "b-alpha", "b-charlie", "b-bravo"]
    );
    assert_eq!(merged[3], entry("b-bravo", paragraph("Bravo paragraph.")));
    // The content CRDT is untouched, so anchors on the moved block survive.
    assert_eq!(
        anchor_fate(&comment("b-bravo"), &result.ops),
        AnchorFate::Untouched
    );
    assert_eq!(
        anchor_fate(&suggestion("b-bravo"), &result.ops),
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

    let result = reconcile(Some(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [Op::MoveBlock {
            block_id: "b-alpha".to_string(),
            new_position: 2,
        }]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        ids_of(&merged),
        ["b-title", "b-bravo", "b-alpha", "b-charlie"]
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

    let result = reconcile(Some(base), incoming, &canonical);

    assert_eq!(
        result.ops,
        [Op::MoveBlock {
            block_id: "b-twin-1".to_string(),
            new_position: 3,
        }]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        ids_of(&merged),
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

    let result = reconcile(Some(base), incoming, &canonical);

    assert_eq!(
        result.ops,
        [
            Op::DeleteBlock {
                block_id: "b-twin-1".to_string(),
            },
            Op::InsertBlock {
                block_id: "fresh-1".to_string(),
                node: paragraph("Twin paragraph."),
                position: 1,
            },
            Op::InsertBlock {
                block_id: "fresh-2".to_string(),
                node: paragraph("Twin paragraph."),
                position: 3,
            },
        ]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        ids_of(&merged),
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
    let incoming =
        "# Title, expanded\n\nAlpha paragraph.\n\nBravo paragraph, incoming edit.\n\nCharlie paragraph.\n";

    let result = reconcile(Some(BASE), incoming, &canonical);

    // The non-conflicting hunk still applies…
    assert_eq!(
        result.ops,
        [Op::ReplaceBlockContent {
            block_id: "b-title".to_string(),
            content: text_content("Title, expanded"),
        }]
    );
    // …and the conflicting hunk becomes a structured artifact carrying the
    // incoming hunk, the canonical block ref, and the base context.
    assert_eq!(
        result.conflicts,
        [ConflictArtifact {
            block_ids: vec!["b-bravo".to_string()],
            base: vec![paragraph("Bravo paragraph.")],
            incoming: vec![paragraph("Bravo paragraph, incoming edit.")],
            canonical: vec![paragraph("Bravo paragraph, canonical edit.")],
        }]
    );
    // The canonical side is retained for the conflicted block.
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        ids_of(&merged),
        ["b-title", "b-alpha", "b-bravo", "b-charlie"]
    );
    assert_eq!(
        merged[0],
        entry("b-title", block_node("h1", "Title, expanded"))
    );
    assert_eq!(
        merged[2],
        entry("b-bravo", paragraph("Bravo paragraph, canonical edit."))
    );
    // Anchors on the conflicted block stay untouched: its canonical content
    // did not change.
    assert_eq!(
        anchor_fate(&comment("b-bravo"), &result.ops),
        AnchorFate::Untouched
    );
    assert_eq!(
        anchor_fate(&suggestion("b-bravo"), &result.ops),
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

    let result = reconcile(Some(BASE), incoming, &canonical);

    assert_eq!(result.ops, []);
    assert_eq!(
        result.conflicts,
        [ConflictArtifact {
            block_ids: vec!["b-bravo".to_string()],
            base: vec![paragraph("Bravo paragraph.")],
            incoming: vec![],
            canonical: vec![paragraph("Bravo paragraph, canonical edit.")],
        }]
    );
    // The edited canonical block survives the incoming delete.
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(merged, canonical);
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
    let incoming =
        "# Title\n\nAlpha paragraph.\n\nBravo paragraph, incoming edit.\n\nCharlie paragraph, incoming edit.\n";

    let result = reconcile(Some(BASE), incoming, &canonical);

    assert_eq!(result.ops, []);
    assert_eq!(
        result.conflicts,
        [ConflictArtifact {
            block_ids: vec!["b-bravo".to_string(), "b-charlie".to_string()],
            base: vec![
                paragraph("Bravo paragraph."),
                paragraph("Charlie paragraph.")
            ],
            incoming: vec![
                paragraph("Bravo paragraph, incoming edit."),
                paragraph("Charlie paragraph, incoming edit.")
            ],
            canonical: vec![
                paragraph("Bravo paragraph, canonical edit."),
                paragraph("Charlie paragraph.")
            ],
        }]
    );
    assert_eq!(apply_ops(&canonical, &result.ops), canonical);
}

// ---------------------------------------------------------------------------
// Anchor fate.
// ---------------------------------------------------------------------------

#[test]
fn anchors_in_a_replaced_block_orphan_comments_and_invalidate_suggestions() {
    let canonical = base_canonical();
    let incoming =
        "# Title\n\nAlpha paragraph.\n\nBravo paragraph, edited.\n\nCharlie paragraph.\n";

    let result = reconcile(Some(BASE), incoming, &canonical);

    assert_eq!(
        anchor_fate(&comment("b-bravo"), &result.ops),
        AnchorFate::Orphaned
    );
    assert_eq!(
        anchor_fate(&suggestion("b-bravo"), &result.ops),
        AnchorFate::Invalidated
    );
    // Anchors outside the changed hunk are untouched.
    assert_eq!(
        anchor_fate(&comment("b-alpha"), &result.ops),
        AnchorFate::Untouched
    );
    assert_eq!(
        anchor_fate(&suggestion("b-charlie"), &result.ops),
        AnchorFate::Untouched
    );
}

#[test]
fn anchors_in_a_deleted_block_orphan_comments_and_invalidate_suggestions() {
    let canonical = base_canonical();
    let incoming = "# Title\n\nAlpha paragraph.\n\nCharlie paragraph.\n";

    let result = reconcile(Some(BASE), incoming, &canonical);

    assert_eq!(
        anchor_fate(&comment("b-bravo"), &result.ops),
        AnchorFate::Orphaned
    );
    assert_eq!(
        anchor_fate(&suggestion("b-bravo"), &result.ops),
        AnchorFate::Invalidated
    );
    assert_eq!(
        anchor_fate(&comment("b-charlie"), &result.ops),
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
    let incoming =
        "# Title\n\nAlpha paragraph, revised.\n\nBravo paragraph.\n\nCharlie paragraph.\n\nDelta paragraph.\n";

    let result = reconcile(Some(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [
            Op::ReplaceBlockContent {
                block_id: "b-alpha".to_string(),
                content: text_content("Alpha paragraph, revised."),
            },
            Op::InsertBlock {
                block_id: "fresh-1".to_string(),
                node: paragraph("Delta paragraph."),
                position: 4,
            },
        ]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        ids_of(&merged),
        ["b-title", "b-alpha", "b-bravo", "b-charlie", "fresh-1"]
    );
}

#[test]
fn missing_base_first_import_inserts_every_block_with_fresh_ids() {
    let incoming = "# Title\n\nAlpha paragraph.\n";

    let result = reconcile(None, incoming, &[]);

    assert_eq!(
        result.ops,
        [
            Op::InsertBlock {
                block_id: "fresh-1".to_string(),
                node: block_node("h1", "Title"),
                position: 0,
            },
            Op::InsertBlock {
                block_id: "fresh-2".to_string(),
                node: paragraph("Alpha paragraph."),
                position: 1,
            },
        ]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&[], &result.ops);
    assert_eq!(ids_of(&merged), ["fresh-1", "fresh-2"]);
}

// ---------------------------------------------------------------------------
// Op-vocabulary boundary: block type changes.
// ---------------------------------------------------------------------------

/// There is no `set_block_type` in the op vocabulary, so a block whose type
/// changes at the same base position (here p → h2 with identical text) loses
/// its identity: delete + insert with a fresh ID. Recorded as a Gate C
/// finding — heading-level tweaks (h2 → h3) would orphan anchors under this
/// rule, which production may want to soften with a type op.
#[test]
fn type_change_at_the_same_position_degrades_to_delete_plus_insert() {
    let canonical = base_canonical();
    let incoming = "# Title\n\nAlpha paragraph.\n\n## Bravo paragraph.\n\nCharlie paragraph.\n";

    let result = reconcile(Some(BASE), incoming, &canonical);

    assert_eq!(
        result.ops,
        [
            Op::DeleteBlock {
                block_id: "b-bravo".to_string(),
            },
            Op::InsertBlock {
                block_id: "fresh-1".to_string(),
                node: block_node("h2", "Bravo paragraph."),
                position: 2,
            },
        ]
    );
    assert_eq!(result.conflicts, []);
    let merged = apply_ops(&canonical, &result.ops);
    assert_eq!(
        ids_of(&merged),
        ["b-title", "b-alpha", "fresh-1", "b-charlie"]
    );
}
