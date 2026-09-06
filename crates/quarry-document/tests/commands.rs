#![allow(clippy::unwrap_used, reason = "tests use explicit document fixtures")]

use quarry_document::{Command, CommandRequest, Document, SeedBlock, TargetOwner};
use std::collections::BTreeMap;

fn request(doc: &Document, id: &str, commands: Vec<Command>) -> CommandRequest {
    CommandRequest {
        request_id: id.into(),
        base: doc.heads().iter().map(ToString::to_string).collect(),
        commands,
        at: "2026-09-04T12:00:00Z".into(),
    }
}

fn seed() -> Document {
    let mut doc = Document::with_id("test-document").unwrap();
    doc.insert_block(SeedBlock {
        id: "a".into(),
        kind: "p".into(),
        parent: None,
        position: 0,
        attrs: BTreeMap::new(),
        text: "😀 TARGET".into(),
    })
    .unwrap();
    doc
}

#[test]
fn delayed_subtree_deletion_rejects_unseen_text_and_marks_but_allows_unrelated_edits() {
    for change in ["text", "mark", "unrelated"] {
        let mut base = seed();
        base.insert_block(SeedBlock {
            id: "quote".into(),
            kind: "code_block".into(),
            parent: None,
            position: 1,
            attrs: BTreeMap::new(),
            text: String::new(),
        })
        .unwrap();
        base.insert_block(SeedBlock {
            id: "child".into(),
            kind: "code_line".into(),
            parent: Some("quote".into()),
            position: 0,
            attrs: BTreeMap::new(),
            text: "Child TARGET".into(),
        })
        .unwrap();
        let delete = request(
            &base,
            "delete-subtree",
            vec![Command::DeleteBlock {
                block: "quote".into(),
            }],
        );
        let mut current = Document::load(&base.save()).unwrap();
        match change {
            "text" => current
                .insert_text(&current.point("child", 0).unwrap(), "Browser ")
                .unwrap(),
            "mark" => current
                .format(
                    &current.selection("child", 0, 5).unwrap(),
                    "bold",
                    &serde_json::json!(true),
                )
                .unwrap(),
            _ => current
                .insert_text(&current.point("a", 0).unwrap(), "Browser ")
                .unwrap(),
        }
        let before = current.heads();
        if change == "unrelated" {
            current.apply_request(&delete).unwrap();
            assert!(current.block("quote").unwrap().deleted);
            assert!(current.block("child").unwrap().deleted);
            assert_eq!(current.block_view("a").unwrap().text, "Browser 😀 TARGET");
        } else {
            assert!(current.apply_request(&delete).is_err());
            assert_eq!(current.heads(), before);
            assert!(!current.block("child").unwrap().deleted);
        }
        assert_eq!(
            Document::load(&current.save()).unwrap().view().unwrap(),
            current.view().unwrap()
        );
    }
}

#[test]
fn delayed_typing_into_a_deleted_block_conflicts_while_a_late_comment_keeps_its_hidden_target() {
    let base = seed();
    let mut current = Document::load(&base.save()).unwrap();
    current.delete_block("a").unwrap();
    let before = current.heads();
    let typing = request(
        &base,
        "late-typing",
        vec![Command::InsertText {
            at: base.point("a", 0).unwrap(),
            text: "Unsent browser ".into(),
        }],
    );
    assert!(current.apply_request(&typing).is_err());
    assert_eq!(current.heads(), before);
    current
        .apply_request(&request(
            &base,
            "late-comment",
            vec![Command::AddComment {
                id: "late".into(),
                author: "Reviewer".into(),
                body: "Keep this discussion".into(),
                ranges: base.selection("a", 3, 9).unwrap(),
            }],
        ))
        .unwrap();
    assert_eq!(
        current.comment_target("late").unwrap().state,
        quarry_document::TargetState::Hidden
    );
}

