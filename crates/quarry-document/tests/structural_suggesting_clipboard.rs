#![allow(
    clippy::unwrap_used,
    reason = "tests use explicit structural and clipboard fixtures"
)]

use quarry_document::{
    Command, CommandRequest, Document, EditAction, EditMode, ProposalAction, ProposalState,
    SeedBlock, TargetOwner, TargetState,
};

fn seed(id: &str, position: usize, text: &str) -> SeedBlock {
    SeedBlock {
        id: id.into(),
        kind: "p".into(),
        attrs: Default::default(),
        parent: None,
        position,
        text: text.into(),
    }
}

fn edit(mode: EditMode, action: EditAction) -> Command {
    Command::Edit { mode, action }
}

fn direct(action: EditAction) -> Command {
    edit(EditMode::Direct, action)
}

fn suggest(id: &str, action: EditAction) -> Command {
    edit(
        EditMode::Suggest {
            id: id.into(),
            author: "Reviewer".into(),
        },
        action,
    )
}

fn request(document: &Document, id: &str, commands: Vec<Command>) -> CommandRequest {
    CommandRequest {
        request_id: id.into(),
        base: document.heads().iter().map(ToString::to_string).collect(),
        commands,
        at: String::new(),
    }
}

fn reload(document: &Document) -> Document {
    let loaded = Document::load(&document.save()).unwrap();
    assert_eq!(loaded.view().unwrap(), document.view().unwrap());
    loaded
}

#[test]
fn suggested_split_is_structural_and_transfers_proposed_and_commented_text_on_acceptance() {
    let mut document = Document::from_blocks(&[seed("a", 0, "Before TARGET")]).unwrap();
    document
        .add_comment(
            "comment",
            "Agent",
            "Keep",
            &document.selection("a", 7, 13).unwrap(),
        )
        .unwrap();
    let split_at = document.point("a", 7).unwrap();
    document
        .apply(&[suggest(
            "split",
            EditAction::SplitBlock {
                block: "a".into(),
                proposal: None,
                at: split_at,
                new_block: "tail".into(),
            },
        )])
        .unwrap();

    let view = document.view().unwrap();
    assert_eq!(view.blocks.len(), 1);
    assert_eq!(view.blocks[0].text, "Before TARGET");
    assert_eq!(view.proposals[0].text, "\n");
    assert!(matches!(
        view.proposals[0].proposal.action,
        ProposalAction::SplitBlock { .. }
    ));

    document
        .apply(&[edit(
            EditMode::Continue {
                id: "split".into(),
                author: "Reviewer".into(),
            },
            EditAction::InsertText {
                at: document.proposal_point("split", 1).unwrap(),
                text: "New ".into(),
            },
        )])
        .unwrap();
    document
        .insert_text(&document.point("a", 10).unwrap(), "!")
        .unwrap();
    document.accept_proposal("split").unwrap();

    let view = document.view().unwrap();
    assert_eq!(
        view.blocks
            .iter()
            .map(|block| (block.block.id.as_str(), block.text.as_str()))
            .collect::<Vec<_>>(),
        [("a", "Before "), ("tail", "New TAR!GET")]
    );
    assert_eq!(view.comments[0].target.state, TargetState::Attached);
    assert_eq!(
        view.comments[0].target.attachments[0].owner,
        TargetOwner::Block("tail".into())
    );
    assert_eq!(view.comments[0].target.attachments[0].quote, "TAR!GET");
    reload(&document);
}

