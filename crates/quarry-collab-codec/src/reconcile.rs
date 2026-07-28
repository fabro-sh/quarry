//! diff3 Markdown reconciliation — Phase 4 of the session-scoped collab
//! rewrite, promoted from the proven Gate C spike
//! (`tests/phase_zero_gate_c.rs`, kept untouched as the historical record).
//!
//! A whole-file Markdown write merges into the canonical block tree via a
//! three-way merge at top-level block granularity against a stored base — the
//! same trust model as `git merge` — with zero similarity scoring. Block
//! identity flows through positional base mapping; true conflicts never fail
//! the write, they become structured [`ReconcileConflict`] artifacts while
//! every non-conflicting hunk still applies.
//!
//! ## Hunk-to-operation mapping rules (Gate C, binding)
//!
//! Block-ify base and incoming with the production Markdown codec
//! ([`markdown_to_block_rows`]); the canonical side comes straight from the
//! stored rows. Then:
//!
//! 1. Compute exact-equality LCS alignments `base ↔ incoming` and
//!    `base ↔ canonical`. "Exactly equal" for pairing means the whole
//!    block subtree minus identity: `block_type`, `attrs`, `text`, `marks`,
//!    `links`, and children, compared recursively; `block_id`, `position`,
//!    and parent linkage are excluded. Ties are deterministic: equal blocks
//!    always match (front to back), and on equal-score delete/insert ties
//!    the base side advances first (deletions before insertions).
//! 2. Base indices matched in BOTH alignments are *stable anchors*. The spans
//!    between consecutive stable anchors form regions with a base slice, an
//!    incoming slice, and a canonical slice.
//! 3. Region classification by sequence equality, in this order:
//!    - incoming == base      → only canonical changed → keep canonical, no ops
//!    - incoming == canonical → both made the same change → no ops
//!    - canonical == base     → only incoming changed → emit ops (rule 4)
//!    - otherwise             → true conflict: keep the canonical slice and
//!      emit a [`ReconcileConflict`]; the write still succeeds and ops from
//!      other regions still apply. The artifact carries the canonical block
//!      ids, the three slices as Markdown, and `after_block_id` — the stable
//!      block immediately preceding the region in the merged document
//!      (`None` at document start) — so the `kind = conflict` review item has
//!      an attachment point even when no canonical block survives in the
//!      region. Edit-vs-delete lands here naturally (incoming slice empty:
//!      canonical block kept, artifact records the delete intent), as does
//!      its mirror, canonical-delete vs incoming-edit (`block_ids` and
//!      `canonical_markdown` empty: the canonical delete stands, the incoming
//!      edit rides only in the artifact, anchored by `after_block_id`).
//! 4. Inside an incoming-only region, canonical == base positionally, so every
//!    base index maps 1:1 to a canonical `block_id`. Within-region LCS matches
//!    are unchanged blocks (no op). The remaining gaps yield delete candidates
//!    (unmatched base blocks) and insert candidates (unmatched incoming
//!    blocks), processed by rules 5–7.
//! 5. *Aligned-equal consumption, then move pairing.* First, the k-th delete
//!    candidate of a gap that is exactly equal to the k-th insert candidate
//!    of the same gap is consumed as unchanged (id preserved, no op) — a
//!    strict no-guess refinement of the spike rules (positional identity at
//!    its purest), and what keeps the bounded degraded mode from
//!    re-describing identical blocks as moves.
//!    *Move pairing (global, exact)* then runs: an unmatched deleted base block
//!    pairs with an unmatched inserted incoming block as `move_block` iff
//!    their content is exactly equal AND that content occurs exactly once
//!    among all unmatched deletes and exactly once among all unmatched
//!    inserts. Ambiguous duplicates (multiplicity ≥ 2 on either side) are
//!    never paired; they fall through to delete + insert with fresh IDs.
//! 6. *Positional replace pairing per gap:* the k-th remaining deleted base
//!    block pairs with the k-th remaining inserted incoming block on the
//!    mapped ID. For leaf blocks: a changed type (production improvement over
//!    the spike, which lacked `set_block_type`) emits `set_block_type`
//!    (carrying the new attrs when those changed too), then changed inline
//!    content emits `replace_block_content`, then a same-type attrs change
//!    emits `set_block_attrs` — pinned in that order. Conversions to or from
//!    `raw_markdown` keep the spike's delete + insert (the gateway rejects
//!    those conversions).
//!
//!    *Rule 6a — container recursion.* Same-type container blocks (nested
//!    children: code blocks, tables, …) whose children changed reconcile
//!    their child sequences with the same machinery one level down: bounded
//!    exact-equality LCS over the child shapes, gap-wise k-th/k-th pairing
//!    through rule 6 (recursing further for nested containers such as table
//!    rows), child leftovers becoming `delete_block` on the mapped child id
//!    or `insert_child` with fresh IDs at final merged child indices.
//!    Unchanged children keep their ids — review anchors on them survive.
//!    Child-level move pairing is not attempted: a child moved within its
//!    container degrades to delete + fresh insert. Containers still degrade
//!    to a whole-subtree delete + insert when the type changed along with
//!    the children or when flat inline content drifted on a block that has
//!    children (no such shape is produced by the codec). A type/attrs-only
//!    change over identical children pairs as before.
//! 7. Leftover deletes → `delete_block`; leftover inserts → `insert_block`
//!    with fresh IDs minted by the caller. Fresh IDs are minted in two runs:
//!    child-level inserts from rule 6a first (ascending base order), then
//!    top-level insert subtrees in document order.
//! 8. Op order and application semantics (positions are indices into the final
//!    merged top-level sequence; `insert_child` positions are indices into
//!    the final merged child sequence of their parent): replaces/type-sets/
//!    attr-sets (ID-addressed, any depth) first, then deletes (any depth),
//!    then top-level placements, then child inserts. Application: apply
//!    ID-addressed ops, then deletes, then remove every `move_block` target,
//!    then place moves and top-level inserts at their stated positions in
//!    ascending order, then place child inserts at their stated child indices
//!    ascending per parent (parents of child inserts are paired blocks and
//!    are never moved or deleted). (`translate to sequential gateway ops` is
//!    the adapter layer's job; see `quarry-server`.)
//!
//! ## The canonical empty-paragraph shape
//!
//! Markdown cannot express trailing empty paragraphs, but the canonical empty
//! document is one empty paragraph row (Phase 1's "zero rows means no
//! projection" rule) and live sessions can leave trailing empties. Treating
//! those rows as content would turn every first write into a false conflict
//! (`canonical != base` where base text is empty). Rule: trailing empty
//! paragraph rows are excluded from the diff, and when the reconcile emits
//! any ops at all it also deletes them (the merged content replaces the
//! placeholder). A no-op reconcile leaves them untouched.
//!
//! ## Perf bound (Gate C perf warning honored)
//!
//! The spike's O(n·m)-space LCS is kept as the simple, pinned-tie-break DP,
//! but with three production guards:
//!
//! - The common *prefix* run of exactly-equal blocks is trimmed before the DP
//!   (provably identical to the spike's front-to-back walk, which always
//!   pairs equal heads). Typical whole-file writes touch a few blocks, so the
//!   DP usually runs on a tiny remainder.
//! - If the remaining score matrix would exceed [`LCS_CELL_LIMIT`] cells, the
//!   reconcile degrades deterministically instead of allocating: the common
//!   *suffix* run is also trimmed and the middle is left unmatched, which
//!   downstream becomes pure positional pairing (rule 6) — IDs are still
//!   preserved positionally, but move detection and fine-grained region
//!   separation are lost for the oversized middle. Documented and pinned by
//!   `bounded_lcs_*` tests.
//! - All score-matrix fills in one reconcile share [`LCS_WORK_CELL_LIMIT`].
//!   Once that work budget is spent, later alignments (including recursive
//!   container alignments) use the same deterministic degraded mode. The
//!   per-matrix limit remains a separate memory bound.
//!
//! The stable-anchor intersection is a sorted two-pointer merge (both LCS
//! outputs are ascending in base index), not the spike's O(n²) scan.

