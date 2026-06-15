use quarry_collab_codec::{
    apply_built, apply_review_patch_to_map, block_markdown_to_slate, build_nodes,
    encode_update_v1_from_built_with_review, strip_trailing_empty_paragraphs, xmltext_to_slate,
    Node, ReviewMeta, ReviewMetaEntry, ReviewMetaPatch,
};
use serde_json::json;
use std::collections::BTreeMap;
use yrs::types::text::YChange;
use yrs::{
    updates::decoder::Decode, Any, Doc, Map, OffsetKind, Options, Out, ReadTxn, Text, Transact,
    Update, WriteTxn, Xml, XmlTextRef,
};

#[test]
fn parses_core_plate_shapes() {
    let nodes = block_markdown_to_slate(
        "Paragraph **bold** *em* <u>u</u> ~~s~~ `code` [link](https://x.test) [[Doc|Label]].",
    )
    .unwrap();

    assert_eq!(
        serde_json::to_value(&nodes).unwrap(),
        json!([
            {
                "type": "p",
                "children": [
                    { "text": "Paragraph " },
                    { "bold": true, "text": "bold" },
                    { "text": " " },
                    { "italic": true, "text": "em" },
                    { "text": " " },
                    { "underline": true, "text": "u" },
                    { "text": " " },
                    { "strikethrough": true, "text": "s" },
                    { "text": " " },
                    { "code": true, "text": "code" },
                    { "text": " " },
                    { "type": "a", "url": "https://x.test", "children": [{ "text": "link" }] },
                    { "text": " " },
                    { "type": "wikilink", "target": "Doc", "alias": "Label", "children": [{ "text": "" }] },
                    { "text": "." }
                ]
            }
        ])
    );
}

#[test]
fn parses_list_block_as_multiple_top_level_nodes() {
    let nodes = block_markdown_to_slate("- one\n  - nested\n- two\n").unwrap();

    assert_eq!(
        serde_json::to_value(&nodes).unwrap(),
        json!([
            { "type": "p", "indent": 1, "listStyleType": "disc", "children": [{ "text": "one" }] },
            { "type": "p", "indent": 2, "listStyleType": "disc", "children": [{ "text": "nested" }] },
            { "type": "p", "indent": 1, "listStyleType": "disc", "children": [{ "text": "two" }] }
        ])
    );
}

#[test]
fn parses_blocks_that_plate_models_as_voids_or_structures() {
    let nodes = block_markdown_to_slate(
        "![alt](assets/x.png)\n\n```mermaid\ngraph TD; A-->B;\n```\n\n| A | B |\n| - | - |\n| 1 | 2 |\n",
    )
    .unwrap();

    assert_eq!(
        serde_json::to_value(&nodes).unwrap(),
        json!([
            { "type": "img", "caption": [{ "text": "alt" }], "url": "assets/x.png", "children": [{ "text": "" }] },
            { "type": "mermaid", "code": "graph TD; A-->B;", "children": [{ "text": "" }] },
            {
                "type": "table",
                "align": [null, null],
                "children": [
                    {
                        "type": "tr",
                        "children": [
                            { "type": "th", "children": [{ "type": "p", "children": [{ "text": "A" }] }] },
                            { "type": "th", "children": [{ "type": "p", "children": [{ "text": "B" }] }] }
                        ]
                    },
                    {
                        "type": "tr",
                        "children": [
                            { "type": "td", "children": [{ "type": "p", "children": [{ "text": "1" }] }] },
                            { "type": "td", "children": [{ "type": "p", "children": [{ "text": "2" }] }] }
                        ]
                    }
                ]
            }
        ])
    );
}

#[test]
fn unsupported_critic_markup_returns_error() {
    assert!(block_markdown_to_slate("See {==here==}{#c1}.\n").is_err());
}

#[test]
fn zero_width_placeholder_block_matches_plate_empty_paragraph() {
    let nodes = block_markdown_to_slate("\u{200b}\n\n").unwrap();

    assert_eq!(
        serde_json::to_value(&nodes).unwrap(),
        json!([{ "type": "p", "children": [{ "text": "" }] }])
    );
}

