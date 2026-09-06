#![allow(clippy::unwrap_used, reason = "tests use explicit review fixtures")]
use quarry_document::{
    Comment, DiscussionState, Document, ReviewMetadata, SeedBlock, TargetOwner, TargetState,
};

fn block(id: &str, text: &str, position: usize) -> SeedBlock {
    SeedBlock {
        id: id.into(),
        kind: "p".into(),
        parent: None,
        position,
        attrs: Default::default(),
        text: text.into(),
    }
}
fn seed() -> Document {
    Document::from_blocks(&[
        block("a", "Before TARGET after.", 0),
        block("b", "Other", 1),
    ])
    .unwrap()
}

#[test]
fn extending_deletion_preserves_identity_comments_unicode_and_undo() {
    let mut d = Document::from_blocks(&[block("a", "😀TARGET tail", 0)]).unwrap();
    d.propose_replacement(
        "remove",
        "Reviewer",
        "a",
        &d.point("a", 5).unwrap(),
        &d.selection("a", 5, 8).unwrap(),
        "",
    )
    .unwrap();
    d.add_comment(
        "c",
        "Agent",
        "Keep discussion",
        &d.selection("a", 2, 8).unwrap(),
    )
    .unwrap();
    d.reply_comment("reply", "remove", "Agent", "Why remove?")
        .unwrap();
    let before = d.heads();
    d.continue_text_proposal(
        "remove",
        "Reviewer",
        &d.point("a", 0).unwrap(),
        &d.selection("a", 0, 5).unwrap(),
        "",
    )
    .unwrap();
    let after = d.heads();
    assert_eq!(d.block_view("a").unwrap().text, "😀TARGET tail");
    assert_eq!(
        d.proposal_target("remove").unwrap().attachments[0].quote,
        "😀TARGET"
    );
    assert_eq!(d.proposals().unwrap().len(), 1);
    let mut loaded = Document::load(&d.save()).unwrap();
    loaded.accept_proposal("remove").unwrap();
    assert_eq!(loaded.block_view("a").unwrap().text, " tail");
    assert_eq!(
        loaded.comment("reply").unwrap().parent_id.as_deref(),
        Some("remove")
    );
    d.revert(&before, &after).unwrap();
    assert_eq!(
        d.proposal_target("remove").unwrap().attachments[0].quote,
        "GET"
    );
    assert_eq!(
        d.comment_target("c").unwrap().attachments[0].quote,
        "TARGET"
    );
}

#[test]
fn deletion_extension_rejects_nonadjacent_overlap_other_author_and_decided_proposals_atomically() {
    let mut d = seed();
    d.propose_replacement(
        "remove",
        "Reviewer",
        "a",
        &d.point("a", 10).unwrap(),
        &d.selection("a", 10, 13).unwrap(),
        "",
    )
    .unwrap();
    for (author, ranges) in [
        ("Other", d.selection("a", 7, 10).unwrap()),
        ("Reviewer", d.selection("a", 0, 3).unwrap()),
        ("Reviewer", d.selection("a", 9, 11).unwrap()),
        ("Reviewer", d.selection("b", 0, 3).unwrap()),
        ("Reviewer", vec![]),
    ] {
        let before = d.heads();
        assert!(
            d.continue_text_proposal("remove", author, &d.point("a", 7).unwrap(), &ranges, "")
                .is_err()
        );
        assert_eq!(d.heads(), before);
    }
    d.reject_proposal("remove").unwrap();
    let before = d.heads();
    assert!(
        d.continue_text_proposal(
            "remove",
            "Reviewer",
            &d.point("a", 7).unwrap(),
            &d.selection("a", 7, 10).unwrap(),
            ""
        )
        .is_err()
    );
    assert_eq!(d.heads(), before);
}