use crate::Unsupported;
use crate::rows::{BlockRow, LinkRange, MarkRun, block_rows_to_markdown, markdown_to_block_rows};
use crate::slate::Attrs;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// Maximum LCS score-matrix size in cells (after prefix trimming) before the
/// reconcile degrades to bounded positional pairing. 2^20 cells ≈ 8 MiB of
/// matrix; a document needs ~1000 *changed-region* top-level blocks on both
/// sides to hit it.
const LCS_CELL_LIMIT: usize = 1 << 20;
/// Maximum total score-matrix cells evaluated by all LCS alignments in one
/// reconcile. This preserves the two top-level alignments' former combined
/// allowance while bounding recursive container work across the whole write.
const LCS_WORK_CELL_LIMIT: usize = 2 * LCS_CELL_LIMIT;

/// The base text a whole-file write reconciles against.
pub enum ReconcileBase<'a> {
    /// A stored shadow base (Git peer bases, FUSE open-handle bases, REST
    /// `If-Match` version content).
    Markdown(&'a str),
    /// Two-way degenerate case (CLI, missing shadow base, first import): the
    /// base *is* the current canonical state, so nothing can conflict and
    /// every incoming difference applies.
    CurrentCanonical,
}

/// Reconciler output vocabulary, mirroring the gateway ops it feeds.
/// Placement indices follow rule 8: each is a final merged sibling index in
/// the scope named by its variant. The server translates them to the
/// gateway's sequential semantics.
#[derive(Clone, Debug, PartialEq)]
pub enum ReconcileOp {
    ReplaceBlockContent {
        block_id: String,
        text: String,
        marks: Vec<MarkRun>,
        links: Vec<LinkRange>,
    },
    SetBlockType {
        block_id: String,
        block_type: String,
        /// `Some` when the attrs changed along with the type (replaced
        /// wholesale); `None` keeps the block's attrs.
        attrs: Option<Attrs>,
    },
    SetBlockAttrs {
        block_id: String,
        attrs: Attrs,
    },
    DeleteBlock {
        block_id: String,
    },
    MoveBlock {
        block_id: String,
        /// Final merged top-level index (rule 8).
        position: usize,
    },
    InsertBlock {
        /// Final merged top-level index (rule 8).
        position: usize,
        /// The inserted subtree in depth-first order with freshly minted ids:
        /// `rows[0]` is the top-level block (`parent_block_id = None`),
        /// descendants carry their parent links and sibling positions.
        rows: Vec<BlockRow>,
    },
    /// A fresh child inserted into a surviving container (rule 6a).
    InsertChild {
        /// The canonical id of the container the child is inserted into. The
        /// parent is a paired block: never moved, never deleted.
        parent_block_id: String,
        /// Final merged child index within the parent (rule 8).
        position: usize,
        /// The inserted subtree in depth-first order with freshly minted ids:
        /// `rows[0]` is the inserted child (`parent_block_id = None` — the
        /// op-level parent governs), descendants carry their parent links and
        /// sibling positions.
        rows: Vec<BlockRow>,
    },
}

/// Conflict-as-data: the canonical side is retained in the merge; this
/// artifact becomes a `kind = conflict` review item carrying the losing
/// (incoming) hunk and the base context.
#[derive(Clone, Debug, PartialEq)]
pub struct ReconcileConflict {
    /// The canonical blocks retained in the conflicted region (empty when the
    /// canonical side deleted the region).
    pub block_ids: Vec<String>,
    /// Attachment point: the stable canonical block immediately preceding
    /// this region in the merged document (`None` = document start).
    pub after_block_id: Option<String>,
    pub base_markdown: String,
    pub incoming_markdown: String,
    pub canonical_markdown: String,
}

#[derive(Debug, Default)]
pub struct ReconcileOutcome {
    pub ops: Vec<ReconcileOp>,
    pub conflicts: Vec<ReconcileConflict>,
    /// True when any alignment exceeded the per-matrix memory limit or shared
    /// work limit and degraded to bounded positional pairing (move detection
    /// and fine-grained region separation were lost for this write). Callers
    /// own surfacing it — the codec never logs.
    pub degraded: bool,
}