#[test]
fn block_update_proposal_keeps_text_identity_after_typing_and_move_and_replays_with_undo() {
    let mut doc = seed();
    doc.insert_block(SeedBlock {
        id: "b".into(),
        kind: "p".into(),
        parent: None,
        position: 1,
        attrs: BTreeMap::new(),
        text: "Other TARGET".into(),
    })
    .unwrap();
    doc.add_comment("c", "Reviewer", "Keep", &doc.selection("a", 3, 9).unwrap())
        .unwrap();
    let mut replay = Document::load(&doc.save()).unwrap();
    let mut requests = Vec::new();
    let propose = request(
        &doc,
        "block-proposal",
        vec![Command::ProposeBlockUpdate {
            id: "heading".into(),
            author: "Reviewer".into(),
            block: "a".into(),
            kind: "h2".into(),
            attrs: BTreeMap::new(),
        }],
    );
    doc.apply_request(&propose).unwrap();
    requests.push(propose);
    assert_eq!(doc.block("a").unwrap().kind, "p");
    let edit = request(
        &doc,
        "other-editor",
        vec![
            Command::InsertText {
                at: doc.point("a", 0).unwrap(),
                text: "Agent ".into(),
            },
            Command::MoveBlock {
                block: "a".into(),
                parent: None,
                before: None,
            },
        ],
    );
    doc.apply_request(&edit).unwrap();
    requests.push(edit);
    let before = doc.heads();
    let accept = request(
        &doc,
        "accept-heading",
        vec![Command::AcceptProposal {
            id: "heading".into(),
        }],
    );
    doc.apply_request(&accept).unwrap();
    requests.push(accept);
    let after = doc.heads();
    assert_eq!(doc.block("a").unwrap().kind, "h2");
    assert_eq!(doc.block_view("a").unwrap().text, "Agent 😀 TARGET");
    let target = doc.comment_target("c").unwrap();
    assert_eq!(target.attachments[0].owner, TargetOwner::Block("a".into()));
    assert_eq!(target.attachments[0].quote, "TARGET");
    let undo = request(
        &doc,
        "undo-heading",
        vec![Command::Revert {
            before: before.iter().map(ToString::to_string).collect(),
            after: after.iter().map(ToString::to_string).collect(),
        }],
    );
    doc.apply_request(&undo).unwrap();
    requests.push(undo);
    assert_eq!(doc.block("a").unwrap().kind, "p");
    assert_eq!(doc.comment_target("c").unwrap(), target);
    for request in &requests {
        replay.apply_request(request).unwrap();
        let heads = replay.heads();
        assert!(replay.apply_request(request).is_err());
        assert_eq!(replay.heads(), heads);
    }
    assert_eq!(replay.view().unwrap(), doc.view().unwrap());
    assert_eq!(
        Document::load(&doc.save()).unwrap().view().unwrap(),
        doc.view().unwrap()
    );
}

#[test]
fn block_source_proposals_reject_changed_properties_and_invalid_shapes_atomically() {
    let mut doc = seed();
    let attrs = |source: &str| BTreeMap::from([("markdown".into(), serde_json::json!(source))]);
    doc.insert_block(SeedBlock {
        id: "raw".into(),
        kind: "raw_markdown".into(),
        parent: None,
        position: 1,
        attrs: attrs("Original source"),
        text: String::new(),
    })
    .unwrap();
    doc.apply(&[Command::ProposeBlockUpdate {
        id: "source".into(),
        author: "Reviewer".into(),
        block: "raw".into(),
        kind: "raw_markdown".into(),
        attrs: attrs("Proposed source"),
    }])
    .unwrap();
    assert_eq!(doc.block("raw").unwrap().attrs, attrs("Original source"));
    doc.set_block("raw", "raw_markdown", attrs("Agent source"))
        .unwrap();
    let before = doc.heads();
    assert!(
        doc.apply(&[Command::AcceptProposal {
            id: "source".into()
        }])
        .is_err()
    );
    assert_eq!(doc.heads(), before);
    assert_eq!(doc.block("raw").unwrap().attrs, attrs("Agent source"));
    assert!(doc.view().unwrap().proposals[0].acceptance_error.is_some());
    assert!(
        doc.apply(&[Command::ProposeBlockUpdate {
            id: "invalid".into(),
            author: "Reviewer".into(),
            block: "a".into(),
            kind: "code_line".into(),
            attrs: BTreeMap::new()
        }])
        .is_err()
    );
    assert_eq!(doc.heads(), before);
    doc.set_block("raw", "raw_markdown", attrs("Original source"))
        .unwrap();
    doc.apply(&[Command::AcceptProposal {
        id: "source".into(),
    }])
    .unwrap();
    assert_eq!(doc.block("raw").unwrap().attrs, attrs("Proposed source"));
}