#[test]
fn delayed_deletion_extension_keeps_concurrent_prefix_and_comment_but_rejects_changed_targets() {
    use quarry_document::Command;
    let mut base = seed();
    base.propose_replacement(
        "remove",
        "Reviewer",
        "a",
        &base.point("a", 10).unwrap(),
        &base.selection("a", 10, 13).unwrap(),
        "",
    )
    .unwrap();
    let heads = base.heads();
    let command = Command::ContinueTextProposal {
        at: base.point("a", 7).unwrap(),
        text: String::new(),
        id: "remove".into(),
        author: "Reviewer".into(),
        ranges: base.selection("a", 7, 10).unwrap(),
    };
    let mut d = base.fork();
    d.insert_text(&d.point("a", 0).unwrap(), "Before TARGET ")
        .unwrap();
    d.add_comment(
        "late",
        "Agent",
        "Original",
        &d.selection("a", 21, 27).unwrap(),
    )
    .unwrap();
    d.propose_insertion(
        "unrelated",
        "Agent",
        "b",
        &d.point("b", 0).unwrap(),
        "Review ",
    )
    .unwrap();
    d.apply_at(&heads, std::slice::from_ref(&command)).unwrap();
    assert_eq!(
        d.proposal_target("remove").unwrap().attachments[0].start,
        21
    );
    d.accept_proposal("remove").unwrap();
    assert_eq!(
        d.block_view("a").unwrap().text,
        "Before TARGET Before  after."
    );
    assert_eq!(d.comment("late").unwrap().original_quote, "TARGET");
    for changed in ["text", "addition", "decision", "extension"] {
        let mut d = base.fork();
        match changed {
            "text" => d.insert_text(&d.point("a", 11).unwrap(), "NEW").unwrap(),
            "addition" => d.insert_text(&d.point("a", 8).unwrap(), "NEW").unwrap(),
            "decision" => d.reject_proposal("remove").unwrap(),
            _ => d
                .continue_text_proposal(
                    "remove",
                    "Reviewer",
                    &d.point("a", 13).unwrap(),
                    &d.selection("a", 13, 14).unwrap(),
                    "",
                )
                .unwrap(),
        }
        let before = d.heads();
        assert!(d.apply_at(&heads, std::slice::from_ref(&command)).is_err());
        assert_eq!(d.heads(), before);
    }
}

#[test]
fn proposed_split_join_preserve_unicode_targets_and_exact_selection_owners() {
    let mut d = seed();
    d.propose_blocks(
        "tree",
        "Agent",
        None,
        None,
        &[
            block("one", "😀TARGET", 0),
            block("duplicate", "😀TARGET", 1),
        ],
    )
    .unwrap();
    let canonical = d.view().unwrap().blocks;
    let source = d.proposed_blocks_view("tree").unwrap()[0].block.segments[0]
        .source
        .clone();
    let target = d.proposed_block_selection("tree", "one", 2, 8).unwrap();
    d.add_comment("keep", "Reader", "Exact characters", &target)
        .unwrap();
    let saved = d.proposed_block_point("tree", "one", 6).unwrap();
    let before = d.heads();
    d.split_proposed_block(
        "tree",
        "one",
        &d.proposed_block_point("tree", "one", 5).unwrap(),
        "two",
    )
    .unwrap();
    let after = d.heads();
    let proposed = d.proposed_blocks_view("tree").unwrap();
    assert_eq!(
        proposed.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(),
        ["😀TAR", "GET", "😀TARGET"]
    );
    assert_eq!(proposed[0].block.segments[0].source, source);
    assert_eq!(proposed[1].block.segments[0].source, source);
    let located = d.locate_point(&saved).unwrap().unwrap();
    assert_eq!(located.offset, 6);
    assert_eq!(
        located.proposed_block.unwrap(),
        quarry_document::ResolvedProposedPoint {
            block: "two".into(),
            offset: 1
        }
    );
    let right_start = d.proposed_block_point("tree", "two", 0).unwrap();
    assert_eq!(
        d.locate_point(&right_start)
            .unwrap()
            .unwrap()
            .proposed_block
            .unwrap()
            .block,
        "two"
    );
    assert_eq!(
        d.comment_target("keep")
            .unwrap()
            .attachments
            .iter()
            .map(|part| part.quote.as_str())
            .collect::<String>(),
        "TARGET"
    );
    assert_eq!(d.view().unwrap().blocks, canonical);
    d.insert_text(&d.proposed_block_point("tree", "two", 0).unwrap(), "Late ")
        .unwrap();
    d.revert(&before, &after).unwrap();
    assert_eq!(
        d.proposed_blocks_view("tree").unwrap()[0].text,
        "😀TARLate GET"
    );
    assert_eq!(
        d.comment_target("keep")
            .unwrap()
            .attachments
            .iter()
            .map(|part| part.quote.as_str())
            .collect::<String>(),
        "TARLate GET"
    );
    assert_eq!(
        d.view().unwrap().comments[0].comment.original_quote,
        "TARGET"
    );
    d.split_proposed_block(
        "tree",
        "one",
        &d.proposed_block_point("tree", "one", 5).unwrap(),
        "again",
    )
    .unwrap();
    d.join_proposed_blocks("tree", "one", "again").unwrap();
    assert_eq!(
        d.proposed_blocks_view("tree").unwrap()[0].text,
        "😀TARLate GET"
    );
    assert_eq!(d.view().unwrap().blocks, canonical);
    let mut restored = Document::load(&d.save()).unwrap();
    restored.accept_proposal("tree").unwrap();
    assert_eq!(
        restored
            .view()
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.block.id == "one")
            .unwrap()
            .text,
        "😀TARLate GET"
    );
    assert!(
        restored
            .comment_target("keep")
            .unwrap()
            .attachments
            .iter()
            .all(|part| part.owner == TargetOwner::Block("one".into()))
    );
    assert_eq!(
        restored
            .view()
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.block.id == "duplicate")
            .unwrap()
            .text,
        "😀TARGET"
    );
}