#[test]
fn suggested_join_keeps_both_blocks_until_acceptance_and_preserves_late_text_identity() {
    let mut document =
        Document::from_blocks(&[seed("left", 0, "Left "), seed("right", 1, "TARGET right")])
            .unwrap();
    document
        .add_comment(
            "comment",
            "Agent",
            "Keep",
            &document.selection("right", 0, 6).unwrap(),
        )
        .unwrap();
    document
        .apply(&[suggest(
            "join",
            EditAction::JoinBlocks {
                left: "left".into(),
                right: "right".into(),
                proposal: None,
            },
        )])
        .unwrap();
    assert_eq!(document.view().unwrap().blocks.len(), 2);
    assert!(matches!(
        document.proposal("join").unwrap().action,
        ProposalAction::JoinBlocks { .. }
    ));

    document
        .insert_text(&document.point("right", 3).unwrap(), "!")
        .unwrap();
    document.accept_proposal("join").unwrap();
    let view = document.view().unwrap();
    assert_eq!(view.blocks.len(), 1);
    assert_eq!(view.blocks[0].text, "Left TAR!GET right");
    assert_eq!(view.comments[0].target.state, TargetState::Attached);
    assert_eq!(
        view.comments[0].target.attachments[0].owner,
        TargetOwner::Block("left".into())
    );
    assert_eq!(view.comments[0].target.attachments[0].quote, "TAR!GET");
    reload(&document);
}

#[test]
fn rejecting_structural_suggestions_leaves_the_canonical_blocks_unchanged() {
    let mut document =
        Document::from_blocks(&[seed("left", 0, "Left"), seed("right", 1, "Right")]).unwrap();
    document
        .apply(&[suggest(
            "split",
            EditAction::SplitBlock {
                block: "left".into(),
                proposal: None,
                at: document.point("left", 2).unwrap(),
                new_block: "split-tail".into(),
            },
        )])
        .unwrap();
    document.reject_proposal("split").unwrap();
    document
        .apply(&[suggest(
            "join",
            EditAction::JoinBlocks {
                left: "left".into(),
                right: "right".into(),
                proposal: None,
            },
        )])
        .unwrap();
    document.reject_proposal("join").unwrap();

    let view = document.view().unwrap();
    assert_eq!(
        view.blocks
            .iter()
            .map(|block| (block.block.id.as_str(), block.text.as_str()))
            .collect::<Vec<_>>(),
        [("left", "Left"), ("right", "Right")]
    );
    assert_eq!(view.proposals.len(), 2);
    assert!(
        view.proposals
            .iter()
            .all(|proposal| proposal.proposal.state == ProposalState::Rejected)
    );
    reload(&document);
}

#[test]
fn suggested_multi_block_paste_is_atomic_across_a_selection_and_late_edits() {
    let mut document = Document::from_blocks(&[
        seed("left", 0, "Left TARGET"),
        seed("middle", 1, "Remove block"),
        seed("right", 2, "Right KEEP"),
    ])
    .unwrap();
    document
        .add_comment(
            "tail",
            "Agent",
            "Keep",
            &document.selection("right", 6, 10).unwrap(),
        )
        .unwrap();
    document
        .apply(&[suggest(
            "paste",
            EditAction::PasteBlocks {
                block: "left".into(),
                at: document.point("left", 5).unwrap(),
                focus: document.point("right", 6).unwrap(),
                blocks: vec![
                    seed("pasted-first", 0, "Copy one"),
                    seed("pasted-last", 1, "Copy two"),
                ],
            },
        )])
        .unwrap();

    let view = document.view().unwrap();
    assert_eq!(
        view.blocks
            .iter()
            .map(|view| view.text.as_str())
            .collect::<Vec<_>>(),
        ["Left TARGET", "Remove block", "Right KEEP"]
    );
    assert_eq!(view.proposals[0].text, "Copy one\nCopy two");
    assert_eq!(document.proposal_view("paste").unwrap(), view.proposals[0]);
    assert_eq!(
        document.proposal_views_for_block("left").unwrap(),
        vec![view.proposals[0].clone()]
    );
    let target = view.proposals[0]
        .target
        .attachments
        .iter()
        .find(|part| part.owner == TargetOwner::Block("left".into()))
        .unwrap();
    assert_eq!(
        document
            .review_markers(&TargetOwner::Block("left".into()))
            .unwrap(),
        vec![(
            "paste_blocks".into(),
            "paste".into(),
            target.start,
            target.end
        )]
    );
    assert!(matches!(
        view.proposals[0].proposal.action,
        ProposalAction::PasteBlocks { .. }
    ));
    assert_eq!(
        view.proposals[0]
            .target
            .attachments
            .iter()
            .map(|part| part.quote.as_str())
            .collect::<String>(),
        "TARGETRight "
    );
    document
        .add_comment(
            "pasted-comment",
            "Reviewer",
            "Keep pasted identity",
            &document.proposal_selection("paste", 9, 17).unwrap(),
        )
        .unwrap();
    assert_eq!(
        document
            .review_markers(&TargetOwner::Proposal("paste".into()))
            .unwrap(),
        vec![("comment".into(), "pasted-comment".into(), 9, 17)]
    );

    document
        .insert_text(&document.point("right", 8).unwrap(), "!")
        .unwrap();
    document.accept_proposal("paste").unwrap();
    let view = document.view().unwrap();
    assert_eq!(
        view.blocks
            .iter()
            .map(|view| (view.block.id.as_str(), view.text.as_str()))
            .collect::<Vec<_>>(),
        [("left", "Left Copy one"), ("pasted-last", "Copy twoKE!EP")]
    );
    let tail = view
        .comments
        .iter()
        .find(|view| view.comment.id == "tail")
        .unwrap();
    let pasted = view
        .comments
        .iter()
        .find(|view| view.comment.id == "pasted-comment")
        .unwrap();
    assert_eq!(tail.target.state, TargetState::Attached);
    assert_eq!(
        tail.target.attachments[0].owner,
        TargetOwner::Block("pasted-last".into())
    );
    assert_eq!(tail.target.attachments[0].quote, "KE!EP");
    assert_eq!(
        pasted.target.attachments[0].owner,
        TargetOwner::Block("pasted-last".into())
    );
    assert_eq!(pasted.target.attachments[0].quote, "Copy two");
    reload(&document);
}

