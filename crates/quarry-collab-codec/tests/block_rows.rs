//! Markdown ↔ block rows: the Phase 1 storage projection of the codec.
//!
//! Import mints fresh block ids, flattens lists Plate-style, nests only real
//! containers (`code_block` → `code_line`, tables), and falls back to
//! `raw_markdown` rows for safe top-level constructs the row model cannot
//! hold. Export is deterministic and idempotent after one-time normalization.

use quarry_collab_codec::slate::attrs;
use quarry_collab_codec::{
    block_rows_to_markdown, block_rows_to_nodes, is_utf16_boundary, markdown_to_block_rows,
    utf16_len, BlockRow, LinkRange, MarkRun, Node,
};
use serde_json::json;

fn sequential_ids() -> impl FnMut() -> String {
    let mut next = 0u32;
    move || {
        next += 1;
        format!("b{next}")
    }
}

fn import(markdown: &str) -> Vec<BlockRow> {
    markdown_to_block_rows(markdown, sequential_ids()).expect("fixture imports")
}

fn export(rows: &[BlockRow]) -> String {
    block_rows_to_markdown(rows).expect("rows export")
}

fn reexport(markdown: &str) -> String {
    export(&import(markdown))
}

#[test]
fn imports_heading_and_marked_paragraph_with_fresh_ids_and_utf16_offsets() {
    let rows = import("# Gate 👍 heading\n\nIntro with **bold** and *italic* runs.\n");

    assert_eq!(
        rows,
        vec![
            BlockRow {
                block_id: "b1".to_string(),
                parent_block_id: None,
                position: 0,
                block_type: "h1".to_string(),
                attrs: attrs([] as [(&str, serde_json::Value); 0]),
                text: "Gate 👍 heading".to_string(),
                marks: vec![],
                links: vec![],
            },
            BlockRow {
                block_id: "b2".to_string(),
                parent_block_id: None,
                position: 1,
                block_type: "p".to_string(),
                attrs: attrs([] as [(&str, serde_json::Value); 0]),
                text: "Intro with bold and italic runs.".to_string(),
                marks: vec![
                    MarkRun {
                        start: 11,
                        end: 15,
                        marks: attrs([("bold", json!(true))]),
                    },
                    MarkRun {
                        start: 20,
                        end: 26,
                        marks: attrs([("italic", json!(true))]),
                    },
                ],
                links: vec![],
            },
        ]
    );
}

#[test]
fn imports_links_as_ranges_over_flat_text() {
    let rows = import("See the [docs site](https://example.test/docs) for details.\n");

    assert_eq!(rows[0].text, "See the docs site for details.");
    assert_eq!(
        rows[0].links,
        vec![LinkRange {
            start: 8,
            end: 17,
            url: "https://example.test/docs".to_string(),
        }]
    );
}

#[test]
fn imports_code_blocks_as_nested_code_line_rows() {
    let rows = import("```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n");

    let parents: Vec<(&str, Option<&str>, u32, &str)> = rows
        .iter()
        .map(|row| {
            (
                row.block_id.as_str(),
                row.parent_block_id.as_deref(),
                row.position,
                row.block_type.as_str(),
            )
        })
        .collect();
    assert_eq!(
        parents,
        vec![
            ("b1", None, 0, "code_block"),
            ("b2", Some("b1"), 0, "code_line"),
            ("b3", Some("b1"), 1, "code_line"),
            ("b4", Some("b1"), 2, "code_line"),
        ]
    );
    assert_eq!(rows[0].attrs, attrs([("lang", json!("rust"))]));
    assert_eq!(rows[2].text, "    println!(\"hi\");");
}

#[test]
fn imports_nested_list_items_as_flat_rows_with_indent_attrs() {
    let rows = import("- one\n    - nested\n- two\n");

    let items: Vec<(&str, u32, serde_json::Value)> = rows
        .iter()
        .map(|row| {
            (
                row.text.as_str(),
                row.position,
                serde_json::to_value(&row.attrs).unwrap(),
            )
        })
        .collect();
    assert_eq!(
        items,
        vec![
            ("one", 0, json!({"indent": 1, "listStyleType": "disc"})),
            ("nested", 1, json!({"indent": 2, "listStyleType": "disc"})),
            ("two", 2, json!({"indent": 1, "listStyleType": "disc"})),
        ]
    );
}

#[test]
fn safe_unsupported_block_falls_back_to_raw_markdown_row() {
    let rows = import("Before.\n\n<div>\nopaque html\n</div>\n\nAfter.\n");

    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1].block_type, "raw_markdown");
    assert_eq!(
        rows[1].attrs,
        attrs([("markdown", json!("<div>\nopaque html\n</div>"))])
    );
    assert_eq!(rows[2].text, "After.");
    assert_eq!(rows[2].position, 2);
}