/// Three-way merge of a whole-file Markdown write against the canonical block
/// rows. `mint_block_id` supplies ids for freshly inserted blocks and their
/// descendants in rule-7 order: child insertions first in ascending base
/// order, then top-level insertions in document order.
///
/// Errors only on Markdown the codec refuses outright (CriticMarkup): a
/// content error on the incoming text, never a merge outcome.
pub fn reconcile(
    base: ReconcileBase<'_>,
    incoming_markdown: &str,
    canonical_rows: &[BlockRow],
    mut mint_block_id: impl FnMut() -> String,
) -> Result<ReconcileOutcome, Unsupported> {
    let incoming = parse_top_blocks(incoming_markdown)?;
    let mut canonical = group_top_blocks(canonical_rows)?;
    let trailing_empty = trim_trailing_empty_paragraphs(&mut canonical);

    let base: Vec<TopBlock> = match base {
        ReconcileBase::Markdown(markdown) => parse_top_blocks(markdown)?,
        ReconcileBase::CurrentCanonical => canonical.clone(),
    };

    let mut lcs_budget = LcsWorkBudget::new(LCS_WORK_CELL_LIMIT);
    let (base_to_incoming, incoming_degraded) = lcs_matching(&base, &incoming, &mut lcs_budget);
    let (base_to_canonical, canonical_degraded) = lcs_matching(&base, &canonical, &mut lcs_budget);
    let anchors = stable_anchors(&base_to_incoming, &base_to_canonical);
    let segments = build_segments(base.len(), incoming.len(), canonical.len(), &anchors);

    let blocks = ReconcileBlocks {
        base: &base,
        incoming: &incoming,
        canonical: &canonical,
    };
    let classified = classify_segments(
        segments,
        blocks.base,
        blocks.incoming,
        blocks.canonical,
        &base_to_incoming,
    )?;
    let consumed =
        consume_positionally_equal_pairs(&classified.plans, blocks.base, blocks.incoming);
    let moves = pair_unique_moves(&classified.plans, blocks.base, blocks.incoming, &consumed);

    let ContentOps {
        replace_ops,
        delete_ops,
        child_insert_ops,
        replaced_by_insert,
        degraded: child_pairing_degraded,
    } = build_content_ops(
        &classified,
        blocks,
        &consumed,
        &moves,
        &mut mint_block_id,
        &mut lcs_budget,
    );

    let placement_ops = build_placement_ops(
        &classified,
        blocks,
        &moves,
        &replaced_by_insert,
        &mut mint_block_id,
    );

    let mut ops = replace_ops;
    ops.extend(delete_ops);
    ops.extend(placement_ops);
    ops.extend(child_insert_ops);
    prepend_trailing_empty_deletes(&mut ops, trailing_empty);
    Ok(ReconcileOutcome {
        ops,
        conflicts: classified.conflicts,
        degraded: incoming_degraded
            || canonical_degraded
            || moves.degraded
            || child_pairing_degraded,
    })
}

fn trim_trailing_empty_paragraphs(canonical: &mut Vec<TopBlock>) -> Vec<String> {
    // Trailing empty paragraphs are invisible to Markdown (module docs).
    let mut trailing_empty: Vec<String> = Vec::new();
    while canonical
        .last()
        .is_some_and(|top| is_empty_paragraph_block(&top.shape))
    {
        let top = canonical.pop().expect("checked non-empty");
        trailing_empty.push(top.rows[0].block_id.clone());
    }
    trailing_empty.reverse();
    trailing_empty
}

struct ClassifiedSegments {
    plans: Vec<SegmentPlan>,
    conflicts: Vec<ReconcileConflict>,
    /// base index → canonical index, for incoming-only regions (positional
    /// 1:1 by construction: `canonical == base` there, shape-recursively, so
    /// canonical child subtrees also align with base children by position).
    canonical_index_by_base: HashMap<usize, usize>,
}

#[derive(Clone, Copy)]
struct ReconcileBlocks<'a> {
    base: &'a [TopBlock],
    incoming: &'a [TopBlock],
    canonical: &'a [TopBlock],
}

fn classify_segments(
    segments: Vec<Segment>,
    base: &[TopBlock],
    incoming: &[TopBlock],
    canonical: &[TopBlock],
    base_to_incoming: &[(usize, usize)],
) -> Result<ClassifiedSegments, Unsupported> {
    // Pass 1: classify every segment; collect conflict artifacts and, for
    // incoming-only regions, the positional base → canonical ID mapping.
    let mut plans = Vec::new();
    let mut conflicts = Vec::new();
    let mut canonical_index_by_base: HashMap<usize, usize> = HashMap::new();
    let mut last_stable: Option<usize> = None;
    for segment in segments {
        match segment {
            Segment::Stable { canonical: index } => {
                last_stable = Some(index);
                plans.push(SegmentPlan::Stable);
            }
            Segment::Unstable(region) => {
                let base_slice = &base[region.base.clone()];
                let incoming_slice = &incoming[region.incoming.clone()];
                let canonical_slice = &canonical[region.canonical.clone()];
                if shapes_equal(incoming_slice, base_slice)
                    || shapes_equal(incoming_slice, canonical_slice)
                {
                    plans.push(SegmentPlan::KeepCanonical {
                        canonical: region.canonical,
                    });
                } else if shapes_equal(canonical_slice, base_slice) {
                    for b in region.base.clone() {
                        let c = region.canonical.start + (b - region.base.start);
                        canonical_index_by_base.insert(b, c);
                    }
                    plans.push(SegmentPlan::ApplyIncoming(incoming_region(
                        &region,
                        base_to_incoming,
                    )));
                } else {
                    conflicts.push(ReconcileConflict {
                        block_ids: canonical_slice
                            .iter()
                            .map(|top| top.rows[0].block_id.clone())
                            .collect(),
                        after_block_id: last_stable
                            .map(|index| canonical[index].rows[0].block_id.clone()),
                        base_markdown: slice_markdown(base_slice)?,
                        incoming_markdown: slice_markdown(incoming_slice)?,
                        canonical_markdown: slice_markdown(canonical_slice)?,
                    });
                    plans.push(SegmentPlan::KeepCanonical {
                        canonical: region.canonical,
                    });
                }
            }
        }
    }
    Ok(ClassifiedSegments {
        plans,
        conflicts,
        canonical_index_by_base,
    })
}

struct ConsumedPairs {
    deletes: HashSet<usize>,
    inserts: HashSet<usize>,
}