#[test]
fn suggested_multi_block_paste_keeps_every_middle_block_in_clipboard_order() {
    let mut document = Document::from_blocks(&[seed("destination", 0, "Tail")]).unwrap();
    let at = document.point("destination", 0).unwrap();
    document
        .apply(&[suggest(
            "paste",
            EditAction::PasteBlocks {
                block: "destination".into(),
                at: at.clone(),
                focus: at,
                blocks: vec![
                    seed("first", 0, "One"),
                    seed("second", 1, "Two"),
                    seed("third", 2, "Three"),
                    seed("fourth", 3, "Four"),
                ],
            },
        )])
        .unwrap();

    document.accept_proposal("paste").unwrap();
    assert_eq!(
        document
            .view()
            .unwrap()
            .blocks
            .iter()
            .map(|view| (view.block.id.as_str(), view.text.as_str()))
            .collect::<Vec<_>>(),
        [
            ("destination", "One"),
            ("second", "Two"),
            ("third", "Three"),
            ("fourth", "FourTail"),
        ]
    );
    reload(&document);
}

#[test]
fn suggested_paste_rejects_a_changed_whole_block_selection_without_partial_mutation() {
    let mut document = Document::from_blocks(&[
        seed("left", 0, "Left"),
        seed("middle", 1, "Remove"),
        seed("right", 2, "Right"),
    ])
    .unwrap();
    document
        .apply(&[suggest(
            "paste",
            EditAction::PasteBlocks {
                block: "left".into(),
                at: document.point("left", 2).unwrap(),
                focus: document.point("right", 2).unwrap(),
                blocks: vec![seed("first", 0, "One"), seed("last", 1, "Two")],
            },
        )])
        .unwrap();
    document
        .insert_text(&document.point("middle", 3).unwrap(), "!")
        .unwrap();
    let before = document.view().unwrap();
    assert!(document.validate_proposal_acceptance("paste").is_err());
    assert!(document.accept_proposal("paste").is_err());
    assert_eq!(document.view().unwrap(), before);
    assert_eq!(document.block_view("middle").unwrap().text, "Rem!ove");
    reload(&document);
}

