//! Production session projections (Phase 3): rows ↔ session-doc round trips
//! including review anchors as marks, checkpoint degradation, and the
//! gateway's in-place reconciliation.
//!
//! These cover the PRODUCTION versions of the mechanics proven by the Gate A
//! spike (`tests/phase_zero_gate_a.rs`). Anchors ride as browser review marks
//! rather than server-side sticky indices — see the `session_doc` module docs
//! for the rationale; the trade-offs proven by the spike (interior inserts
//! grow ranges, boundary inserts at the start shift them, deleted ranges
//! collapse) are re-proven here against the mark representation.

use quarry_collab_codec::{
    apply_built, build_nodes, project_session_nodes, reconcile_session_children,
    seed_session_nodes, xmltext_to_slate, Attrs, BlockRow, LinkRange, MarkRun, Node, SessionAnchor,
    SessionAnchorKind, SessionProjection,
};
use serde_json::{json, Value};
use yrs::branch::{Branch, BranchID};
use yrs::types::text::YChange;
use yrs::updates::decoder::Decode;
use yrs::{
    Doc, GetString, Offset, OffsetKind, Options, Out, ReadTxn, StateVector, Text, Transact,
    TransactionMut, Update, WriteTxn, Xml, XmlTextRef,
};

const ROOT: &str = "content";

fn block(block_id: &str, parent: Option<&str>, position: u32, ty: &str, text: &str) -> BlockRow {
    BlockRow {
        block_id: block_id.to_string(),
        parent_block_id: parent.map(str::to_string),
        position,
        block_type: ty.to_string(),
        attrs: Attrs::new(),
        text: text.to_string(),
        marks: Vec::new(),
        links: Vec::new(),
    }
}

fn mark(start: u32, end: u32, key: &str) -> MarkRun {
    MarkRun {
        start,
        end,
        marks: [(key.to_string(), json!(true))].into_iter().collect(),
    }
}

fn comment(id: &str, block_id: &str, start: u32, end: u32) -> SessionAnchor {
    SessionAnchor {
        id: id.to_string(),
        kind: SessionAnchorKind::Comment,
        block_id: block_id.to_string(),
        start,
        end,
        replacement: None,
        by: None,
        at_ms: 0,
    }
}

fn suggestion(id: &str, block_id: &str, start: u32, end: u32, replacement: &str) -> SessionAnchor {
    SessionAnchor {
        id: id.to_string(),
        kind: SessionAnchorKind::Suggestion,
        block_id: block_id.to_string(),
        start,
        end,
        replacement: Some(replacement.to_string()),
        by: Some("ai:codex".to_string()),
        at_ms: 1_780_627_260_480,
    }
}

fn fixture_rows() -> Vec<BlockRow> {
    let heading = block("b-heading", None, 0, "h1", "Session 👍 heading");
    let intro = BlockRow {
        marks: vec![mark(11, 15, "bold")],
        ..block("b-intro", None, 1, "p", "Intro with bold and plain runs.")
    };
    let link = BlockRow {
        links: vec![LinkRange {
            start: 8,
            end: 17,
            url: "https://example.test/docs".to_string(),
        }],
        ..block("b-link", None, 2, "p", "See the docs site for details.")
    };
    let code = BlockRow {
        attrs: [("lang".to_string(), json!("rust"))].into_iter().collect(),
        ..block("b-code", None, 3, "code_block", "")
    };
    let code_line = block("b-code-1", Some("b-code"), 0, "code_line", "fn main() {}");
    let raw = BlockRow {
        attrs: [(
            "markdown".to_string(),
            json!("::: warning\nNot supported.\n:::"),
        )]
        .into_iter()
        .collect(),
        ..block("b-raw", None, 4, "raw_markdown", "")
    };
    let outro = block("b-outro", None, 5, "p", "Outro paragraph for end anchors.");
    vec![heading, intro, link, code, code_line, raw, outro]
}