#[test]
fn builder_matches_transport_replay_for_dependent_commands_and_retains_atomicity() {
    let base = seed();
    let mut builder = base
        .command_builder("built", "2026-09-05T00:00:00Z")
        .unwrap();
    builder
        .push(vec![Command::InsertBlock {
            block: SeedBlock {
                id: "b".into(),
                kind: "p".into(),
                parent: None,
                position: 1,
                attrs: Default::default(),
                text: "NEW 😀 text".into(),
            },
        }])
        .unwrap();
    let ranges = builder.selection("b", 4, 6).unwrap();
    builder
        .push(vec![Command::AddComment {
            id: "c".into(),
            author: "agent".into(),
            body: "keep".into(),
            ranges,
        }])
        .unwrap();
    let at = builder.point("b", 7).unwrap();
    builder
        .push(vec![Command::SplitBlock {
            block: "b".into(),
            at,
            new_block: "tail".into(),
        }])
        .unwrap();
    let (built, request) = builder.finish().unwrap();
    let mut replay = Document::load(&base.save()).unwrap();
    replay.apply_request(&request).unwrap();
    assert_eq!(built.view().unwrap(), replay.view().unwrap());
    assert_eq!(built.heads(), replay.heads());
    assert_eq!(
        built.comment_target("c").unwrap().attachments[0].quote,
        "😀"
    );

    let mut failed = base.command_builder("failed", "").unwrap();
    failed
        .push(vec![Command::InsertText {
            at: failed.point("a", 0).unwrap(),
            text: "discard".into(),
        }])
        .unwrap();
    assert!(
        failed
            .push(vec![Command::DeleteBlock {
                block: "missing".into()
            }])
            .is_err()
    );
    // Catching an operation error does not make a partial draft publishable.
    assert!(failed.finish().is_err());
    assert_eq!(base.block_view("a").unwrap().text, "😀 TARGET");
}

#[test]
fn client_and_authority_generate_identical_identity_for_dependent_pending_requests() {
    let mut client = seed();
    let mut authority = Document::load(&client.save()).unwrap();
    let create = request(
        &client,
        "create",
        vec![Command::InsertBlock {
            block: SeedBlock {
                id: "b".into(),
                kind: "p".into(),
                parent: None,
                position: 1,
                attrs: BTreeMap::new(),
                text: "New TARGET".into(),
            },
        }],
    );
    client.apply_request(&create).unwrap();
    let split = request(
        &client,
        "split",
        vec![Command::SplitBlock {
            block: "b".into(),
            at: client.point("b", 4).unwrap(),
            new_block: "tail".into(),
        }],
    );
    client.apply_request(&split).unwrap();
    let type_and_comment = request(
        &client,
        "type",
        vec![
            Command::AddComment {
                id: "c".into(),
                author: "user".into(),
                body: "keep".into(),
                ranges: client.selection("tail", 0, 6).unwrap(),
            },
            Command::InsertText {
                at: client.point("tail", 3).unwrap(),
                text: "!".into(),
            },
        ],
    );
    client.apply_request(&type_and_comment).unwrap();
    for r in [&create, &split, &type_and_comment] {
        authority.apply_request(r).unwrap();
    }
    assert_eq!(client.heads(), authority.heads());
    assert_eq!(client.view().unwrap(), authority.view().unwrap());
    assert_eq!(authority.block_view("tail").unwrap().text, "TAR!GET");
    // Reject reuse even when the actor is no longer the latest document head.
    let heads = authority.heads();
    assert!(authority.apply_request(&type_and_comment).is_err());
    assert_eq!(authority.heads(), heads);
}