fn consume_positionally_equal_pairs(
    plans: &[SegmentPlan],
    base: &[TopBlock],
    incoming: &[TopBlock],
) -> ConsumedPairs {
    // Pass 2a: consume positionally aligned exactly-equal pairs within each
    // gap as unchanged (id preserved, no op). A strict no-guess refinement of
    // the spike rules — positional identity at its purest — and what keeps
    // the bounded degraded mode (whose unmatched middle is an artifact of
    // skipping the DP) from re-describing identical blocks as moves.
    let mut deletes: HashSet<usize> = HashSet::new();
    let mut inserts: HashSet<usize> = HashSet::new();
    for plan in plans {
        if let SegmentPlan::ApplyIncoming(region_plan) = plan {
            for gap in &region_plan.gaps {
                for (b, i) in gap.deletes.clone().zip(gap.inserts.clone()) {
                    if base[b].shape == incoming[i].shape {
                        deletes.insert(b);
                        inserts.insert(i);
                    }
                }
            }
        }
    }
    ConsumedPairs { deletes, inserts }
}

struct MovePairs {
    moved_by_insert: HashMap<usize, usize>,
    moved_bases: HashSet<usize>,
    degraded: bool,
}

fn pair_unique_moves(
    plans: &[SegmentPlan],
    base: &[TopBlock],
    incoming: &[TopBlock],
    consumed: &ConsumedPairs,
) -> MovePairs {
    // Pass 2b: global move pairing over the unmatched candidates — exact
    // equality, and only when the content is unique on both sides.
    let mut delete_candidates: Vec<usize> = Vec::new();
    let mut insert_candidates: Vec<usize> = Vec::new();
    for plan in plans {
        if let SegmentPlan::ApplyIncoming(region_plan) = plan {
            for gap in &region_plan.gaps {
                delete_candidates.extend(
                    gap.deletes
                        .clone()
                        .filter(|b| !consumed.deletes.contains(b)),
                );
                insert_candidates.extend(
                    gap.inserts
                        .clone()
                        .filter(|i| !consumed.inserts.contains(i)),
                );
            }
        }
    }
    // Move pairing scans every (delete, insert) combination; over the same
    // cell limit as the LCS it is skipped entirely (degraded mode loses move
    // detection — module docs).
    let degraded = delete_candidates
        .len()
        .saturating_mul(insert_candidates.len())
        > LCS_CELL_LIMIT;
    let move_candidates = if degraded {
        &[][..]
    } else {
        &insert_candidates[..]
    };
    let mut moved_by_insert: HashMap<usize, usize> = HashMap::new();
    for &insert_idx in move_candidates {
        let shape = &incoming[insert_idx].shape;
        let equal_deletes: Vec<usize> = delete_candidates
            .iter()
            .copied()
            .filter(|&b| base[b].shape == *shape)
            .collect();
        let equal_inserts = insert_candidates
            .iter()
            .filter(|&&other| incoming[other].shape == *shape)
            .count();
        if equal_deletes.len() == 1 && equal_inserts == 1 {
            moved_by_insert.insert(insert_idx, equal_deletes[0]);
        }
    }
    let moved_bases: HashSet<usize> = moved_by_insert.values().copied().collect();
    MovePairs {
        moved_by_insert,
        moved_bases,
        degraded,
    }
}

#[derive(Default)]
struct ContentOps {
    replace_ops: Vec<ReconcileOp>,
    delete_ops: Vec<ReconcileOp>,
    child_insert_ops: Vec<ReconcileOp>,
    replaced_by_insert: HashSet<usize>,
    degraded: bool,
}

fn build_content_ops(
    classified: &ClassifiedSegments,
    blocks: ReconcileBlocks<'_>,
    consumed: &ConsumedPairs,
    moves: &MovePairs,
    mint_block_id: &mut impl FnMut() -> String,
    lcs_budget: &mut LcsWorkBudget,
) -> ContentOps {
    // Pass 3: per-gap positional replace pairing on whatever move pairing left
    // behind; leftovers become deletes and fresh inserts. Plans, gaps within a
    // plan, and pairs within a gap are all walked in ascending base order, so
    // the op vectors come out sorted by base index with no explicit sort
    // (child-level ops ride under their pair's base index).
    let mut ops = ContentOps {
        replaced_by_insert: consumed.inserts.clone(),
        ..ContentOps::default()
    };
    for plan in &classified.plans {
        let SegmentPlan::ApplyIncoming(region_plan) = plan else {
            continue;
        };
        for gap in &region_plan.gaps {
            let mut deletes = gap
                .deletes
                .clone()
                .filter(|b| !moves.moved_bases.contains(b) && !consumed.deletes.contains(b));
            let mut inserts = gap.inserts.clone().filter(|i| {
                !moves.moved_by_insert.contains_key(i) && !consumed.inserts.contains(i)
            });
            loop {
                match (deletes.next(), inserts.next()) {
                    (Some(b), Some(i)) => {
                        let canonical_top =
                            &blocks.canonical[classified.canonical_index_by_base[&b]];
                        let Some(changes) =
                            pair_changes(&blocks.base[b].shape, &blocks.incoming[i].shape)
                        else {
                            // Unpairable (raw_markdown conversion, container
                            // type change over changed children): delete +
                            // fresh insert.
                            ops.delete_ops.push(ReconcileOp::DeleteBlock {
                                block_id: canonical_top.rows[0].block_id.clone(),
                            });
                            continue;
                        };
                        let ids = if changes.children_changed {
                            id_tree(canonical_top)
                        } else {
                            IdNode {
                                id: canonical_top.rows[0].block_id.as_str(),
                                children: Vec::new(),
                            }
                        };
                        apply_pair_replace_ops(
                            &blocks.base[b].shape,
                            &blocks.incoming[i].shape,
                            &ids,
                            changes,
                            mint_block_id,
                            lcs_budget,
                            &mut ops,
                        );
                        ops.replaced_by_insert.insert(i);
                    }
                    (Some(b), None) => ops.delete_ops.push(ReconcileOp::DeleteBlock {
                        block_id: blocks.canonical[classified.canonical_index_by_base[&b]].rows[0]
                            .block_id
                            .clone(),
                    }),
                    (None, Some(_)) => {} // stays a fresh insert
                    (None, None) => break,
                }
            }
        }
    }
    ops
}