#[test]
fn invalid_proposed_splits_and_joins_do_not_change_history_or_targets() {
    let mut d = seed();
    d.propose_blocks(
        "tree",
        "Agent",
        None,
        None,
        &[
            block("one", "😀TARGET", 0),
            block("two", "Other", 1),
            block("three", "Last", 2),
        ],
    )
    .unwrap();
    let at = d.proposed_block_point("tree", "one", 2).unwrap();
    let foreign = d.point("a", 1).unwrap();
    for (name, point) in [
        ("one", at.clone()),
        ("two", at.clone()),
        ("a", at.clone()),
        ("new", foreign),
    ] {
        let heads = d.heads();
        let view = d.view().unwrap();
        assert!(d.split_proposed_block("tree", "one", &point, name).is_err());
        assert_eq!(d.heads(), heads);
        assert_eq!(d.view().unwrap(), view);
    }
    for (left, right) in [
        ("one", "one"),
        ("two", "one"),
        ("one", "three"),
        ("one", "missing"),
    ] {
        let heads = d.heads();
        let view = d.view().unwrap();
        assert!(d.join_proposed_blocks("tree", left, right).is_err());
        assert_eq!(d.heads(), heads);
        assert_eq!(d.view().unwrap(), view);
    }
    d.reject_proposal("tree").unwrap();
    let heads = d.heads();
    assert!(d.split_proposed_block("tree", "one", &at, "new").is_err());
    assert!(d.join_proposed_blocks("tree", "one", "two").is_err());
    assert_eq!(d.heads(), heads);
}

#[test]
fn block_proposal_can_be_accepted_again_after_undo_without_reusing_unrelated_identity() {
    let mut d = seed();
    d.propose_blocks(
        "structure",
        "Agent",
        None,
        Some("b".into()),
        &[block("new", "TARGET", 0)],
    )
    .unwrap();
    let ranges = d
        .proposed_block_selection("structure", "new", 0, 6)
        .unwrap();
    let before = d.heads();
    d.accept_proposal("structure").unwrap();
    let after = d.heads();
    d.add_comment("late", "Reviewer", "Keep", &ranges).unwrap();
    d.revert(&before, &after).unwrap();
    assert!(d.validate_proposal_acceptance("structure").is_ok());
    assert_eq!(
        d.view().unwrap().comments[0].target.attachments[0].owner,
        TargetOwner::Proposal("structure".into())
    );
    let mut restored = Document::load(&d.save()).unwrap();
    restored.accept_proposal("structure").unwrap();
    assert_eq!(
        restored.block("new").unwrap().segments,
        d.block("new").unwrap().segments
    );
    assert_eq!(
        restored.view().unwrap().comments[0].target.attachments[0].owner,
        TargetOwner::Block("new".into())
    );

    for deleted in [false, true] {
        let mut collision = seed();
        collision
            .propose_blocks(
                "other",
                "Agent",
                None,
                None,
                &[block("collision", "TARGET", 0)],
            )
            .unwrap();
        collision
            .insert_block(block("collision", "TARGET", 2))
            .unwrap();
        if deleted {
            collision.delete_block("collision").unwrap();
        }
        let before = collision.heads();
        assert!(collision.accept_proposal("other").is_err());
        assert_eq!(collision.heads(), before);
    }
}