#[test]
fn wikilink_paragraph_falls_back_to_raw_markdown_preserving_source() {
    let rows = import("Linked to [[Other Note|alias]] here.\n");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].block_type, "raw_markdown");
    assert_eq!(
        rows[0].attrs.get("markdown"),
        Some(&json!("Linked to [[Other Note|alias]] here."))
    );
}

#[test]
fn critic_markup_returns_the_typed_unsupported_error() {
    let error = markdown_to_block_rows("See {==here==}{#c1}.\n", sequential_ids()).unwrap_err();

    assert_eq!(error.0, "critic markup");
}

#[test]
fn block_rows_to_nodes_places_block_id_as_the_id_attribute() {
    let nodes = block_rows_to_nodes(&import("Hello world.\n")).unwrap();

    assert_eq!(
        serde_json::to_value(&nodes).unwrap(),
        json!([
            {
                "type": "p",
                "id": "b1",
                "children": [{ "text": "Hello world." }]
            }
        ])
    );
}

#[test]
fn block_rows_to_nodes_rejects_orphaned_parent_references() {
    let orphan = BlockRow {
        block_id: "b1".to_string(),
        parent_block_id: Some("missing".to_string()),
        position: 0,
        block_type: "p".to_string(),
        attrs: attrs([] as [(&str, serde_json::Value); 0]),
        text: "lost".to_string(),
        marks: vec![],
        links: vec![],
    };

    assert!(block_rows_to_nodes(std::slice::from_ref(&orphan)).is_err());
}

#[test]
fn exports_marks_links_and_structure_back_to_markdown() {
    let markdown = "\
# Title

Intro with **bold** and *italic* and `code` and ~~gone~~.

See the [docs site](https://example.test/docs) for details.

- one
    - nested
- two

1. first
2. second

- [x] done
- [ ] todo

> Quoted line.
>
> Second paragraph.

```rust
fn main() {}
```

***

![alt text](assets/pic.png)

| A | B |
| --- | --- |
| 1 | 2 |
";

    assert_eq!(reexport(markdown), markdown);
}

#[test]
fn export_is_idempotent_after_one_normalization_pass() {
    // Deliberately denormalized input: setext heading, loose list, indented
    // code, soft-wrapped paragraph, link title (dropped), html fallback,
    // underline/sub/sup forms, critic-free punctuation soup.
    let markdown = "\
Title line
==========

para one
wrapped line with literal *stars* and 1986 numbers

- loose item one

- loose item two

1) paren ordered

    indented code block

[titled](https://example.test \"title\")

<u>under</u> and H~2~O and x^2^

<div data-x=\"1\">
raw block
</div>

| Left | Right |
|:-----|------:|
| a | b |
";

    let once = reexport(markdown);
    let twice = reexport(&once);
    assert_eq!(twice, once);
}

#[test]
fn export_escapes_text_that_would_reparse_as_syntax() {
    // Mid-line "1." cannot form a list marker, so only structural characters
    // get escaped; the line-start digit case is covered separately below.
    let rows = import("Literal \\*stars\\*, 1\\. not a list, a \\| pipe, AT&T.\n");

    let exported = export(&rows);
    assert_eq!(
        exported,
        "Literal \\*stars\\*, 1. not a list, a \\| pipe, AT\\&T.\n"
    );
    assert_eq!(reexport(&exported), exported);
}

#[test]
fn export_escapes_line_start_digits_that_would_become_ordered_lists() {
    let rows = import("1\\. literal numbered line\n");

    let exported = export(&rows);
    assert_eq!(exported, "1\\. literal numbered line\n");
    assert_eq!(reexport(&exported), exported);
}

#[test]
fn frontmatterless_round_trip_of_emoji_keeps_utf16_offsets_exact() {
    let rows = import("Emoji 👍 then **bold**.\n");

    assert_eq!(rows[0].text, "Emoji 👍 then bold.");
    // "Emoji 👍 then " = 6 + 2 (surrogate pair) + 6 = 14 UTF-16 units.
    assert_eq!(
        rows[0].marks,
        vec![MarkRun {
            start: 14,
            end: 18,
            marks: attrs([("bold", json!(true))]),
        }]
    );
    assert_eq!(export(&rows), "Emoji 👍 then **bold**.\n");
}

#[test]
fn utf16_helpers_count_code_units_and_detect_surrogate_splits() {
    assert_eq!(utf16_len("Gate 👍!"), 8);
    assert!(is_utf16_boundary("Gate 👍!", 5));
    assert!(is_utf16_boundary("Gate 👍!", 7));
    assert!(is_utf16_boundary("Gate 👍!", 8));
    assert!(!is_utf16_boundary("Gate 👍!", 6));
    assert!(!is_utf16_boundary("Gate 👍!", 9));
}

#[test]
fn empty_document_imports_to_no_rows_and_exports_to_empty_string() {
    let rows = import("");

    assert!(rows.is_empty());
    assert_eq!(export(&rows), "");
}

#[test]
fn mermaid_and_empty_paragraph_blocks_round_trip() {
    let markdown = "```mermaid\ngraph TD; A-->B;\n```\n";

    let rows = import(markdown);
    assert_eq!(rows[0].block_type, "mermaid");
    assert_eq!(reexport(markdown), markdown);
}

#[test]
fn raw_fallback_blocks_survive_reexport_byte_identically() {
    let markdown = "Before.\n\n[^note]: a footnote definition\n\nAfter.\n";

    let once = reexport(markdown);
    let twice = reexport(&once);
    assert_eq!(twice, once);
    assert!(once.contains("[^note]: a footnote definition"));
}

#[test]
fn bold_link_groups_share_the_surrounding_mark_delimiters() {
    let markdown = "Read **the [linked](https://x.test) words** now.\n";

    let rows = import(markdown);
    let nodes = block_rows_to_nodes(&rows).unwrap();
    let Node::Element { children, .. } = &nodes[0] else {
        panic!("paragraph expected");
    };
    assert_eq!(children.len(), 5);
    assert_eq!(reexport(markdown), markdown);
}

#[test]
fn multi_line_setext_heading_joins_with_spaces_and_stays_idempotent() {
    let once = reexport("alpha\nbeta\n===\n");

    assert_eq!(once, "# alpha beta\n");
    assert_eq!(reexport(&once), once);
}

#[test]
fn empty_heading_exports_without_a_trailing_space() {
    let once = reexport("##\n");

    assert_eq!(once, "##\n");
    assert_eq!(reexport(&once), once);
}

#[test]
fn all_space_code_span_does_not_grow_on_round_trip() {
    let once = reexport("a ` ` b\n");

    assert_eq!(once, "a ` ` b\n");
    assert_eq!(reexport(&once), once);
}

#[test]
fn link_reference_definitions_are_preserved_as_raw_rows() {
    let markdown =
        "See [docs][ref].\n\n[ref]: https://example.test\n\n[unused]: https://y.test \"Title\"\n";

    let rows = import(markdown);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].block_type, "p");
    assert_eq!(rows[0].links[0].url, "https://example.test");
    assert_eq!(rows[1].block_type, "raw_markdown");
    assert_eq!(
        rows[1].attrs.get("markdown"),
        Some(&json!("[ref]: https://example.test"))
    );
    assert_eq!(rows[2].block_type, "raw_markdown");
    assert_eq!(
        rows[2].attrs.get("markdown"),
        Some(&json!("[unused]: https://y.test \"Title\""))
    );

    // The used reference inlines into its link; both definition lines stay.
    let once = export(&rows);
    assert_eq!(
        once,
        "See [docs](https://example.test).\n\n[ref]: https://example.test\n\n[unused]: https://y.test \"Title\"\n"
    );
    assert_eq!(reexport(&once), once);
}