fn build_placement_ops(
    classified: &ClassifiedSegments,
    blocks: ReconcileBlocks<'_>,
    moves: &MovePairs,
    replaced_by_insert: &HashSet<usize>,
    mint_block_id: &mut impl FnMut() -> String,
) -> Vec<ReconcileOp> {
    // Pass 4: walk the merged sequence to assign final positions to moves and
    // fresh top-level inserts (fresh IDs for these subtrees are minted in
    // document order, after pass 3's child-insert ids — rule 7).
    let mut merged_len = 0usize;
    let mut placement_ops: Vec<ReconcileOp> = Vec::new();
    for plan in &classified.plans {
        match plan {
            SegmentPlan::Stable => merged_len += 1,
            SegmentPlan::KeepCanonical { canonical } => merged_len += canonical.len(),
            SegmentPlan::ApplyIncoming(region_plan) => {
                for i in region_plan.region.incoming.clone() {
                    if region_plan.matched.contains_key(&i) || replaced_by_insert.contains(&i) {
                        merged_len += 1;
                    } else if let Some(b) = moves.moved_by_insert.get(&i) {
                        placement_ops.push(ReconcileOp::MoveBlock {
                            block_id: blocks.canonical[classified.canonical_index_by_base[b]].rows
                                [0]
                            .block_id
                            .clone(),
                            position: merged_len,
                        });
                        merged_len += 1;
                    } else {
                        placement_ops.push(ReconcileOp::InsertBlock {
                            position: merged_len,
                            rows: fresh_subtree_rows(&blocks.incoming[i].shape, mint_block_id),
                        });
                        merged_len += 1;
                    }
                }
            }
        }
    }
    placement_ops
}

fn prepend_trailing_empty_deletes(ops: &mut Vec<ReconcileOp>, trailing_empty: Vec<String>) {
    // The merged content replaces the trailing empty-paragraph placeholders
    // whenever anything changed at all (module docs).
    if !ops.is_empty() {
        let mut with_trailing: Vec<ReconcileOp> = trailing_empty
            .into_iter()
            .map(|block_id| ReconcileOp::DeleteBlock { block_id })
            .collect();
        with_trailing.append(ops);
        *ops = with_trailing;
    }
}

// ---------------------------------------------------------------------------
// Block model: top-level subtrees compared by identity-free shape.
// ---------------------------------------------------------------------------

/// Everything that makes a block "exactly equal" for pairing — the whole
/// subtree minus identity (`block_id`, `position`, parent linkage).
#[derive(Clone, Debug, PartialEq)]
struct Shape {
    block_type: String,
    attrs: Attrs,
    text: String,
    marks: Vec<MarkRun>,
    links: Vec<LinkRange>,
    children: Vec<Shape>,
}

/// One top-level block: its subtree rows in depth-first order (`rows[0]` is
/// the top-level row) plus the comparable shape.
#[derive(Clone, Debug)]
struct TopBlock {
    rows: Vec<BlockRow>,
    shape: Shape,
}

fn parse_top_blocks(markdown: &str) -> Result<Vec<TopBlock>, Unsupported> {
    let mut next = 0usize;
    let rows = markdown_to_block_rows(markdown, || {
        next += 1;
        format!("parsed-{next}")
    })?;
    group_top_blocks(&rows)
}

fn group_top_blocks(rows: &[BlockRow]) -> Result<Vec<TopBlock>, Unsupported> {
    let mut children_of: HashMap<Option<&str>, Vec<&BlockRow>> = HashMap::new();
    for row in rows {
        children_of
            .entry(row.parent_block_id.as_deref())
            .or_default()
            .push(row);
    }
    for siblings in children_of.values_mut() {
        siblings.sort_by_key(|row| row.position);
    }
    let mut tops = Vec::new();
    let mut placed = 0usize;
    for row in children_of.get(&None).cloned().unwrap_or_default() {
        let mut subtree = Vec::new();
        collect_subtree(row, &children_of, &mut subtree);
        placed += subtree.len();
        let shape = shape_of(row, &children_of);
        tops.push(TopBlock {
            rows: subtree.into_iter().cloned().collect(),
            shape,
        });
    }
    if placed != rows.len() {
        return Err(Unsupported::new(
            "block rows contain orphaned parent references",
        ));
    }
    Ok(tops)
}

fn collect_subtree<'a>(
    row: &'a BlockRow,
    children_of: &HashMap<Option<&str>, Vec<&'a BlockRow>>,
    out: &mut Vec<&'a BlockRow>,
) {
    out.push(row);
    for child in children_of
        .get(&Some(row.block_id.as_str()))
        .cloned()
        .unwrap_or_default()
    {
        collect_subtree(child, children_of, out);
    }
}

fn shape_of(row: &BlockRow, children_of: &HashMap<Option<&str>, Vec<&BlockRow>>) -> Shape {
    Shape {
        block_type: row.block_type.clone(),
        attrs: row.attrs.clone(),
        text: row.text.clone(),
        marks: row.marks.clone(),
        links: row.links.clone(),
        children: children_of
            .get(&Some(row.block_id.as_str()))
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|child| shape_of(child, children_of))
            .collect(),
    }
}

fn shapes_equal(a: &[TopBlock], b: &[TopBlock]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.shape == y.shape)
}

fn is_empty_paragraph_block(shape: &Shape) -> bool {
    shape.block_type == "p"
        && shape.attrs.is_empty()
        && shape.text.is_empty()
        && shape.marks.is_empty()
        && shape.links.is_empty()
        && shape.children.is_empty()
}

/// Serializes a region slice back to Markdown for conflict artifacts.
fn slice_markdown(tops: &[TopBlock]) -> Result<String, Unsupported> {
    let rows: Vec<BlockRow> = tops.iter().flat_map(|top| top.rows.clone()).collect();
    if rows.is_empty() {
        return Ok(String::new());
    }
    block_rows_to_markdown(&rows)
}

/// Borrowed identity tree of a canonical subtree. `TopBlock.rows` and
/// `TopBlock.shape` are both depth-first in the same sibling order, so changed
/// containers can build this directly without reconstructing or sorting a
/// parent-to-children index. Only ids emitted in operations are cloned.
struct IdNode<'a> {
    id: &'a str,
    children: Vec<IdNode<'a>>,
}