#[test]
fn acceptance_readiness_is_derived_without_invalidating_historical_proposals() {
    let mut d = seed();
    d.insert_block(SeedBlock {
        kind: "table".into(),
        text: String::new(),
        ..block("table", "", 2)
    })
    .unwrap();
    let row = SeedBlock {
        kind: "tr".into(),
        text: String::new(),
        ..block("row", "", 0)
    };
    d.propose_blocks("structure", "agent", Some("table".into()), None, &[row])
        .unwrap();
    assert!(d.validate_proposal_acceptance("structure").is_ok());
    d.set_block("table", "p", Default::default()).unwrap();
    let before = d.view().unwrap();
    assert!(before.proposals[0].acceptance_error.is_some());
    assert!(d.accept_proposal("structure").is_err());
    assert_eq!(d.view().unwrap(), before);
    d.reject_proposal("structure").unwrap();
    d.delete_block("table").unwrap();
    assert!(Document::load(&d.save()).is_ok());

    d.propose_block_delete("delete", "agent", "a").unwrap();
    let ranges = d.selection("a", 7, 13).unwrap();
    d.add_comment("c", "user", "discuss", &ranges).unwrap();
    assert!(d.validate_proposal_acceptance("delete").is_ok());
    d.insert_text(&d.point("a", 8).unwrap(), "new").unwrap();
    let before = d.heads();
    assert!(d.validate_proposal_acceptance("delete").is_err());
    assert!(d.accept_proposal("delete").is_err());
    assert_eq!(d.heads(), before);
}

#[test]
fn invalid_proposed_structure_rolls_back_sources_and_can_be_retried() {
    let mut d = seed();
    let before = d.view().unwrap();
    let mut first = block("new-a", "A", 0);
    first.parent = Some("new-b".into());
    let mut second = block("new-b", "B", 0);
    second.parent = Some("new-a".into());
    assert!(
        d.propose_blocks("proposal", "Agent", None, None, &[first, second])
            .is_err()
    );
    assert_eq!(d.view().unwrap(), before);
    d.propose_blocks(
        "proposal",
        "Agent",
        None,
        None,
        &[block("new-a", "Valid", 0)],
    )
    .unwrap();
    d.accept_proposal("proposal").unwrap();
    assert_eq!(d.block_view("new-a").unwrap().text, "Valid");
}

#[test]
fn replacement_preserves_comments_on_proposed_text_and_discussion_on_deleted_text() {
    let mut d = seed();
    let at = d.point("a", 7).unwrap();
    let ranges = d.selection("a", 7, 13).unwrap();
    d.add_comment("old", "user", "About the original", &ranges)
        .unwrap();
    d.propose_replacement("p", "agent", "a", &at, &ranges, "NEW")
        .unwrap();
    let range = d.proposal_selection("p", 0, 3).unwrap();
    d.add_comment("new", "user", "About the proposal", &range)
        .unwrap();
    d.accept_proposal("p").unwrap();
    assert_eq!(d.block_view("a").unwrap().text, "Before NEW after.");
    assert_eq!(
        d.comment_target("new").unwrap().attachments[0].owner,
        TargetOwner::Block("a".into())
    );
    assert_eq!(d.comment_target("old").unwrap().state, TargetState::Deleted);
    assert_eq!(d.comment("old").unwrap().state, DiscussionState::Open);
}

#[test]
fn replacement_refuses_to_delete_edits_that_arrived_after_the_proposal() {
    let mut d = seed();
    let at = d.point("a", 7).unwrap();
    let ranges = d.selection("a", 7, 13).unwrap();
    d.propose_replacement("p", "agent", "a", &at, &ranges, "NEW")
        .unwrap();
    let at = d.point("a", 10).unwrap();
    d.insert_text(&at, "!").unwrap();
    let before = d.heads();
    assert!(d.accept_proposal("p").is_err());
    assert_eq!(d.heads(), before);
    assert_eq!(d.block_view("a").unwrap().text, "Before TAR!GET after.");
}