#[test]
fn yjs_build_round_trips_observable_slate() {
    let nodes = block_markdown_to_slate("# 😀 UTF16\n\nA 👍 **B**\n").unwrap();
    let built = build_nodes(&nodes).unwrap();
    let doc = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    });
    let root = {
        let mut txn = doc.transact_mut();
        let text = txn.get_or_insert_text("content");
        let root: &XmlTextRef = text.as_ref();
        apply_built(&mut txn, root, 0, &built);
        root.clone()
    };
    let txn = doc.transact();
    let first = root.diff(&txn, YChange::identity).remove(0);
    let Out::YXmlText(first_child) = first.insert else {
        panic!("expected embedded XML text");
    };
    assert!(first.attributes.is_none());
    assert_eq!(
        first_child.get_attribute(&txn, "type"),
        Some(Out::Any(Any::String("h1".into())))
    );
    let observable = xmltext_to_slate(&txn, &root).unwrap();
    let Node::Element { children, .. } = observable else {
        panic!("expected fragment");
    };

    assert_eq!(children, nodes);
}

#[test]
fn strips_trailing_empty_paragraphs() {
    let nodes = block_markdown_to_slate("Keep me.\n").unwrap();
    let mut with_trailing = nodes.clone();
    with_trailing.push(Node::element(
        "p",
        Default::default(),
        vec![Node::text("", Default::default())],
    ));

    assert_eq!(strip_trailing_empty_paragraphs(&with_trailing), nodes);
}

#[test]
fn reads_js_slate_yjs_room_update() {
    let update = [
        4, 8, 137, 212, 196, 229, 14, 0, 135, 130, 224, 248, 245, 12, 21, 6, 40, 0, 137, 212, 196,
        229, 14, 0, 4, 116, 121, 112, 101, 1, 119, 2, 104, 49, 4, 0, 137, 212, 196, 229, 14, 0, 8,
        85, 110, 116, 105, 116, 108, 101, 100, 135, 137, 212, 196, 229, 14, 0, 6, 40, 0, 137, 212,
        196, 229, 14, 10, 4, 116, 121, 112, 101, 1, 119, 1, 112, 135, 137, 212, 196, 229, 14, 10,
        6, 40, 0, 137, 212, 196, 229, 14, 12, 4, 116, 121, 112, 101, 1, 119, 1, 112, 4, 0, 137,
        212, 196, 229, 14, 12, 28, 65, 103, 101, 110, 116, 32, 105, 110, 106, 101, 99, 116, 105,
        111, 110, 32, 108, 105, 118, 101, 32, 114, 111, 117, 110, 100, 32, 50, 8, 130, 224, 248,
        245, 12, 0, 129, 226, 151, 170, 143, 5, 12, 1, 0, 2, 129, 130, 224, 248, 245, 12, 0, 1, 0,
        14, 129, 130, 224, 248, 245, 12, 3, 1, 0, 2, 129, 130, 224, 248, 245, 12, 18, 1, 0, 2, 1,
        226, 169, 157, 161, 6, 0, 0, 3, 6, 226, 151, 170, 143, 5, 0, 1, 1, 7, 99, 111, 110, 116,
        101, 110, 116, 1, 0, 9, 129, 226, 151, 170, 143, 5, 0, 1, 0, 1, 129, 226, 151, 170, 143, 5,
        10, 1, 0, 29, 3, 130, 224, 248, 245, 12, 1, 0, 24, 226, 169, 157, 161, 6, 1, 0, 3, 226,
        151, 170, 143, 5, 1, 0, 42,
    ];
    let doc = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    });
    let decoded = Update::decode_v1(&update).unwrap();
    {
        let mut txn = doc.transact_mut();
        txn.apply_update(decoded).unwrap();
    }
    let txn = doc.transact();
    let root = txn.get_text("content").unwrap();
    let root: &XmlTextRef = root.as_ref();
    let Node::Element { children, .. } = xmltext_to_slate(&txn, root).unwrap() else {
        panic!("expected fragment");
    };

    assert_eq!(
        serde_json::to_value(children).unwrap(),
        json!([
            { "type": "h1", "children": [{ "text": "Untitled" }] },
            { "type": "p", "children": [{ "text": "" }] },
            { "type": "p", "children": [{ "text": "Agent injection live round 2" }] }
        ])
    );
}