fn fixture_anchors() -> Vec<SessionAnchor> {
    vec![
        comment("c-start", "b-heading", 0, 7),
        comment("c-emoji", "b-heading", 8, 10),
        comment("c-link", "b-link", 4, 21),
        suggestion("s-sub", "b-intro", 11, 15, "brave"),
        suggestion("s-del", "b-outro", 0, 5, ""),
        suggestion("s-ins", "b-outro", 16, 16, "inserted "),
    ]
}

fn mint_sequence(prefix: &'static str) -> impl FnMut() -> String {
    let mut next = 0u32;
    move || {
        next += 1;
        format!("{prefix}-{next}")
    }
}

fn project(nodes: &[Node]) -> SessionProjection {
    project_session_nodes(nodes, mint_sequence("minted")).expect("projection succeeds")
}

fn sorted_anchors(mut anchors: Vec<SessionAnchor>) -> Vec<SessionAnchor> {
    anchors.sort_by(|left, right| left.id.cmp(&right.id));
    anchors
}

/// Strips seed-side-only fields (`by`/`at_ms` are not recoverable for
/// comments; suggestions carry them in their marks).
fn comparable(mut anchors: Vec<SessionAnchor>) -> Vec<SessionAnchor> {
    for anchor in &mut anchors {
        if anchor.kind == SessionAnchorKind::Comment {
            anchor.by = None;
            anchor.at_ms = 0;
        }
    }
    sorted_anchors(anchors)
}

fn new_session_doc() -> Doc {
    Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    })
}

fn seeded_doc(rows: &[BlockRow], anchors: &[SessionAnchor]) -> Doc {
    let doc = new_session_doc();
    let nodes = seed_session_nodes(rows, anchors).expect("seed builds");
    let built = build_nodes(&nodes).expect("nodes build");
    let mut txn = doc.transact_mut();
    let text = txn.get_or_insert_text(ROOT);
    let root: &XmlTextRef = text.as_ref();
    let root = root.clone();
    apply_built(&mut txn, &root, 0, &built);
    drop(txn);
    doc
}