#[test]
fn replacement_spans_blocks_and_follows_characters_after_split_and_move() {
    let mut d = seed();
    d.insert_block(block("repeat", "TARGETOther", 2)).unwrap();
    let mut ranges = d.selection("a", 7, 13).unwrap();
    ranges.extend(d.selection("b", 0, 5).unwrap());
    d.add_comment("old", "Reviewer", "Original characters", &ranges)
        .unwrap();
    d.propose_replacement(
        "replacement",
        "Reviewer",
        "a",
        &d.point("a", 7).unwrap(),
        &ranges,
        "NEW",
    )
    .unwrap();
    let proposed = d.proposal_selection("replacement", 0, 3).unwrap();
    d.add_comment("new", "Reviewer", "New characters", &proposed)
        .unwrap();
    d.split_block("b", &d.point("b", 2).unwrap(), "tail")
        .unwrap();
    d.move_block("tail", None, Some("a")).unwrap();
    d.validate_proposal_acceptance("replacement").unwrap();
    let before = d.heads();
    d.accept_proposal("replacement").unwrap();
    let after = d.heads();
    assert_eq!(d.block_view("a").unwrap().text, "Before NEW after.");
    assert_eq!(d.block_view("b").unwrap().text, "");
    assert_eq!(d.block_view("tail").unwrap().text, "");
    assert_eq!(d.block_view("repeat").unwrap().text, "TARGETOther");
    assert_eq!(d.comment_target("old").unwrap().state, TargetState::Deleted);
    assert_eq!(
        d.comment_target("new").unwrap().attachments[0].owner,
        TargetOwner::Block("a".into())
    );
    d.revert(&before, &after).unwrap();
    assert_eq!(
        d.comment_target("new").unwrap().attachments[0].owner,
        TargetOwner::Proposal("replacement".into())
    );
    let mut reloaded = Document::load(&d.save()).unwrap();
    reloaded.accept_proposal("replacement").unwrap();
    assert_eq!(reloaded.block_view("a").unwrap().text, "Before NEW after.");
}

#[test]
fn cross_block_replacement_refuses_changed_or_partly_hidden_targets_atomically() {
    for change in ["typing", "text deletion", "block deletion"] {
        let mut d = seed();
        let mut ranges = d.selection("a", 7, 13).unwrap();
        ranges.extend(d.selection("b", 0, 5).unwrap());
        d.propose_replacement(
            "replacement",
            "Reviewer",
            "a",
            &d.point("a", 7).unwrap(),
            &ranges,
            "NEW",
        )
        .unwrap();
        match change {
            "typing" => d.insert_text(&d.point("b", 2).unwrap(), "new").unwrap(),
            "text deletion" => d.delete_text(&d.selection("b", 1, 3).unwrap()).unwrap(),
            _ => d.delete_block("b").unwrap(),
        }
        let before = d.heads();
        assert!(d.accept_proposal("replacement").is_err(), "{change}");
        assert_eq!(d.heads(), before, "{change}");
    }
}

#[test]
fn insertion_follows_its_position_when_a_block_is_split_and_moved() {
    let mut d = seed();
    let at = d.point("a", 7).unwrap();
    d.propose_insertion("p", "agent", "a", &at, "NEW ").unwrap();
    let split = d.point("a", 4).unwrap();
    d.split_block("a", &split, "tail").unwrap();
    d.move_block("tail", None, None).unwrap();
    d.accept_proposal("p").unwrap();
    assert_eq!(d.block_view("tail").unwrap().text, "re NEW TARGET after.");
}

#[test]
fn proposed_blocks_keep_delayed_comments_after_acceptance_without_copying_text() {
    let mut base = seed();
    base.propose_blocks(
        "p",
        "agent",
        None,
        Some("b".into()),
        &[block("new1", "FIRST", 0), block("new2", "SECOND", 1)],
    )
    .unwrap();
    let mut reviewer = base.fork();
    let mut authority = base.fork();
    authority.accept_proposal("p").unwrap();
    let ranges = reviewer.proposal_selection("p", 5, 11).unwrap();
    reviewer.add_comment("c", "user", "Keep", &ranges).unwrap();
    authority.merge(&reviewer).unwrap();
    assert_eq!(
        authority.comment_target("c").unwrap().attachments[0].owner,
        TargetOwner::Block("new2".into())
    );
    assert_eq!(
        authority
            .blocks()
            .unwrap()
            .iter()
            .map(|b| b.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "new1", "new2", "b"]
    );
}