#[test]
fn review_seed_update_contains_body_and_review_map() {
    let nodes = block_markdown_to_slate("Hello\n").unwrap();
    let built = build_nodes(&nodes).unwrap();
    let mut meta = ReviewMeta::default();
    meta.comments.insert(
        "c1".to_string(),
        ReviewMetaEntry {
            by: "user".to_string(),
            at: "2026-01-01T00:00:00.000Z".to_string(),
            edited_at: None,
            body: Some("note".to_string()),
            re: None,
            status: Some("resolved".to_string()),
            resolved: None,
        },
    );

    let update = encode_update_v1_from_built_with_review(&built, "content", "review", &meta);
    let doc = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    });
    {
        let mut txn = doc.transact_mut();
        txn.apply_update(Update::decode_v1(&update).unwrap())
            .unwrap();
    }
    let txn = doc.transact();
    let review = txn.get_map("review").expect("review map exists");
    let Out::YMap(comments) = review.get(&txn, "comments").expect("comments map exists") else {
        panic!("comments must be a nested map");
    };
    let entry: ReviewMetaEntry = comments.get_as(&txn, "c1").unwrap();

    assert_eq!(entry.by, "user");
    assert_eq!(entry.body.as_deref(), Some("note"));
    assert_eq!(entry.status.as_deref(), Some("resolved"));
    assert_eq!(
        xmltext_to_slate(&txn, txn.get_text("content").unwrap().as_ref()).unwrap(),
        { Node::element("fragment", Default::default(), nodes) }
    );
}

#[test]
fn review_patch_map_merges_and_removes_without_touching_other_ids() {
    let doc = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    });
    {
        let mut txn = doc.transact_mut();
        let review = txn.get_or_insert_map("review");
        apply_review_patch_to_map(
            &mut txn,
            &review,
            &ReviewMetaPatch {
                comments: BTreeMap::from([
                    ("c1".to_string(), review_entry("user")),
                    ("c2".to_string(), review_entry("ai:codex")),
                ]),
                suggestions: BTreeMap::from([("s1".to_string(), review_entry("ai:codex"))]),
                remove_comments: Vec::new(),
                remove_suggestions: Vec::new(),
            },
        );
        apply_review_patch_to_map(
            &mut txn,
            &review,
            &ReviewMetaPatch {
                comments: BTreeMap::from([("c3".to_string(), review_entry("reviewer"))]),
                suggestions: BTreeMap::new(),
                remove_comments: vec!["c1".to_string()],
                remove_suggestions: vec!["s1".to_string()],
            },
        );
    }
    let txn = doc.transact();
    let review = txn.get_map("review").expect("review map exists");
    let Out::YMap(comments) = review.get(&txn, "comments").expect("comments map exists") else {
        panic!("comments must be a nested map");
    };
    let Out::YMap(suggestions) = review
        .get(&txn, "suggestions")
        .expect("suggestions map exists")
    else {
        panic!("suggestions must be a nested map");
    };

    assert!(!comments.contains_key(&txn, "c1"));
    assert!(comments.contains_key(&txn, "c2"));
    assert!(comments.contains_key(&txn, "c3"));
    assert!(!suggestions.contains_key(&txn, "s1"));
}

fn review_entry(by: &str) -> ReviewMetaEntry {
    ReviewMetaEntry {
        by: by.to_string(),
        at: "2026-01-01T00:00:00.000Z".to_string(),
        edited_at: None,
        body: None,
        re: None,
        status: None,
        resolved: None,
    }
}
