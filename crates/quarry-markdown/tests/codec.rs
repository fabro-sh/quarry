#![allow(clippy::unwrap_used, reason = "known fixtures")]
use quarry_markdown::{Node, block_markdown_to_nodes, strip_trailing_empty_paragraphs};
use serde_json::json;

#[test]
fn parses_core_markdown_shapes() {
    let nodes = block_markdown_to_nodes(
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
    let nodes = block_markdown_to_nodes("- one\n  - nested\n- two\n").unwrap();

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
fn parses_blocks_that_are_voids_or_structures() {
    let nodes = block_markdown_to_nodes(
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
    assert!(block_markdown_to_nodes("See {==here==}{#c1}.\n").is_err());
}

#[test]
fn critic_markers_inside_code_spans_parse_as_literal_code_text() {
    let nodes = block_markdown_to_nodes("A `{++literal++}` marker.\n").unwrap();

    assert_eq!(
        serde_json::to_value(&nodes).unwrap(),
        json!([
            {
                "type": "p",
                "children": [
                    { "text": "A " },
                    { "code": true, "text": "{++literal++}" },
                    { "text": " marker." }
                ]
            }
        ])
    );
}

#[test]
fn zero_width_placeholder_block_is_an_empty_paragraph() {
    let nodes = block_markdown_to_nodes("\u{200b}\n\n").unwrap();

    assert_eq!(
        serde_json::to_value(&nodes).unwrap(),
        json!([{ "type": "p", "children": [{ "text": "" }] }])
    );
}

#[test]
fn strips_trailing_empty_paragraphs() {
    let nodes = block_markdown_to_nodes("Keep me.\n").unwrap();
    let mut with_trailing = nodes.clone();
    with_trailing.push(Node::element(
        "p",
        Default::default(),
        vec![Node::text("", Default::default())],
    ));

    assert_eq!(strip_trailing_empty_paragraphs(&with_trailing), nodes);
}