#[test]
fn proposed_subtree_deletion_ignores_new_comments_and_moves_but_refuses_new_text() {
    let mut d = seed();
    d.propose_block_delete("p", "agent", "a").unwrap();
    let ranges = d.selection("a", 7, 13).unwrap();
    d.add_comment("c", "user", "Keep this discussion", &ranges)
        .unwrap();
    d.move_block("a", None, None).unwrap();
    let mut changed = d.fork();
    let at = changed.point("a", 7).unwrap();
    changed.insert_text(&at, "NEW ").unwrap();
    assert!(changed.accept_proposal("p").is_err());
    d.accept_proposal("p").unwrap();
    assert_eq!(d.comment_target("c").unwrap().state, TargetState::Hidden);
    assert_eq!(d.comments().unwrap().len(), 1);
}

#[test]
fn replies_resolution_and_preexisting_unattached_records_survive_reload() {
    let mut d = seed();
    let range = d.selection("a", 7, 13).unwrap();
    d.add_comment("c", "user", "Root", &range).unwrap();
    d.reply_comment("r", "c", "agent", "Reply").unwrap();
    d.edit_comment("r", "Updated").unwrap();
    d.resolve_comment("c", true).unwrap();
    d.import_unattached_comment(Comment {
        id: "imported".into(),
        author: "old".into(),
        body: "Preserved".into(),
        target: Vec::new(),
        original_quote: "Lost before migration".into(),
        state: DiscussionState::Open,
        parent_id: None,
        deleted: false,
        metadata: ReviewMetadata {
            unattached_reason: Some("Legacy anchor was orphaned".into()),
            ..Default::default()
        },
    })
    .unwrap();
    let mut restored = Document::load(&d.save()).unwrap();
    assert_eq!(restored.view().unwrap(), d.view().unwrap());
    assert_eq!(
        restored.comment_target("imported").unwrap().state,
        TargetState::Unattached
    );
    restored.delete_comment("c").unwrap();
    assert_eq!(
        restored
            .comments()
            .unwrap()
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["imported"]
    );
}