#[test]
fn delayed_comment_uses_its_original_base_and_excludes_concurrent_boundary_typing() {
    let base = seed();
    let comment = request(
        &base,
        "comment",
        vec![Command::AddComment {
            id: "c".into(),
            author: "user".into(),
            body: "keep".into(),
            ranges: base.selection("a", 3, 9).unwrap(),
        }],
    );
    let typing = request(
        &base,
        "typing",
        vec![Command::InsertText {
            at: base.point("a", 9).unwrap(),
            text: " outside".into(),
        }],
    );
    let mut left = base.fork();
    let mut right = base.fork();
    left.apply_request(&comment).unwrap();
    left.apply_request(&typing).unwrap();
    right.apply_request(&typing).unwrap();
    right.apply_request(&comment).unwrap();
    assert_eq!(left.heads(), right.heads());
    assert_eq!(left.view().unwrap(), right.view().unwrap());
    assert_eq!(
        left.comment_target("c").unwrap().attachments[0].quote,
        "TARGET"
    );
}

#[test]
fn batch_failure_stale_structure_and_foreign_merge_leave_authority_unchanged() {
    let mut doc = seed();
    let before = doc.heads();
    let invalid = request(
        &doc,
        "invalid",
        vec![
            Command::InsertText {
                at: doc.point("a", 3).unwrap(),
                text: "bad".into(),
            },
            Command::MoveBlock {
                block: "a".into(),
                parent: Some("a".into()),
                before: None,
            },
        ],
    );
    assert!(doc.apply_request(&invalid).is_err());
    assert_eq!(doc.heads(), before);
    let stale = request(
        &doc,
        "stale",
        vec![Command::DeleteBlock { block: "a".into() }],
    );
    let typing = request(
        &doc,
        "typing",
        vec![Command::InsertText {
            at: doc.point("a", 3).unwrap(),
            text: "good".into(),
        }],
    );
    doc.apply_request(&typing).unwrap();
    let split = doc.point("a", 3).unwrap();
    doc.split_block("a", &split, "tail").unwrap();
    let current = doc.heads();
    assert!(doc.apply_request(&stale).is_err());
    assert_eq!(doc.heads(), current);
    assert!(doc.merge(&Document::with_id("another").unwrap()).is_err());
    assert_eq!(doc.heads(), current);
}

#[test]
fn stale_split_accepts_intervening_typing_but_replacement_checks_current_text() {
    let base = seed();
    let mut browser = base.fork();
    let mut server = base.fork();
    let typing = request(
        &base,
        "typing",
        vec![Command::InsertText {
            at: base.point("a", 6).unwrap(),
            text: "!".into(),
        }],
    );
    let split = request(
        &base,
        "split",
        vec![Command::SplitBlock {
            block: "a".into(),
            at: base.point("a", 3).unwrap(),
            new_block: "tail".into(),
        }],
    );
    browser.apply_request(&split).unwrap();
    server.apply_request(&typing).unwrap();
    server.apply_request(&split).unwrap();
    browser.apply_request(&typing).unwrap();
    assert_eq!(browser.view().unwrap(), server.view().unwrap());
    assert_eq!(server.block_view("tail").unwrap().text, "TAR!GET");
    let mut proposed = seed();
    let at = proposed.point("a", 3).unwrap();
    let ranges = proposed.selection("a", 3, 9).unwrap();
    proposed
        .propose_replacement("p", "agent", "a", &at, &ranges, "NEW")
        .unwrap();
    let accept = request(
        &proposed,
        "accept",
        vec![Command::AcceptProposal { id: "p".into() }],
    );
    let at = proposed.point("a", 6).unwrap();
    proposed.insert_text(&at, "!").unwrap();
    let before = proposed.heads();
    assert!(proposed.apply_request(&accept).is_err());
    assert_eq!(proposed.heads(), before);
}