fn id_tree(top: &TopBlock) -> IdNode<'_> {
    let mut rows = top.rows.iter();
    let root = id_node(&top.shape, &mut rows);
    debug_assert!(rows.next().is_none(), "shape must cover every subtree row");
    root
}

fn id_node<'a>(shape: &Shape, rows: &mut std::slice::Iter<'a, BlockRow>) -> IdNode<'a> {
    let row = rows
        .next()
        .expect("shape and rows come from the same grouped subtree");
    debug_assert_eq!(row.block_type, shape.block_type);
    IdNode {
        id: row.block_id.as_str(),
        children: shape
            .children
            .iter()
            .map(|child| id_node(child, rows))
            .collect(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MetadataChanges {
    None,
    Type,
    Attrs,
    TypeAndAttrs,
}

impl MetadataChanges {
    fn type_changed(self) -> bool {
        matches!(self, Self::Type | Self::TypeAndAttrs)
    }

    fn attrs_changed(self) -> bool {
        matches!(self, Self::Attrs | Self::TypeAndAttrs)
    }
}

#[derive(Clone, Copy)]
struct PairChanges {
    metadata: MetadataChanges,
    inline_changed: bool,
    children_changed: bool,
}

/// Rule 6 pairability and the changes an accepted positional pair needs.
/// Same-type containers with changed children recurse under rule 6a.
fn pair_changes(base: &Shape, incoming: &Shape) -> Option<PairChanges> {
    let type_changed = base.block_type != incoming.block_type;
    if type_changed && (base.block_type == "raw_markdown" || incoming.block_type == "raw_markdown")
    {
        return None;
    }
    let inline_changed =
        base.text != incoming.text || base.marks != incoming.marks || base.links != incoming.links;
    let has_children = !base.children.is_empty() || !incoming.children.is_empty();
    let children_changed = base.children != incoming.children;
    // Containers carry no flat inline content, so inline drift on a block
    // with children marks a shape this pairing does not understand; a type
    // change over changed children has no id-preserving story either. Both
    // degrade to delete + fresh insert as before rule 6a.
    if has_children && (inline_changed || (type_changed && children_changed)) {
        return None;
    }
    Some(PairChanges {
        metadata: match (type_changed, base.attrs != incoming.attrs) {
            (false, false) => MetadataChanges::None,
            (true, false) => MetadataChanges::Type,
            (false, true) => MetadataChanges::Attrs,
            (true, true) => MetadataChanges::TypeAndAttrs,
        },
        inline_changed,
        children_changed,
    })
}

/// Appends rule-6 operations directly to the reconcile-wide buckets. The
/// caller has already established pairability with [`pair_changes`].
fn apply_pair_replace_ops(
    base: &Shape,
    incoming: &Shape,
    canonical: &IdNode<'_>,
    changes: PairChanges,
    mint_block_id: &mut impl FnMut() -> String,
    lcs_budget: &mut LcsWorkBudget,
    out: &mut ContentOps,
) {
    if changes.metadata.type_changed() {
        out.replace_ops.push(ReconcileOp::SetBlockType {
            block_id: canonical.id.to_string(),
            block_type: incoming.block_type.clone(),
            attrs: changes
                .metadata
                .attrs_changed()
                .then(|| incoming.attrs.clone()),
        });
    }
    if changes.inline_changed {
        out.replace_ops.push(ReconcileOp::ReplaceBlockContent {
            block_id: canonical.id.to_string(),
            text: incoming.text.clone(),
            marks: incoming.marks.clone(),
            links: incoming.links.clone(),
        });
    }
    if changes.metadata == MetadataChanges::Attrs {
        out.replace_ops.push(ReconcileOp::SetBlockAttrs {
            block_id: canonical.id.to_string(),
            attrs: incoming.attrs.clone(),
        });
    }
    if changes.children_changed {
        reconcile_children(base, incoming, canonical, mint_block_id, lcs_budget, out);
    }
}

/// Rule 6a: reconcile the child sequences of a paired same-type container.
/// The same shape as the top-level rules 4–7, one level down and without
/// move pairing: LCS-matched children are unchanged (id preserved, no op),
/// per-gap k-th/k-th positional pairs go through rule 6 recursively, child
/// leftovers become deletes on the mapped canonical child id or fresh
/// `insert_child`s at final merged child indices (rule 8).
fn reconcile_children(
    base: &Shape,
    incoming: &Shape,
    canonical: &IdNode<'_>,
    mint_block_id: &mut impl FnMut() -> String,
    lcs_budget: &mut LcsWorkBudget,
    out: &mut ContentOps,
) {
    let (matches, degraded) = lcs_matching_bounded_by(
        &base.children,
        &incoming.children,
        LCS_CELL_LIMIT,
        lcs_budget,
        |a, b| a == b,
    );
    out.degraded |= degraded;

    // Incoming children that occupy a slot in the merged child sequence:
    // LCS matches plus successful positional pairs.
    let mut kept = vec![false; incoming.children.len()];
    for &(_, incoming_index) in &matches {
        kept[incoming_index] = true;
    }
    for gap in gaps_between(0..base.children.len(), 0..incoming.children.len(), &matches) {
        let mut deletes = gap.deletes;
        let mut inserts = gap.inserts;
        loop {
            match (deletes.next(), inserts.next()) {
                (Some(bk), Some(ik)) => {
                    let child_canonical = &canonical.children[bk];
                    if let Some(changes) = pair_changes(&base.children[bk], &incoming.children[ik])
                    {
                        apply_pair_replace_ops(
                            &base.children[bk],
                            &incoming.children[ik],
                            child_canonical,
                            changes,
                            mint_block_id,
                            lcs_budget,
                            out,
                        );
                        kept[ik] = true;
                    } else {
                        out.delete_ops.push(ReconcileOp::DeleteBlock {
                            block_id: child_canonical.id.to_string(),
                        });
                    }
                }
                (Some(bk), None) => out.delete_ops.push(ReconcileOp::DeleteBlock {
                    block_id: canonical.children[bk].id.to_string(),
                }),
                (None, Some(_)) => {} // stays a fresh child insert
                (None, None) => break,
            }
        }
    }

    // Final merged child indices for the fresh inserts (rule 8, one level
    // down): kept incoming children occupy slots in incoming order, deletes
    // contribute nothing.
    let mut merged_index = 0usize;
    for (ik, child) in incoming.children.iter().enumerate() {
        if kept[ik] {
            merged_index += 1;
        } else {
            out.child_insert_ops.push(ReconcileOp::InsertChild {
                parent_block_id: canonical.id.to_string(),
                position: merged_index,
                rows: fresh_subtree_rows(child, mint_block_id),
            });
            merged_index += 1;
        }
    }
}

/// Synthesizes a fresh [`BlockRow`] subtree from an incoming [`Shape`],
/// minting ids in depth-first order. `rows[0]` carries
/// `parent_block_id = None` and `position = 0`; the placement operation
/// supplies the root's parent and position.
fn fresh_subtree_rows(shape: &Shape, mint_block_id: &mut impl FnMut() -> String) -> Vec<BlockRow> {
    fn walk<F: FnMut() -> String>(
        shape: &Shape,
        parent: Option<&str>,
        position: u32,
        mint_block_id: &mut F,
        out: &mut Vec<BlockRow>,
    ) {
        let id = mint_block_id();
        out.push(BlockRow {
            block_id: id.clone(),
            parent_block_id: parent.map(str::to_string),
            position,
            block_type: shape.block_type.clone(),
            attrs: shape.attrs.clone(),
            text: shape.text.clone(),
            marks: shape.marks.clone(),
            links: shape.links.clone(),
        });
        for (index, child) in shape.children.iter().enumerate() {
            walk(child, Some(&id), index as u32, mint_block_id, out);
        }
    }
    let mut rows = Vec::new();
    walk(shape, None, 0, mint_block_id, &mut rows);
    rows
}

// ---------------------------------------------------------------------------
// Alignment: bounded exact-equality LCS with pinned tie-breaks.
// ---------------------------------------------------------------------------

struct LcsWorkBudget {
    remaining_cells: usize,
}

impl LcsWorkBudget {
    fn new(remaining_cells: usize) -> Self {
        Self { remaining_cells }
    }

    fn spend(&mut self, cells: usize) -> bool {
        if cells > self.remaining_cells {
            return false;
        }
        self.remaining_cells -= cells;
        true
    }
}

/// Exact-equality LCS matching between two block sequences, plus whether the
/// matching degraded. Deterministic tie rules: equal blocks always match
/// (front to back); on equal-score ties the left (base) side advances first,
/// i.e. deletions before insertions.
///
/// The common prefix is trimmed before the DP (identical outcome — the
/// spike's walk always pairs equal heads). When the remaining matrix exceeds
/// [`LCS_CELL_LIMIT`] cells or the reconcile-wide work budget, matching
/// degrades to prefix + suffix runs with an unmatched middle (module docs).
fn lcs_matching(
    a: &[TopBlock],
    b: &[TopBlock],
    work_budget: &mut LcsWorkBudget,
) -> (Vec<(usize, usize)>, bool) {
    lcs_matching_bounded_by(a, b, LCS_CELL_LIMIT, work_budget, |x, y| x.shape == y.shape)
}

#[cfg(test)]
fn lcs_matching_bounded(
    a: &[TopBlock],
    b: &[TopBlock],
    cell_limit: usize,
) -> (Vec<(usize, usize)>, bool) {
    let mut work_budget = LcsWorkBudget::new(usize::MAX);
    lcs_matching_bounded_by(a, b, cell_limit, &mut work_budget, |x, y| {
        x.shape == y.shape
    })
}

/// The bounded-LCS core, generic over the element type so the same pinned
/// tie-breaks and degraded mode serve both top-level blocks (compared by
/// shape) and container children (rule 6a, compared as shapes directly).
fn lcs_matching_bounded_by<T>(
    a: &[T],
    b: &[T],
    cell_limit: usize,
    work_budget: &mut LcsWorkBudget,
    eq: impl Fn(&T, &T) -> bool,
) -> (Vec<(usize, usize)>, bool) {
    let common = a.len().min(b.len());
    let mut prefix = 0usize;
    while prefix < common && eq(&a[prefix], &b[prefix]) {
        prefix += 1;
    }
    let mut pairs: Vec<(usize, usize)> = (0..prefix).map(|index| (index, index)).collect();
    let (a_rest, b_rest) = (&a[prefix..], &b[prefix..]);

    if a_rest.is_empty() || b_rest.is_empty() {
        return (pairs, false);
    }

    let columns = b_rest.len().saturating_add(1);
    let matrix_cells = a_rest.len().saturating_add(1).saturating_mul(columns);
    let work_cells = a_rest.len().saturating_mul(b_rest.len());
    if matrix_cells > cell_limit || !work_budget.spend(work_cells) {
        // Degraded mode: greedy suffix run, unmatched middle.
        let common = a_rest.len().min(b_rest.len());
        let mut suffix = 0usize;
        while suffix < common
            && eq(
                &a_rest[a_rest.len() - 1 - suffix],
                &b_rest[b_rest.len() - 1 - suffix],
            )
        {
            suffix += 1;
        }
        pairs.extend(
            (0..suffix)
                .rev()
                .map(|back| (a.len() - 1 - back, b.len() - 1 - back)),
        );
        return (pairs, true);
    }

    let mut score = vec![0usize; matrix_cells];
    for i in (0..a_rest.len()).rev() {
        for j in (0..b_rest.len()).rev() {
            score[i * columns + j] = if eq(&a_rest[i], &b_rest[j]) {
                1 + score[(i + 1) * columns + j + 1]
            } else {
                score[(i + 1) * columns + j].max(score[i * columns + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < a_rest.len() && j < b_rest.len() {
        if eq(&a_rest[i], &b_rest[j]) {
            pairs.push((prefix + i, prefix + j));
            i += 1;
            j += 1;
        } else if score[(i + 1) * columns + j] >= score[i * columns + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    (pairs, false)
}

/// Base indices matched in BOTH alignments, with their partners. Both inputs
/// are ascending in base index, so this is a sorted two-pointer merge.
fn stable_anchors(
    base_to_incoming: &[(usize, usize)],
    base_to_canonical: &[(usize, usize)],
) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let (mut x, mut y) = (0usize, 0usize);
    while x < base_to_incoming.len() && y < base_to_canonical.len() {
        let (bi, i) = base_to_incoming[x];
        let (bc, c) = base_to_canonical[y];
        match bi.cmp(&bc) {
            std::cmp::Ordering::Less => x += 1,
            std::cmp::Ordering::Greater => y += 1,
            std::cmp::Ordering::Equal => {
                out.push((bi, i, c));
                x += 1;
                y += 1;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Regions between stable anchors.
// ---------------------------------------------------------------------------

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
    /// Carries its canonical index so conflict artifacts can reference the
    /// preceding stable block as their attachment point.
    Stable {
        canonical: usize,
    },
    Unstable(UnstableRegion),
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
        out.push(Segment::Stable { canonical: sc });
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
    deletes: Range<usize>,
    inserts: Range<usize>,
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

/// Streams the gaps before, between, and after sorted match pairs. The final
/// range endpoints act as a sentinel match, so top-level and child
/// reconciliation share the same gap semantics without per-index vectors.
fn gaps_between(
    base: Range<usize>,
    incoming: Range<usize>,
    matches: &[(usize, usize)],
) -> impl Iterator<Item = Gap> + '_ {
    let mut next_base = base.start;
    let mut next_incoming = incoming.start;
    matches
        .iter()
        .copied()
        .chain(std::iter::once((base.end, incoming.end)))
        .map(move |(matched_base, matched_incoming)| {
            let gap = Gap {
                deletes: next_base..matched_base,
                inserts: next_incoming..matched_incoming,
            };
            next_base = matched_base + 1;
            next_incoming = matched_incoming + 1;
            gap
        })
}

fn incoming_region(region: &UnstableRegion, base_to_incoming: &[(usize, usize)]) -> IncomingRegion {
    let matches: Vec<(usize, usize)> = base_to_incoming
        .iter()
        .copied()
        .filter(|(b, _)| region.base.contains(b))
        .collect();
    let gaps = gaps_between(region.base.clone(), region.incoming.clone(), &matches).collect();
    IncomingRegion {
        region: region.clone(),
        matched: matches.into_iter().map(|(b, i)| (i, b)).collect(),
        gaps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paragraph_top(id: &str, text: &str) -> TopBlock {
        let row = BlockRow {
            block_id: id.to_string(),
            parent_block_id: None,
            position: 0,
            block_type: "p".to_string(),
            attrs: Attrs::new(),
            text: text.to_string(),
            marks: Vec::new(),
            links: Vec::new(),
        };
        let shape = Shape {
            block_type: row.block_type.clone(),
            attrs: row.attrs.clone(),
            text: row.text.clone(),
            marks: row.marks.clone(),
            links: row.links.clone(),
            children: Vec::new(),
        };
        TopBlock {
            rows: vec![row],
            shape,
        }
    }

    fn tops(texts: &[&str]) -> Vec<TopBlock> {
        texts
            .iter()
            .enumerate()
            .map(|(index, text)| paragraph_top(&format!("b{index}"), text))
            .collect()
    }

    /// The cell bound is enforced AFTER prefix trimming, so a long unchanged
    /// document head never forces degraded mode.
    #[test]
    fn bounded_lcs_trims_the_common_prefix_before_applying_the_cell_limit() {
        let a = tops(&["same-1", "same-2", "same-3", "old"]);
        let b = tops(&["same-1", "same-2", "same-3", "new"]);

        // The 1x1 middle needs a 2x2 score matrix, exactly the limit of 4.
        let (pairs, degraded) = lcs_matching_bounded(&a, &b, 4);
        assert_eq!(pairs, [(0, 0), (1, 1), (2, 2)]);
        assert!(!degraded);
    }

    /// Over the bound, the matching degrades to prefix + suffix runs with an
    /// unmatched middle: downstream this becomes pure positional pairing —
    /// IDs survive positionally, move detection is lost.
    #[test]
    fn bounded_lcs_degrades_to_prefix_and_suffix_runs_over_the_cell_limit() {
        let a = tops(&["head", "alpha", "bravo", "charlie", "tail"]);
        let b = tops(&["head", "charlie", "alpha", "bravo", "tail"]);

        let (full, full_degraded) = lcs_matching_bounded(&a, &b, usize::MAX);
        assert_eq!(full, [(0, 0), (1, 2), (2, 3), (4, 4)]);
        assert!(!full_degraded);

        let (degraded_pairs, degraded) = lcs_matching_bounded(&a, &b, 4);
        assert_eq!(degraded_pairs, [(0, 0), (4, 4)]);
        assert!(degraded);
    }

    #[test]
    fn bounded_lcs_shares_work_budget_across_alignments() {
        let a = tops(&["alpha", "bravo"]);
        let b = tops(&["charlie", "delta"]);
        let mut work_budget = LcsWorkBudget::new(4);

        let (_, first_degraded) =
            lcs_matching_bounded_by(&a, &b, 9, &mut work_budget, |x, y| x.shape == y.shape);
        let (_, second_degraded) =
            lcs_matching_bounded_by(&a, &b, 9, &mut work_budget, |x, y| x.shape == y.shape);

        assert!(!first_degraded);
        assert!(second_degraded);
        assert_eq!(work_budget.remaining_cells, 0);
    }

    #[test]
    fn bounded_lcs_with_an_empty_side_needs_no_matrix_or_work_budget() {
        let a = tops(&["alpha", "bravo"]);
        let empty = Vec::new();
        let mut work_budget = LcsWorkBudget::new(0);

        let (pairs, degraded) =
            lcs_matching_bounded_by(&a, &empty, 0, &mut work_budget, |x, y| x.shape == y.shape);

        assert!(pairs.is_empty());
        assert!(!degraded);
    }

    #[test]
    fn stable_anchor_intersection_merges_sorted_alignments() {
        let base_to_incoming = [(0, 0), (2, 1), (5, 4)];
        let base_to_canonical = [(0, 0), (3, 2), (5, 5)];

        assert_eq!(
            stable_anchors(&base_to_incoming, &base_to_canonical),
            [(0, 0, 0), (5, 4, 5)]
        );
    }
}
