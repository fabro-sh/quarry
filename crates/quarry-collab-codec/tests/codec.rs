use quarry_collab_codec::{
    apply_built, block_markdown_to_slate, build_nodes, strip_trailing_empty_paragraphs,
    xmltext_to_slate, Node,
};
use serde_json::json;
use yrs::types::text::YChange;
use yrs::{
    updates::decoder::Decode, Any, Doc, OffsetKind, Options, Out, ReadTxn, Text, Transact, Update,
    WriteTxn, Xml, XmlTextRef,
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