#[test]
fn unused_link_reference_definition_alone_survives_round_trip() {
    let markdown = "para\n\n[unused]: https://x.test\n\nafter\n";

    let once = reexport(markdown);
    assert_eq!(once, markdown);
    assert_eq!(reexport(&once), once);
}

#[test]
fn mark_runs_with_edge_whitespace_hoist_the_whitespace_outside_delimiters() {
    // Import can never produce this shape (CommonMark flanking rules), but
    // Phase 2 mark ops will: a bold range covering a trailing space.
    let row = BlockRow {
        block_id: "b1".to_string(),
        parent_block_id: None,
        position: 0,
        block_type: "p".to_string(),
        attrs: attrs([] as [(&str, serde_json::Value); 0]),
        text: "bold text".to_string(),
        marks: vec![MarkRun {
            start: 0,
            end: 5,
            marks: attrs([("bold", json!(true))]),
        }],
        links: vec![],
    };

    let once = export(std::slice::from_ref(&row));
    assert_eq!(once, "**bold** text\n");
    assert_eq!(reexport(&once), once);
}

#[test]
fn whitespace_only_mark_run_drops_the_meaningless_delimiters() {
    let row = BlockRow {
        block_id: "b1".to_string(),
        parent_block_id: None,
        position: 0,
        block_type: "p".to_string(),
        attrs: attrs([] as [(&str, serde_json::Value); 0]),
        text: "a b".to_string(),
        marks: vec![MarkRun {
            start: 1,
            end: 2,
            marks: attrs([("bold", json!(true))]),
        }],
        links: vec![],
    };

    let once = export(std::slice::from_ref(&row));
    assert_eq!(once, "a b\n");
    assert_eq!(reexport(&once), once);
}