#[test]
fn suggesting_replaces_one_visible_selection_across_canonical_and_proposed_text() {
    let mut document = Document::from_blocks(&[seed("a", 0, "Before TARGET after")]).unwrap();
    document
        .propose_insertion(
            "existing",
            "Agent",
            "a",
            &document.point("a", 7).unwrap(),
            "NEW",
        )
        .unwrap();
    let mut ranges = document.selection("a", 4, 10).unwrap();
    ranges.extend(document.proposal_selection("existing", 0, 3).unwrap());
    document
        .apply(&[suggest(
            "mixed",
            EditAction::ReplaceText {
                at: document.point("a", 4).unwrap(),
                ranges,
                text: "Changed".into(),
            },
        )])
        .unwrap();

    let view = document.view().unwrap();
    assert_eq!(view.blocks[0].text, "Before TARGET after");
    assert_eq!(
        document.proposal("existing").unwrap().state,
        ProposalState::Rejected
    );
    let mixed = view
        .proposals
        .iter()
        .find(|view| view.proposal.id == "mixed")
        .unwrap();
    assert_eq!(mixed.text, "Changed");
    assert_eq!(
        mixed
            .target
            .attachments
            .iter()
            .map(|part| part.quote.as_str())
            .collect::<String>(),
        "re TAR"
    );
    document.accept_proposal("mixed").unwrap();
    assert_eq!(
        document.block_view("a").unwrap().text,
        "BefoChangedGET after"
    );
    reload(&document);
}

#[test]
fn direct_editing_replaces_canonical_and_proposed_text_without_a_review_prerequisite() {
    let mut document = Document::from_blocks(&[seed("a", 0, "Before TARGET after")]).unwrap();
    document
        .propose_insertion(
            "existing",
            "Agent",
            "a",
            &document.point("a", 7).unwrap(),
            "NEW",
        )
        .unwrap();
    let mut ranges = document.selection("a", 4, 10).unwrap();
    ranges.extend(document.proposal_selection("existing", 0, 3).unwrap());
    document
        .apply(&[direct(EditAction::ReplaceText {
            at: document.point("a", 4).unwrap(),
            ranges,
            text: "Changed".into(),
        })])
        .unwrap();

    assert_eq!(
        document.block_view("a").unwrap().text,
        "BefoChangedGET after"
    );
    assert_eq!(
        document.proposal("existing").unwrap().state,
        ProposalState::Rejected
    );
    reload(&document);
}

#[test]
fn replacing_a_proposed_split_boundary_rejects_that_structure_in_the_same_edit() {
    let mut document = Document::from_blocks(&[seed("a", 0, "Before TARGET")]).unwrap();
    document
        .apply(&[suggest(
            "split",
            EditAction::SplitBlock {
                block: "a".into(),
                proposal: None,
                at: document.point("a", 7).unwrap(),
                new_block: "tail".into(),
            },
        )])
        .unwrap();
    let mut ranges = document.proposal_selection("split", 0, 1).unwrap();
    ranges.extend(document.selection("a", 7, 10).unwrap());
    document
        .apply(&[suggest(
            "replace",
            EditAction::ReplaceText {
                at: document.proposal_point("split", 0).unwrap(),
                ranges,
                text: "New".into(),
            },
        )])
        .unwrap();

    assert_eq!(
        document.proposal("split").unwrap().state,
        ProposalState::Rejected
    );
    assert!(
        document
            .view()
            .unwrap()
            .proposals
            .iter()
            .find(|view| view.proposal.id == "replace")
            .unwrap()
            .acceptance_error
            .is_none()
    );
    document.accept_proposal("replace").unwrap();
    assert_eq!(document.block_view("a").unwrap().text, "Before NewGET");
    reload(&document);
}