#[test]
fn saved_selection_follows_join_split_and_move_without_quote_matching() {
    let mut doc = seed();
    let point = doc.point("a", 6).unwrap();
    let split = doc.point("a", 3).unwrap();
    doc.split_block("a", &split, "tail").unwrap();
    doc.move_block("tail", None, Some("a")).unwrap();
    let located = doc.locate_point(&point).unwrap().unwrap();
    assert_eq!(located.owner, TargetOwner::Block("tail".into()));
    assert_eq!(located.offset, 3);
    doc.join_blocks("tail", "a").unwrap();
    assert_eq!(doc.locate_point(&point).unwrap().unwrap(), located);
    doc.delete_block("tail").unwrap();
    assert_eq!(doc.locate_point(&point).unwrap(), None);
}

#[test]
fn formatting_proposal_preserves_characters_after_split_and_unrelated_marks() {
    let mut doc = seed();
    let target = doc.selection("a", 3, 9).unwrap();
    doc.add_comment("original", "Reviewer", "Keep TARGET", &target)
        .unwrap();
    let command = request(
        &doc,
        "format-proposal",
        vec![Command::ProposeFormat {
            id: "format".into(),
            author: "Reviewer".into(),
            ranges: target.clone(),
            name: "bold".into(),
            value: true.into(),
        }],
    );
    let mut replay = Document::load(&doc.save()).unwrap();
    doc.apply_request(&command).unwrap();
    replay.apply_request(&command).unwrap();
    assert_eq!(doc.view().unwrap(), replay.view().unwrap());
    assert_eq!(doc.view().unwrap().blocks[0].text, "😀 TARGET");
    assert!(
        doc.view().unwrap().blocks[0]
            .runs
            .iter()
            .all(|run| !run.marks.contains_key("bold"))
    );
    doc.format(&target, "italic", &true.into()).unwrap();
    doc.split_block("a", &doc.point("a", 6).unwrap(), "tail")
        .unwrap();
    doc.insert_text(&doc.point("a", 0).unwrap(), "prefix ")
        .unwrap();
    doc.validate_proposal_acceptance("format").unwrap();
    let before = doc.heads().iter().map(ToString::to_string).collect();
    doc.accept_proposal("format").unwrap();
    let after = doc.heads().iter().map(ToString::to_string).collect();
    let parts = doc.comment_target("original").unwrap().attachments;
    assert_eq!(
        parts
            .iter()
            .map(|part| part.quote.as_str())
            .collect::<String>(),
        "TARGET"
    );
    assert_eq!(parts.len(), 2);
    for block in doc.view().unwrap().blocks {
        for run in block.runs {
            if run.marks.get("italic") == Some(&true.into()) {
                assert_eq!(run.marks.get("bold"), Some(&true.into()));
            }
        }
    }
    let late = doc.selection("tail", 0, 3).unwrap();
    doc.add_comment("late", "Agent", "Original characters", &late)
        .unwrap();
    doc.apply(&[Command::Revert { before, after }]).unwrap();
    assert_eq!(
        doc.comment_target("late").unwrap().attachments[0].quote,
        "GET"
    );
    assert!(
        doc.view()
            .unwrap()
            .blocks
            .iter()
            .flat_map(|block| &block.runs)
            .all(|run| !run.marks.contains_key("bold"))
    );
    assert_eq!(
        Document::load(&doc.save()).unwrap().view().unwrap(),
        doc.view().unwrap()
    );
}

#[test]
fn formatting_proposals_reject_changed_marks_deleted_and_hidden_targets_atomically() {
    for change in ["mark", "delete", "hidden"] {
        let mut doc = seed();
        let ranges = doc.selection("a", 3, 9).unwrap();
        doc.propose_format("f", "Reviewer", &ranges, "bold", &true.into())
            .unwrap();
        match change {
            "mark" => doc.format(&ranges, "bold", &true.into()).unwrap(),
            "delete" => doc.delete_text(&doc.selection("a", 4, 5).unwrap()).unwrap(),
            _ => {
                doc.split_block("a", &doc.point("a", 6).unwrap(), "tail")
                    .unwrap();
                doc.delete_block("tail").unwrap();
            }
        }
        let before = doc.save();
        assert!(doc.accept_proposal("f").is_err(), "{change}");
        assert_eq!(doc.save(), before);
        doc.reject_proposal("f").unwrap();
        assert_eq!(
            doc.proposal("f").unwrap().state,
            quarry_document::ProposalState::Rejected
        );
    }
}
