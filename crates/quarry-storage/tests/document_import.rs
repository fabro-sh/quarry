#![allow(clippy::unwrap_used, reason = "tests use explicit migration fixtures")]
use quarry_document::{Document, ProposalState, TargetState};
use quarry_storage::{
    BlockMutationState, BlockReviewItem, BlockReviewKind, BlockReviewState,
    MARKDOWN_INSERT_SUGGESTION_CONTEXT, document_projection, document_review_projection,
    import_document,
};
use serde_json::json;

fn item(id: &str, block: &str, kind: BlockReviewKind) -> BlockReviewItem {
    BlockReviewItem {
        id: id.into(),
        document_id: "migration-fixture".into(),
        block_id: block.into(),
        kind,
        start_offset: 4,
        end_offset: 10,
        body: Some("Review body".into()),
        replacement: None,
        author: Some("Reviewer".into()),
        state: BlockReviewState::Open,
        quote: Some("TARGET".into()),
        context_before: None,
        context_after: None,
        parent_item_id: None,
        created_at: "2026-08-01T12:00:00Z".into(),
        updated_at: "2026-08-02T12:00:00Z".into(),
    }
}

#[test]
fn markdown_review_reuses_verified_block_identity_and_preserves_disagreement() {
    let rows = quarry_markdown::markdown_to_block_rows("# Title\n\nSee TARGET and old.\n", {
        let mut id = 0;
        move || {
            id += 1;
            format!("old-{id}")
        }
    })
    .unwrap();
    let state = BlockMutationState {
        document_id: "mixed-import".into(),
        path: "review.md".into(),
        head_version_id: "old".into(),
        content_type: "text/markdown".into(),
        metadata: json!({}),
        rows,
        review_items: Vec::new(),
        version_ids: Default::default(),
        replay: None,
    };
    let markdown = "# Title\n\nSee {==TARGET==}{>>Keep<<}{#c} and {~~old~>new~~}{#s}.\n";
    let imported = quarry_storage::import_document_with_markdown(&state, markdown).unwrap();
    assert_eq!(document_projection(&imported).unwrap(), state.rows);
    assert_eq!(
        imported.comment_target("c").unwrap().attachments[0].owner,
        quarry_document::TargetOwner::Block("old-2".into())
    );
    assert!(imported.validate_proposal_acceptance("s").is_ok());

    let mut changed = state.clone();
    changed.rows[1].text = "Unrelated TARGET elsewhere".into();
    let imported = quarry_storage::import_document_with_markdown(&changed, markdown).unwrap();
    assert_eq!(document_projection(&imported).unwrap(), changed.rows);
    assert_eq!(imported.comment("c").unwrap().original_quote, "TARGET");
    assert_eq!(
        imported.comment_target("c").unwrap().state,
        TargetState::Unattached
    );
    let proposal = imported.proposal("s").unwrap();
    assert_eq!(proposal.metadata.legacy_record.unwrap()["text"], "new");
    assert!(imported.validate_proposal_acceptance("s").is_err());
    assert_eq!(
        Document::load(&imported.save()).unwrap().view().unwrap(),
        imported.view().unwrap()
    );
}

#[test]
fn migration_preserves_nested_blocks_formatting_threads_pending_proposals_and_legacy_records() {
    let mut next = 0;
    let rows = quarry_markdown::markdown_to_block_rows(
        "# Title\n\nSee [TARGET](https://example.com) here.\n\n| Column |\n| --- |\n| **Cell** |\n",
        || {
            next += 1;
            format!("b{next}")
        },
    )
    .unwrap();
    let paragraph = rows
        .iter()
        .find(|r| r.text == "See TARGET here.")
        .unwrap()
        .block_id
        .clone();
    let root = item("root", &paragraph, BlockReviewKind::Comment);
    let reply = BlockReviewItem {
        id: "reply".into(),
        parent_item_id: Some("root".into()),
        ..root.clone()
    };
    let orphan = BlockReviewItem {
        id: "orphan".into(),
        block_id: "missing".into(),
        state: BlockReviewState::Orphaned,
        start_offset: 0,
        end_offset: 0,
        ..root.clone()
    };
    let replacement = BlockReviewItem {
        replacement: Some("PROPOSED".into()),
        ..item("replacement", &paragraph, BlockReviewKind::Suggestion)
    };
    let closed = BlockReviewItem {
        id: "closed".into(),
        state: BlockReviewState::Resolved,
        ..replacement.clone()
    };
    let markdown = BlockReviewItem {
        start_offset: 0,
        end_offset: 0,
        context_after: Some(MARKDOWN_INSERT_SUGGESTION_CONTEXT.into()),
        replacement: Some("**BOLD** and [LINK](https://example.com)\n\nSecond\n".into()),
        ..item("markdown", &paragraph, BlockReviewKind::Suggestion)
    };
    let conflict = BlockReviewItem {
        body: Some("Incoming".into()),
        context_before: Some("Base".into()),
        quote: Some("Canonical".into()),
        start_offset: 0,
        end_offset: 0,
        ..item("conflict", &paragraph, BlockReviewKind::Conflict)
    };
    // Deliberately reverse both trees and reply order. No row-order dependency.
    let mut reversed = rows.clone();
    reversed.reverse();
    let state = BlockMutationState {
        document_id: "migration-fixture".into(),
        path: "doc.md".into(),
        head_version_id: "old".into(),
        content_type: "text/markdown".into(),
        metadata: json!({}),
        rows: reversed,
        review_items: vec![
            reply,
            orphan.clone(),
            replacement,
            closed.clone(),
            markdown,
            conflict.clone(),
            root,
        ],
        version_ids: Default::default(),
        replay: None,
    };
    let d = import_document(&state).unwrap();
    let rendered =
        quarry_markdown::block_rows_to_markdown(&document_projection(&d).unwrap()).unwrap();
    assert!(rendered.contains("[TARGET](https://example.com)"));
    assert!(rendered.contains("**Cell**"));
    assert_eq!(
        d.comment("reply").unwrap().parent_id.as_deref(),
        Some("root")
    );
    assert_eq!(
        d.comment_target("orphan").unwrap().state,
        TargetState::Unattached
    );
    assert_eq!(
        d.comment("orphan").unwrap().metadata.legacy_record,
        Some(serde_json::to_value(orphan).unwrap())
    );
    assert_eq!(d.proposal("closed").unwrap().state, ProposalState::Closed);
    assert_eq!(
        d.proposal("closed").unwrap().metadata.legacy_record,
        Some(serde_json::to_value(closed).unwrap())
    );
    assert_eq!(
        d.conflict("conflict").unwrap().metadata.legacy_record,
        Some(serde_json::to_value(conflict).unwrap())
    );
    assert_eq!(d.conflict("conflict").unwrap().incoming, "Incoming");
    let projected = document_review_projection(&d).unwrap();
    let markdown = projected
        .iter()
        .find(|i| i.id == "markdown")
        .unwrap()
        .replacement
        .as_ref()
        .unwrap();
    assert!(markdown.contains("**BOLD**"));
    assert!(markdown.contains("[LINK](https://example.com)"));
    let restored = Document::load(&d.save()).unwrap();
    assert_eq!(restored.view().unwrap(), d.view().unwrap());
}