#[test]
fn a_proposed_move_keeps_text_identity_and_accepts_later_text_edits_but_refuses_changed_placement()
{
    let mut d = seed();
    d.insert_block(block("c", "TARGET again", 2)).unwrap();
    d.add_comment("keep", "Reader", "Keep", &d.selection("a", 7, 13).unwrap())
        .unwrap();
    let canonical = d.view().unwrap().blocks;
    d.apply(&[quarry_document::Command::ProposeBlockMove {
        id: "move".into(),
        author: "Reviewer".into(),
        block: "a".into(),
        parent: None,
        before: None,
    }])
    .unwrap();
    assert_eq!(d.view().unwrap().blocks, canonical);
    d.insert_text(&d.point("a", 0).unwrap(), "Agent ").unwrap();
    let before = d.heads();
    d.accept_proposal("move").unwrap();
    let after = d.heads();
    assert_eq!(
        d.blocks()
            .unwrap()
            .iter()
            .map(|b| b.id.as_str())
            .collect::<Vec<_>>(),
        ["b", "c", "a"]
    );
    let comment = d.comment_target("keep").unwrap();
    assert_eq!(comment.attachments[0].owner, TargetOwner::Block("a".into()));
    assert_eq!(comment.attachments[0].quote, "TARGET");
    d.revert(&before, &after).unwrap();
    assert_eq!(
        d.blocks()
            .unwrap()
            .iter()
            .map(|b| b.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    let mut restored = Document::load(&d.save()).unwrap();
    restored.accept_proposal("move").unwrap();
    assert_eq!(restored.comment_target("keep").unwrap(), comment);
    d.move_block("a", None, Some("c")).unwrap();
    let state = d.view().unwrap();
    assert!(d.accept_proposal("move").is_err());
    assert_eq!(d.view().unwrap(), state);
}

#[test]
fn proposed_moves_validate_destination_and_reject_atomically_when_it_disappears() {
    let mut d = seed();
    d.insert_block(block("c", "Destination", 2)).unwrap();
    let before = d.heads();
    assert!(
        d.propose_block_move("invalid", "Reviewer", "a", Some("b".into()), None)
            .is_err()
    );
    assert_eq!(d.heads(), before);
    d.propose_block_move("move", "Reviewer", "a", None, Some("c".into()))
        .unwrap();
    d.delete_block("c").unwrap();
    let state = d.view().unwrap();
    assert!(state.proposals[0].acceptance_error.is_some());
    assert!(d.accept_proposal("move").is_err());
    assert_eq!(d.view().unwrap(), state);
    let mut restored = Document::load(&d.save()).unwrap();
    restored.reject_proposal("move").unwrap();
    assert_eq!(
        restored.block_view("a").unwrap().text,
        "Before TARGET after."
    );
}

#[test]
fn proposed_properties_keep_text_ownership_and_validate_before_mutation() {
    let mut d = seed();
    d.propose_blocks(
        "new",
        "Reviewer",
        None,
        None,
        &[
            block("p", "TARGET", 0),
            SeedBlock {
                kind: "mermaid".into(),
                attrs: [("code".into(), "flowchart LR\nA --> B".into())].into(),
                ..block("diagram", "", 1)
            },
        ],
    )
    .unwrap();
    let canonical = d.view().unwrap().blocks;
    let ranges = d.proposed_block_selection("new", "p", 0, 6).unwrap();
    d.add_comment("keep", "Reader", "Keep", &ranges).unwrap();
    d.apply(&[quarry_document::Command::SetProposedBlock {
        proposal: "new".into(),
        block: "p".into(),
        kind: "h2".into(),
        attrs: Default::default(),
    }])
    .unwrap();
    d.set_proposed_block(
        "new",
        "diagram",
        "mermaid",
        [("code".into(), "flowchart LR\nX --> Y".into())].into(),
    )
    .unwrap();
    assert_eq!(d.view().unwrap().blocks, canonical);
    assert_eq!(
        d.comment_target("keep").unwrap().attachments[0].quote,
        "TARGET"
    );
    let state = d.view().unwrap();
    assert!(
        d.set_proposed_block("new", "p", "table", Default::default())
            .is_err()
    );
    assert!(
        d.set_proposed_block("new", "missing", "p", Default::default())
            .is_err()
    );
    assert_eq!(d.view().unwrap(), state);
    let mut restored = Document::load(&d.save()).unwrap();
    restored.accept_proposal("new").unwrap();
    assert_eq!(restored.block("p").unwrap().kind, "h2");
    assert_eq!(
        restored.block("diagram").unwrap().attrs["code"],
        "flowchart LR\nX --> Y"
    );
    assert_eq!(
        restored.comment_target("keep").unwrap().attachments[0].owner,
        TargetOwner::Block("p".into())
    );
    let state = restored.view().unwrap();
    assert!(
        restored
            .set_proposed_block("new", "diagram", "mermaid", Default::default())
            .is_err()
    );
    assert_eq!(restored.view().unwrap(), state);
}

#[test]
fn proposed_structure_retains_sources_through_reorder_removal_acceptance_and_undo() {
    use quarry_document::{Command, ProposedBlockPlacement as Placement};
    let mut d = seed();
    d.propose_blocks(
        "tree",
        "Agent",
        None,
        None,
        &[block("one", "TARGET", 0), block("two", "TARGET", 1)],
    )
    .unwrap();
    let canonical = d.view().unwrap().blocks;
    let sources = d.proposed_blocks_view("tree").unwrap();
    d.add_comment(
        "keep",
        "Reader",
        "Keep first",
        &d.proposed_block_selection("tree", "one", 0, 6).unwrap(),
    )
    .unwrap();
    d.add_comment(
        "removed",
        "Reader",
        "Keep second",
        &d.proposed_block_selection("tree", "two", 0, 6).unwrap(),
    )
    .unwrap();
    let before = d.heads();
    d.apply(&[Command::SetProposedStructure {
        proposal: "tree".into(),
        blocks: vec![
            Placement::New {
                block: block("three", "New TARGET", 0),
            },
            Placement::Existing {
                block: "one".into(),
                parent: None,
                position: 1,
            },
        ],
    }])
    .unwrap();
    let after = d.heads();
    let point = d.proposed_block_point("tree", "one", 0).unwrap();
    d.insert_text(&point, "Agent ").unwrap();
    assert_eq!(d.view().unwrap().blocks, canonical);
    assert_eq!(
        d.proposed_blocks_view("tree").unwrap()[1].block.segments,
        sources[0].block.segments
    );
    assert_eq!(
        d.comment_target("keep").unwrap().attachments[0].quote,
        "TARGET"
    );
    assert_eq!(
        d.comment_target("removed").unwrap().state,
        TargetState::Hidden
    );
    d.revert(&before, &after).unwrap();
    assert_eq!(
        d.proposed_blocks_view("tree").unwrap()[0].text,
        "Agent TARGET"
    );
    assert_eq!(
        d.comment_target("removed").unwrap().state,
        TargetState::Attached
    );
    let mut restored = Document::load(&d.save()).unwrap();
    restored.accept_proposal("tree").unwrap();
    assert_eq!(
        restored.block("one").unwrap().segments,
        sources[0].block.segments
    );
    assert_eq!(
        restored.comment_target("keep").unwrap().attachments[0].owner,
        TargetOwner::Block("one".into())
    );
    assert_eq!(
        restored.comment_target("removed").unwrap().attachments[0].owner,
        TargetOwner::Block("two".into())
    );
    assert!(
        restored
            .set_proposed_structure(
                "tree",
                &[Placement::Existing {
                    block: "one".into(),
                    parent: None,
                    position: 0
                }]
            )
            .is_err()
    );
}

#[test]
fn proposed_structure_rejects_identity_replacement_cycles_and_invalid_schema_atomically() {
    use quarry_document::ProposedBlockPlacement as Placement;
    let mut d = seed();
    d.propose_blocks(
        "tree",
        "Agent",
        None,
        None,
        &[block("one", "TARGET", 0), block("two", "TARGET", 1)],
    )
    .unwrap();
    let existing = |id: &str, parent: Option<&str>, position| Placement::Existing {
        block: id.into(),
        parent: parent.map(Into::into),
        position,
    };
    let invalid = vec![
        vec![],
        vec![Placement::New {
            block: block("one", "copied", 0),
        }],
        vec![existing("missing", None, 0)],
        vec![existing("one", None, 0), existing("one", None, 1)],
        vec![existing("one", None, 0), existing("two", None, 0)],
        vec![existing("one", Some("missing"), 0)],
        vec![
            existing("one", Some("two"), 0),
            existing("two", Some("one"), 0),
        ],
        vec![existing("one", Some("two"), 0), existing("two", None, 0)],
        vec![Placement::New {
            block: block("a", "canonical ID", 0),
        }],
    ];
    for placements in invalid {
        let state = d.view().unwrap();
        assert!(
            d.set_proposed_structure("tree", &placements).is_err(),
            "{placements:?}"
        );
        assert_eq!(d.view().unwrap(), state);
    }
}

#[test]
fn rejected_proposal_comments_and_selections_are_hidden_until_undo() {
    let mut d = seed();
    d.propose_insertion("p", "Agent", "a", &d.point("a", 0).unwrap(), "TARGET")
        .unwrap();
    let point = d.proposal_point("p", 2).unwrap();
    d.add_comment(
        "keep",
        "Reader",
        "Keep",
        &d.proposal_selection("p", 0, 6).unwrap(),
    )
    .unwrap();
    let before = d.heads();
    d.reject_proposal("p").unwrap();
    let after = d.heads();
    let mut restored = Document::load(&d.save()).unwrap();
    assert_eq!(
        restored.comment_target("keep").unwrap().state,
        TargetState::Hidden
    );
    assert!(restored.locate_point(&point).unwrap().is_none());
    restored.revert(&before, &after).unwrap();
    assert_eq!(
        restored.comment_target("keep").unwrap().attachments[0].quote,
        "TARGET"
    );
    assert_eq!(restored.locate_point(&point).unwrap().unwrap().offset, 2);
}
