#![allow(clippy::unwrap_used, reason = "explicit import fixtures")]
use quarry_document::{Document, TargetOwner, TargetState};
use quarry_markdown::{Node, ReviewMeta, import_markdown_document, import_review_nodes};
use serde_json::json;

fn import(markdown: &str) -> Document {
    let mut index = 0;
    import_markdown_document("import", markdown, &mut || {
        index += 1;
        format!("id{index}")
    })
    .unwrap()
}

#[test]
fn partial_wiki_formatting_exports_valid_syntax_without_changing_native_marks() {
    let syntax = "![[Other#Part|😀label]]";
    let markdown = format!("See {syntax} then {syntax}.\n");
    for mark in quarry_document::INLINE_BOOLEAN_MARKS {
        let mut offset = 4;
        for character in syntax.chars() {
            let mut document = import(&markdown);
            let id = document.blocks().unwrap()[0].id.clone();
            let selection = document
                .selection(&id, offset, offset + character.len_utf16())
                .unwrap();
            document.format(&selection, mark, &json!(true)).unwrap();
            let before = document.view().unwrap();
            let exported = quarry_markdown::document_to_markdown(&document).unwrap();
            assert_eq!(exported, markdown, "{mark} at {offset}");
            assert_eq!(
                quarry_markdown::document_to_markdown(&import(&exported)).unwrap(),
                exported
            );
            assert_eq!(document.view().unwrap(), before);
            assert_eq!(
                Document::load(&document.save()).unwrap().view().unwrap(),
                before
            );
            offset += character.len_utf16();
        }
    }
}

#[test]
fn wiki_export_keeps_whole_link_formatting_and_formats_surrounding_text_separately() {
    let syntax = "[[Other#Part|😀label]]";
    let mut document = import(&format!("Before {syntax} after {syntax}.\n"));
    let id = document.blocks().unwrap()[0].id.clone();
    let end = 7 + syntax.encode_utf16().count();
    document
        .format(
            &document.selection(&id, 0, end + 6).unwrap(),
            "italic",
            &json!(true),
        )
        .unwrap();
    document
        .format(
            &document.selection(&id, 7 + 13, 7 + 15).unwrap(),
            "bold",
            &json!(true),
        )
        .unwrap();
    let exported = quarry_markdown::document_to_markdown(&document).unwrap();
    assert_eq!(exported, format!("*Before {syntax} after* {syntax}.\n"));
    assert_eq!(
        quarry_markdown::document_to_markdown(&import(&exported)).unwrap(),
        exported
    );
}

#[test]
fn imports_unicode_threads_and_substitutions_without_review_syntax_in_body() {
    let mut document = import(
        "See {==😀TARGET==}{>>Keep<<}{#c} and {~~old~>new~~}{#s}.\n\n---\ncomments:\n  c: {by: Reviewer, at: '2026-08-01T00:00:00Z'}\n  reply: {by: Agent, body: Agreed, re: c}\nsuggestions:\n  s: {by: Agent, body: Clearer}\n",
    );
    assert_eq!(
        document.view().unwrap().blocks[0].text,
        "See 😀TARGET and old."
    );
    assert_eq!(
        document.comment_target("c").unwrap().attachments[0].quote,
        "😀TARGET"
    );
    assert_eq!(
        document.comment("reply").unwrap().parent_id.as_deref(),
        Some("c")
    );
    assert_eq!(document.view().unwrap().proposals[0].text, "new");
    document.accept_proposal("s").unwrap();
    assert_eq!(
        document.view().unwrap().blocks[0].text,
        "See 😀TARGET and new."
    );
    assert_eq!(
        Document::load(&document.save()).unwrap().view().unwrap(),
        document.view().unwrap()
    );
}

#[test]
fn comments_on_inserted_proposal_text_follow_acceptance() {
    let nodes: Vec<Node> = serde_json::from_value(json!([{"type":"p","children":[
        {"text":"Before "},
        {"text":"😀proposed","bold":true,"suggestion_s":{"type":"insert"},"comment_c":true},
        {"text":" after"}
    ]}]))
    .unwrap();
    let meta: ReviewMeta = serde_json::from_value(json!({"comments":{"c":{"by":"Reviewer","body":"Keep"}},"suggestions":{"s":{"by":"Agent"}}})).unwrap();
    let mut doc =
        import_review_nodes("proposal-import", &nodes, &meta, &mut || "p".into()).unwrap();
    assert_eq!(
        doc.comment_target("c").unwrap().attachments[0].owner,
        TargetOwner::Proposal("s".into())
    );
    assert_eq!(doc.view().unwrap().blocks[0].text, "Before  after");
    assert_eq!(doc.view().unwrap().proposals[0].runs[0].marks["bold"], true);
    doc.accept_proposal("s").unwrap();
    assert_eq!(
        doc.comment_target("c").unwrap().attachments[0].owner,
        TargetOwner::Block("p".into())
    );
    assert_eq!(
        doc.comment_target("c").unwrap().attachments[0].quote,
        "😀proposed"
    );
}

#[test]
fn missing_markers_are_explicit_and_anonymous_markers_get_persistent_ids() {
    let doc = import(
        "{==TARGET==}{>>Keep<<} and {++new++}\n\n---\ncomments:\n  missing: {by: Reviewer, body: Lost}\nsuggestions:\n  missing-s: {by: Agent, body: Lost}\n",
    );
    assert_eq!(doc.comments().unwrap().len(), 2);
    assert_eq!(doc.proposals().unwrap().len(), 2);
    assert_eq!(
        doc.comment_target("missing").unwrap().state,
        TargetState::Unattached
    );
    assert_eq!(
        doc.proposal_target("missing-s").unwrap().state,
        TargetState::Unattached
    );
    assert!(
        doc.comment("missing")
            .unwrap()
            .metadata
            .legacy_record
            .is_some()
    );
    let restored = Document::load(&doc.save()).unwrap();
    assert_eq!(restored.view().unwrap(), doc.view().unwrap());
}

#[test]
fn complete_block_deletions_and_literal_code_keep_their_semantics() {
    let mut doc = import("{--Remove me--}{#s}\n\n`{++literal++}`\n");
    assert_eq!(doc.view().unwrap().blocks.len(), 2);
    doc.accept_proposal("s").unwrap();
    assert_eq!(doc.view().unwrap().blocks.len(), 1);
    assert_eq!(doc.view().unwrap().blocks[0].text, "{++literal++}");
}

#[test]
fn markdown_corpus_retains_native_review_and_stable_body_export_after_reload() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/markdown");
    let mut count = 0;
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let markdown = std::fs::read_to_string(&path).unwrap();
        let mut id = 0;
        let imported = quarry_markdown::import_markdown_document("corpus", &markdown, &mut || {
            id += 1;
            format!("block-{id}")
        })
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let loaded = quarry_document::Document::load(&imported.save()).unwrap();
        assert_eq!(
            loaded.view().unwrap(),
            imported.view().unwrap(),
            "{}",
            path.display()
        );
        let exported = quarry_markdown::document_to_markdown(&loaded).unwrap();
        let mut id = 0;
        let body = quarry_markdown::import_markdown_document("body", &exported, &mut || {
            id += 1;
            format!("block-{id}")
        })
        .unwrap_or_else(|error| panic!("export {}: {error}", path.display()));
        assert_eq!(
            quarry_markdown::document_to_markdown(&body).unwrap(),
            exported,
            "{}",
            path.display()
        );
        count += 1;
    }
    assert!(count >= 20, "the document input corpus must not disappear");
}