fn doc_children(doc: &Doc) -> Vec<Node> {
    let txn = doc.transact();
    let root = content_root(&txn);
    let Node::Element { children, .. } = xmltext_to_slate(&txn, &root).expect("doc projects")
    else {
        panic!("root must project to a fragment");
    };
    children
}

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
/// Yjs client, apply the edit there, and merge the update back.
fn apply_browser_edit(server: &Doc, edit: impl FnOnce(&mut TransactionMut, &XmlTextRef)) {
    let browser = new_session_doc();
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

fn block_text<T: ReadTxn>(txn: &T, parent: &XmlTextRef, block_id: &str) -> XmlTextRef {
    find_block_text(txn, parent, block_id)
        .unwrap_or_else(|| panic!("block {block_id} not found in session doc"))
}

fn find_block_text<T: ReadTxn>(txn: &T, parent: &XmlTextRef, block_id: &str) -> Option<XmlTextRef> {
    for diff in parent.diff(txn, YChange::identity) {
        let child = match diff.insert {
            Out::YXmlText(child) => child,
            Out::YText(child) => {
                let child: &XmlTextRef = child.as_ref();
                child.clone()
            }
            _ => continue,
        };
        let id = match child.get_attribute(txn, "id") {
            Some(Out::Any(yrs::Any::String(id))) => id.to_string(),
            _ => String::new(),
        };
        if id == block_id {
            return Some(child);
        }
        if let Some(found) = find_block_text(txn, &child, block_id) {
            return Some(found);
        }
    }
    None
}

fn branch_ids(doc: &Doc) -> Vec<BranchID> {
    let txn = doc.transact();
    let root = content_root(&txn);
    root.diff(&txn, YChange::identity)
        .into_iter()
        .filter_map(|diff| match diff.insert {
            Out::YXmlText(child) => {
                let branch: &Branch = child.as_ref();
                Some(branch.id())
            }
            Out::YText(child) => {
                let child: &XmlTextRef = child.as_ref();
                let branch: &Branch = child.as_ref();
                Some(branch.id())
            }
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Round trips
// ---------------------------------------------------------------------------

#[test]
fn identity_round_trip_preserves_rows_and_anchors_exactly() {
    let rows = fixture_rows();
    let anchors = fixture_anchors();
    let doc = seeded_doc(&rows, &anchors);

    let projection = project(&doc_children(&doc));

    assert_eq!(projection.rows, rows);
    assert_eq!(comparable(projection.anchors), comparable(anchors));
}

#[test]
fn seeded_suggestion_marks_match_the_browser_shape() {
    let rows = vec![block("b1", None, 0, "p", "Replace bold here.")];
    let anchors = vec![suggestion("s1", "b1", 8, 12, "brave")];

    let nodes = seed_session_nodes(&rows, &anchors).expect("seed builds");

    assert_eq!(
        serde_json::to_value(&nodes).unwrap(),
        json!([
            {
                "type": "p",
                "id": "b1",
                "children": [
                    { "text": "Replace " },
                    {
                        "text": "bold",
                        "suggestion": true,
                        "suggestion_s1": {
                            "id": "s1",
                            "type": "remove",
                            "userId": "ai:codex",
                            "createdAt": 1_780_627_260_480i64
                        }
                    },
                    {
                        "text": "brave",
                        "suggestion": true,
                        "suggestion_s1": {
                            "id": "s1",
                            "type": "insert",
                            "userId": "ai:codex",
                            "createdAt": 1_780_627_260_480i64
                        }
                    },
                    { "text": " here." }
                ]
            }
        ])
    );
}

#[test]
fn seeded_comment_marks_match_the_browser_shape() {
    let rows = vec![block("b1", None, 0, "p", "See here now.")];
    let anchors = vec![comment("c1", "b1", 4, 8)];

    let nodes = seed_session_nodes(&rows, &anchors).expect("seed builds");

    assert_eq!(
        serde_json::to_value(&nodes).unwrap(),
        json!([
            {
                "type": "p",
                "id": "b1",
                "children": [
                    { "text": "See " },
                    { "text": "here", "comment": true, "comment_c1": true },
                    { "text": " now." }
                ]
            }
        ])
    );
}

#[test]
fn insertion_suggestion_round_trips_as_a_collapsed_anchor() {
    let rows = vec![block("b1", None, 0, "p", "Before after.")];
    let anchors = vec![suggestion("s-ins", "b1", 7, 7, "middle ")];
    let doc = seeded_doc(&rows, &anchors);

    let projection = project(&doc_children(&doc));

    assert_eq!(projection.rows, rows);
    assert_eq!(comparable(projection.anchors), comparable(anchors));
}

#[test]
fn empty_rows_seed_one_empty_paragraph() {
    let nodes = seed_session_nodes(&[], &[]).expect("seed builds");
    assert_eq!(
        serde_json::to_value(&nodes).unwrap(),
        json!([{ "type": "p", "id": "seed-empty", "children": [{ "text": "" }] }])
    );
}

// ---------------------------------------------------------------------------
// Concurrent edits through the CRDT
// ---------------------------------------------------------------------------

#[test]
fn comment_anchor_tracks_concurrent_inserts_through_marks() {
    let rows = vec![block("b1", None, 0, "p", "Intro with bold and plain runs.")];
    let anchors = vec![comment("c1", "b1", 11, 15)]; // "bold"
    let doc = seeded_doc(&rows, &anchors);

    apply_browser_edit(&doc, |txn, root| {
        let intro = block_text(txn, root, "b1");
        intro.insert(txn, 0, "XX"); // before the anchor: shifts right
        intro.insert(txn, 15, "!!!"); // inside the anchor ("bo|ld" shifted by 2): grows
    });

    let projection = project(&doc_children(&doc));
    assert_eq!(
        projection.rows[0].text,
        "XXIntro with bo!!!ld and plain runs."
    );
    assert_eq!(
        comparable(projection.anchors),
        comparable(vec![comment("c1", "b1", 13, 20)])
    );
}

#[test]
fn insert_at_the_start_boundary_is_excluded_from_the_anchor() {
    let rows = vec![block("b1", None, 0, "p", "Intro with bold and plain runs.")];
    let anchors = vec![comment("c1", "b1", 11, 15)]; // "bold"
    let doc = seeded_doc(&rows, &anchors);

    apply_browser_edit(&doc, |txn, root| {
        let intro = block_text(txn, root, "b1");
        intro.insert(txn, 11, "NEW "); // exactly at the anchor's start
    });

    let projection = project(&doc_children(&doc));
    assert_eq!(
        projection.rows[0].text,
        "Intro with NEW bold and plain runs."
    );
    // The Gate A start rule holds for marks too: the anchor shifts right and
    // never grows leftward.
    assert_eq!(
        comparable(projection.anchors),
        comparable(vec![comment("c1", "b1", 15, 19)])
    );
}

#[test]
fn insert_at_the_end_boundary_grows_the_anchor_rightward() {
    let rows = vec![block("b1", None, 0, "p", "Intro with bold and plain runs.")];
    let anchors = vec![comment("c1", "b1", 11, 15)]; // "bold"
    let doc = seeded_doc(&rows, &anchors);

    apply_browser_edit(&doc, |txn, root| {
        let intro = block_text(txn, root, "b1");
        intro.insert(txn, 15, "ish"); // exactly at the anchor's end
    });

    let projection = project(&doc_children(&doc));
    assert_eq!(
        projection.rows[0].text,
        "Intro with boldish and plain runs."
    );
    // Documented divergence from the Gate A sticky-index end rule: a plain
    // insert at a mark's END boundary inherits the mark (Yjs format-marker
    // semantics), so the anchor grows rightward — exactly what the editor
    // displays when a user types at the end of a highlighted range. Rows-mode
    // `replace_block_content` keeps the Gate A exclusion; see the
    // session_doc module docs.
    assert_eq!(
        comparable(projection.anchors),
        comparable(vec![comment("c1", "b1", 11, 18)])
    );
}

#[test]
fn insert_at_a_suggestion_end_boundary_grows_the_anchored_range() {
    let rows = vec![block("b1", None, 0, "p", "Replace bold here.")];
    let anchors = vec![suggestion("s1", "b1", 8, 12, "brave")]; // "bold"
    let doc = seeded_doc(&rows, &anchors);

    apply_browser_edit(&doc, |txn, root| {
        let text = block_text(txn, root, "b1");
        text.insert(txn, 12, "er"); // exactly at the remove-range's end
    });

    let projection = project(&doc_children(&doc));
    // The insert inherits the suggestion's remove mark: the anchored (to be
    // replaced) range grows to "bolder"; the replacement is untouched.
    assert_eq!(projection.rows[0].text, "Replace bolder here.");
    assert_eq!(
        comparable(projection.anchors),
        comparable(vec![suggestion("s1", "b1", 8, 14, "brave")])
    );
}

#[test]
fn deleting_the_whole_commented_range_removes_the_anchor_marks() {
    let rows = vec![block("b1", None, 0, "p", "Intro with bold and plain runs.")];
    let anchors = vec![comment("c1", "b1", 11, 15)];
    let doc = seeded_doc(&rows, &anchors);

    apply_browser_edit(&doc, |txn, root| {
        let intro = block_text(txn, root, "b1");
        intro.remove_range(txn, 11, 4);
    });

    let projection = project(&doc_children(&doc));
    assert_eq!(projection.rows[0].text, "Intro with  and plain runs.");
    // The marks died with the deleted text: the checkpoint reports no anchor
    // and the server orphans the row-side item.
    assert_eq!(projection.anchors, Vec::new());
}

#[test]
fn anchors_survive_a_client_side_block_move() {
    let rows = vec![
        block("b1", None, 0, "p", "First paragraph."),
        BlockRow {
            marks: vec![mark(0, 6, "bold")],
            ..block("b2", None, 1, "p", "Second paragraph.")
        },
    ];
    let anchors = vec![comment("c1", "b2", 7, 16)]; // "paragraph"
    let doc = seeded_doc(&rows, &anchors);

    // A client-side drag-drop move is delete + reinsert of the projected
    // node; marks (including review marks) travel with the content.
    let moved = doc_children(&doc)[1].clone();
    {
        let mut txn = doc.transact_mut();
        let root = content_root_mut(&mut txn);
        root.remove_range(&mut txn, 1, 1);
        apply_built(&mut txn, &root, 0, std::slice::from_ref(&moved));
    }

    let projection = project(&doc_children(&doc));
    let ids: Vec<&str> = projection
        .rows
        .iter()
        .map(|row| row.block_id.as_str())
        .collect();
    assert_eq!(ids, ["b2", "b1"]);
    assert_eq!(projection.rows[0].marks, vec![mark(0, 6, "bold")]);
    assert_eq!(
        comparable(projection.anchors),
        comparable(vec![comment("c1", "b2", 7, 16)])
    );
}

// ---------------------------------------------------------------------------
// Checkpoint shape rules
// ---------------------------------------------------------------------------

#[test]
fn trailing_scaffold_paragraph_is_stripped_even_with_a_runtime_id() {
    let nodes = vec![
        Node::element(
            "p",
            [("id".to_string(), json!("b1"))].into_iter().collect(),
            vec![Node::text("Content.", Attrs::new())],
        ),
        Node::element(
            "p",
            [("id".to_string(), json!("plate-runtime-id"))]
                .into_iter()
                .collect(),
            vec![Node::text("", Attrs::new())],
        ),
    ];

    let projection = project(&nodes);

    assert_eq!(projection.rows, vec![block("b1", None, 0, "p", "Content.")]);
}

#[test]
fn browser_created_blocks_get_minted_ids_and_duplicates_are_remediated() {
    let nodes = vec![
        Node::element(
            "p",
            [("id".to_string(), json!("b1"))].into_iter().collect(),
            vec![Node::text("Original.", Attrs::new())],
        ),
        Node::element("p", Attrs::new(), vec![Node::text("No id.", Attrs::new())]),
        Node::element(
            "p",
            [("id".to_string(), json!("b1"))].into_iter().collect(),
            vec![Node::text("Pasted duplicate.", Attrs::new())],
        ),
    ];

    let projection = project(&nodes);

    assert_eq!(
        projection.rows,
        vec![
            block("b1", None, 0, "p", "Original."),
            block("minted-1", None, 1, "p", "No id."),
            block("minted-2", None, 2, "p", "Pasted duplicate."),
        ]
    );
}

#[test]
fn comment_draft_marks_are_transient_and_never_reach_rows() {
    let nodes = vec![Node::element(
        "p",
        [("id".to_string(), json!("b1"))].into_iter().collect(),
        vec![Node::text(
            "Drafting.",
            [("comment_draft".to_string(), json!(true))]
                .into_iter()
                .collect(),
        )],
    )];

    let projection = project(&nodes);

    assert_eq!(
        projection.rows,
        vec![block("b1", None, 0, "p", "Drafting.")]
    );
    assert_eq!(projection.anchors, Vec::new());
}

#[test]
fn block_with_a_wikilink_degrades_to_a_raw_markdown_row() {
    let nodes = vec![Node::element(
        "p",
        [("id".to_string(), json!("b1"))].into_iter().collect(),
        vec![
            Node::text("See ", Attrs::new()),
            Node::element(
                "wikilink",
                [("target".to_string(), json!("Other Note"))]
                    .into_iter()
                    .collect(),
                vec![Node::text("", Attrs::new())],
            ),
            Node::text(" for more.", Attrs::new()),
        ],
    )];

    let projection = project(&nodes);

    assert_eq!(projection.rows.len(), 1);
    let row = &projection.rows[0];
    assert_eq!(row.block_id, "b1");
    assert_eq!(row.block_type, "raw_markdown");
    assert_eq!(
        row.attrs.get("markdown").and_then(Value::as_str),
        Some("See [[Other Note]] for more.")
    );
}

// ---------------------------------------------------------------------------
// Gateway reconciliation
// ---------------------------------------------------------------------------

fn reconcile(doc: &Doc, pre: &[Node], desired: &[Node]) {
    let mut txn = doc.transact_mut();
    let root = content_root_mut(&mut txn);
    reconcile_session_children(&mut txn, &root, pre, desired).expect("reconcile succeeds");
}

fn doc_image(rows: &[BlockRow], anchors: &[SessionAnchor]) -> Vec<Node> {
    seed_session_nodes(rows, anchors).expect("seed builds")
}

#[test]
fn reconcile_updates_only_the_changed_block_in_place() {
    let rows = vec![
        block("b1", None, 0, "p", "Keep me."),
        block("b2", None, 1, "p", "Change me."),
    ];
    let doc = seeded_doc(&rows, &[]);
    let before = branch_ids(&doc);

    let mut desired_rows = rows.clone();
    desired_rows[1].text = "Changed!".to_string();
    reconcile(&doc, &doc_image(&rows, &[]), &doc_image(&desired_rows, &[]));

    assert_eq!(project(&doc_children(&doc)).rows, desired_rows);
    // Both elements survived in place: no element was recreated.
    assert_eq!(branch_ids(&doc), before);
}

#[test]
fn reconcile_inserts_deletes_and_moves_blocks() {
    let rows = vec![
        block("b1", None, 0, "p", "Alpha."),
        block("b2", None, 1, "p", "Beta."),
        block("b3", None, 2, "p", "Gamma."),
    ];
    let doc = seeded_doc(&rows, &[]);

    // Delete b2, move b3 before b1, insert b4 at the end.
    let desired_rows = vec![
        block("b3", None, 0, "p", "Gamma."),
        block("b1", None, 1, "p", "Alpha."),
        block("b4", None, 2, "p", "Delta."),
    ];
    reconcile(&doc, &doc_image(&rows, &[]), &doc_image(&desired_rows, &[]));

    assert_eq!(project(&doc_children(&doc)).rows, desired_rows);
}

#[test]
fn reconcile_keeps_foreign_scaffold_blocks_in_place() {
    let rows = vec![block("b1", None, 0, "p", "Content.")];
    let doc = seeded_doc(&rows, &[]);
    // A Plate runtime scaffold paragraph trails the content.
    {
        let mut txn = doc.transact_mut();
        let root = content_root_mut(&mut txn);
        let scaffold = Node::element(
            "p",
            [("id".to_string(), json!("plate-scaffold"))]
                .into_iter()
                .collect(),
            vec![Node::text("", Attrs::new())],
        );
        apply_built(&mut txn, &root, 1, std::slice::from_ref(&scaffold));
    }

    let mut desired_rows = rows.clone();
    desired_rows[0].text = "Content updated.".to_string();
    reconcile(&doc, &doc_image(&rows, &[]), &doc_image(&desired_rows, &[]));

    let children = doc_children(&doc);
    assert_eq!(children.len(), 2);
    assert_eq!(
        serde_json::to_value(&children[1]).unwrap(),
        json!({ "type": "p", "id": "plate-scaffold", "children": [{ "text": "" }] })
    );
    // Projection strips the scaffold; the content change landed.
    assert_eq!(project(&children).rows, desired_rows);
}

#[test]
fn reconcile_overlays_new_review_marks_onto_an_existing_block() {
    let rows = vec![block("b1", None, 0, "p", "Comment on this text.")];
    let doc = seeded_doc(&rows, &[]);

    let anchors = vec![comment("c1", "b1", 11, 15)];
    reconcile(&doc, &doc_image(&rows, &[]), &doc_image(&rows, &anchors));

    let projection = project(&doc_children(&doc));
    assert_eq!(projection.rows, rows);
    assert_eq!(comparable(projection.anchors), comparable(anchors));
}

#[test]
fn reconcile_recurses_into_container_blocks() {
    let rows = vec![
        block("b-code", None, 0, "code_block", ""),
        block("b-line-1", Some("b-code"), 0, "code_line", "fn main() {"),
        block("b-line-2", Some("b-code"), 1, "code_line", "}"),
    ];
    let doc = seeded_doc(&rows, &[]);
    let before = branch_ids(&doc);

    let mut desired_rows = rows.clone();
    desired_rows[1].text = "fn main() { println!(); }".to_string();
    reconcile(&doc, &doc_image(&rows, &[]), &doc_image(&desired_rows, &[]));

    assert_eq!(project(&doc_children(&doc)).rows, desired_rows);
    // The container element itself was not recreated.
    assert_eq!(branch_ids(&doc), before);
}

#[test]
fn reconcile_preserves_concurrent_keystrokes_in_untouched_blocks() {
    let rows = vec![
        block("b1", None, 0, "p", "Humans type here."),
        block("b2", None, 1, "p", "Agent target block."),
    ];
    let doc = seeded_doc(&rows, &[]);

    // A browser keystroke lands in b1 while the gateway rewrites b2.
    apply_browser_edit(&doc, |txn, root| {
        let typing = block_text(txn, root, "b1");
        typing.insert(txn, 17, " 123");
    });
    let mut desired_rows = rows.clone();
    desired_rows[1].text = "Agent rewrote the target block.".to_string();
    reconcile(&doc, &doc_image(&rows, &[]), &doc_image(&desired_rows, &[]));

    let projection = project(&doc_children(&doc));
    assert_eq!(projection.rows[0].text, "Humans type here. 123");
    assert_eq!(projection.rows[1].text, "Agent rewrote the target block.");
}

// ---------------------------------------------------------------------------
// Review meta map
// ---------------------------------------------------------------------------

#[test]
fn review_meta_map_round_trips_through_the_doc() {
    use quarry_collab_codec::{
        read_review_meta_from_map, write_review_meta_to_map, ReviewMeta, ReviewMetaEntry,
    };
    let doc = new_session_doc();
    let meta = ReviewMeta {
        comments: [(
            "c1".to_string(),
            ReviewMetaEntry {
                by: "user".to_string(),
                at: "2026-06-09T00:00:00.000Z".to_string(),
                body: Some("Check this".to_string()),
                re: None,
                status: None,
                resolved: None,
            },
        )]
        .into_iter()
        .collect(),
        suggestions: [(
            "s1".to_string(),
            ReviewMetaEntry {
                by: "ai:codex".to_string(),
                at: "2026-06-09T00:00:01.000Z".to_string(),
                body: None,
                re: None,
                status: None,
                resolved: None,
            },
        )]
        .into_iter()
        .collect(),
    };

    {
        let mut txn = doc.transact_mut();
        let map = txn.get_or_insert_map("review");
        write_review_meta_to_map(&mut txn, &map, &meta);
    }

    let txn = doc.transact();
    let map = txn.get_map("review").expect("review map exists");
    assert_eq!(read_review_meta_from_map(&txn, &map), meta);
}

// ---------------------------------------------------------------------------
// Offsets stay anchored to sticky positions through link embeds
// ---------------------------------------------------------------------------

#[test]
fn anchors_spanning_link_embeds_round_trip_exactly() {
    let rows = vec![BlockRow {
        links: vec![LinkRange {
            start: 8,
            end: 17,
            url: "https://example.test/docs".to_string(),
        }],
        ..block("b-link", None, 0, "p", "See the docs site for details.")
    }];
    let anchors = vec![
        comment("c-span", "b-link", 4, 21),    // "the docs site for"
        comment("c-inside", "b-link", 13, 17), // "site"
    ];
    let doc = seeded_doc(&rows, &anchors);

    let projection = project(&doc_children(&doc));

    assert_eq!(projection.rows, rows);
    assert_eq!(comparable(projection.anchors), comparable(anchors));
}

// ---------------------------------------------------------------------------
// Guard: project must keep working against the doc the spike's helpers built
// (the doc shape did not drift between the spike and production).
// ---------------------------------------------------------------------------

#[test]
fn projection_reads_text_inserted_by_a_plain_yjs_client() {
    let rows = vec![block("b1", None, 0, "p", "hello")];
    let doc = seeded_doc(&rows, &[]);
    apply_browser_edit(&doc, |txn, root| {
        let text = block_text(txn, root, "b1");
        text.insert(txn, 5, " world");
        assert_eq!(text.get_string(txn), "hello world");
    });

    let projection = project(&doc_children(&doc));
    assert_eq!(projection.rows[0].text, "hello world");
}

// Sanity: Offset import is used by the sticky-index spike; keep the
// production crate honest about not needing it (compile-time witness that
// the anchors-as-marks projection never touches sticky indices).
#[allow(dead_code)]
fn _sticky_indices_unused(_offset: Offset) {}