#[test]
fn cut_and_paste_reassigns_the_same_text_to_a_new_owner_after_reload() {
    let mut document = Document::from_blocks(&[
        seed("source", 0, "AA TARGET ZZ"),
        seed("destination", 1, "dest"),
    ])
    .unwrap();
    document
        .add_comment(
            "comment",
            "Agent",
            "Keep",
            &document.selection("source", 3, 9).unwrap(),
        )
        .unwrap();
    let before_cut = document.heads();
    document
        .apply(&[direct(EditAction::CutSelection {
            transfer: "transfer".into(),
            anchor: document.point("source", 3).unwrap(),
            focus: document.point("source", 9).unwrap(),
        })])
        .unwrap();
    let after_cut = document.heads();
    let view = document.view().unwrap();
    assert_eq!(view.blocks[0].text, "AA  ZZ");
    assert_eq!(view.comments[0].target.state, TargetState::Hidden);

    let mut document = reload(&document);
    let before_paste = document.heads();
    document
        .apply(&[direct(EditAction::PasteCut {
            transfer: "transfer".into(),
            at: document.point("destination", 4).unwrap(),
            new_block: "unused".into(),
        })])
        .unwrap();
    let after_paste = document.heads();
    let view = document.view().unwrap();
    assert_eq!(
        document.block_view("destination").unwrap().text,
        "destTARGET"
    );
    assert_eq!(view.comments[0].target.state, TargetState::Attached);
    assert_eq!(
        view.comments[0].target.attachments[0].owner,
        TargetOwner::Block("destination".into())
    );
    assert_eq!(view.comments[0].target.attachments[0].quote, "TARGET");

    let snapshot = document.save();
    assert!(
        document
            .apply(&[direct(EditAction::PasteCut {
                transfer: "transfer".into(),
                at: document.point("destination", 0).unwrap(),
                new_block: "repeat".into(),
            })])
            .is_err()
    );
    assert_eq!(document.save(), snapshot);

    document
        .apply(&[Command::Revert {
            before: before_paste.iter().map(ToString::to_string).collect(),
            after: after_paste.iter().map(ToString::to_string).collect(),
        }])
        .unwrap();
    assert_eq!(
        document.view().unwrap().comments[0].target.state,
        TargetState::Hidden
    );
    document
        .apply(&[Command::Revert {
            before: before_cut.iter().map(ToString::to_string).collect(),
            after: after_cut.iter().map(ToString::to_string).collect(),
        }])
        .unwrap();
    assert_eq!(document.block_view("source").unwrap().text, "AA TARGET ZZ");
    assert_eq!(
        document.view().unwrap().comments[0].target.state,
        TargetState::Attached
    );
    reload(&document);
}

#[test]
fn multi_block_cut_survives_concurrent_review_and_recreates_siblings_on_paste() {
    let mut document = Document::from_blocks(&[
        seed("a", 0, "pre ONE"),
        seed("b", 1, "TWO"),
        seed("c", 2, "THREE tail"),
        seed("destination", 3, "here"),
    ])
    .unwrap();
    let cut = request(
        &document,
        "cut",
        vec![direct(EditAction::CutSelection {
            transfer: "multi".into(),
            anchor: document.point("a", 4).unwrap(),
            focus: document.point("c", 5).unwrap(),
        })],
    );
    document
        .add_comment(
            "late-comment",
            "Agent",
            "Keep",
            &document.selection("b", 0, 3).unwrap(),
        )
        .unwrap();
    document
        .insert_text(&document.point("c", 2).unwrap(), "!")
        .unwrap();
    document.apply_request(&cut).unwrap();
    assert_eq!(document.view().unwrap().blocks.len(), 2);
    assert_eq!(document.block_view("a").unwrap().text, "pre  tail");
    assert_eq!(
        document.view().unwrap().comments[0].target.state,
        TargetState::Hidden
    );

    let mut document = reload(&document);
    document
        .apply(&[direct(EditAction::PasteCut {
            transfer: "multi".into(),
            at: document.point("destination", 2).unwrap(),
            new_block: "pasted".into(),
        })])
        .unwrap();
    let view = document.view().unwrap();
    assert_eq!(
        view.blocks
            .iter()
            .map(|block| (block.block.id.as_str(), block.text.as_str()))
            .collect::<Vec<_>>(),
        [
            ("a", "pre  tail"),
            ("destination", "heONE"),
            ("pasted", "TWO"),
            ("pasted:1", "TH!REEre"),
        ]
    );
    assert_eq!(view.comments[0].target.state, TargetState::Attached);
    assert_eq!(
        view.comments[0].target.attachments[0].owner,
        TargetOwner::Block("pasted".into())
    );
    assert_eq!(view.comments[0].target.attachments[0].quote, "TWO");
    reload(&document);
}
