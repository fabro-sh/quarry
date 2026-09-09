#![allow(
    clippy::unwrap_used,
    reason = "tests use explicit native edit fixtures"
)]
use quarry_document::{
    Command, Document, EditAction, EditMode, ProposalAction, SeedBlock, TargetOwner,
};

fn seed() -> Document {
    Document::from_blocks(&[
        SeedBlock {
            id: "a".into(),
            kind: "p".into(),
            parent: None,
            position: 0,
            attrs: Default::default(),
            text: "Before 😀TARGET after.".into(),
        },
        SeedBlock {
            id: "b".into(),
            kind: "p".into(),
            parent: None,
            position: 1,
            attrs: Default::default(),
            text: "Elsewhere".into(),
        },
    ])
    .unwrap()
}
fn suggest(id: &str) -> EditMode {
    EditMode::Suggest {
        id: id.into(),
        author: "Reviewer".into(),
    }
}
fn resume(id: &str) -> EditMode {
    EditMode::Continue {
        id: id.into(),
        author: "Reviewer".into(),
    }
}
fn replace(d: &Document, mode: EditMode, start: usize, end: usize, text: &str) -> Command {
    Command::Edit {
        mode,
        action: EditAction::ReplaceText {
            at: d.point("a", start).unwrap(),
            ranges: d.selection("a", start, end).unwrap(),
            text: text.into(),
        },
    }
}
fn unrelated(d: &mut Document) {
    d.propose_insertion("agent", "Agent", "b", &d.point("b", 0).unwrap(), "Remote ")
        .unwrap();
    d.insert_text(&d.point("b", 0).unwrap(), "Other ").unwrap();
    d.add_comment(
        "remote-comment",
        "Agent",
        "Keep",
        &d.selection("a", 7, 15).unwrap(),
    )
    .unwrap();
}

#[test]
fn segmentation_and_delivery_do_not_define_suggestion_identity() {
    for sizes in [vec![8], vec![2, 6], vec![1, 1, 1, 1, 1, 1, 2]] {
        for one_request in [false, true] {
            for remote in [false, true] {
                let original = seed();
                let mut browser = original.fork();
                let mut server = original.fork();
                let mut builder = browser.command_builder("first", "now").unwrap();
                let mut end = 15;
                for (index, size) in sizes.iter().enumerate() {
                    let action = replace(
                        &builder,
                        if index == 0 {
                            suggest("delete")
                        } else {
                            resume("delete")
                        },
                        end - size,
                        end,
                        "",
                    );
                    builder.push(vec![action]).unwrap();
                    end -= size;
                    if !one_request && index + 1 < sizes.len() {
                        let (next, request) = builder.finish().unwrap();
                        browser = next;
                        server.apply_request(&request).unwrap();
                        builder = browser
                            .command_builder(&format!("part{}", index), "now")
                            .unwrap();
                    }
                }
                let (local, request) = builder.finish().unwrap();
                browser = local;
                if remote {
                    unrelated(&mut server);
                }
                server.apply_request(&request).unwrap();
                assert_eq!(
                    server.proposal("delete").unwrap(),
                    browser.proposal("delete").unwrap()
                );
                assert_eq!(
                    server.proposal_target("delete").unwrap().attachments[0].quote,
                    "😀TARGET"
                );
                assert_eq!(
                    server.proposals().unwrap().len(),
                    if remote { 2 } else { 1 }
                );
                if !remote {
                    assert_eq!(server.heads(), browser.heads());
                }
                server.accept_proposal("delete").unwrap();
                assert_eq!(server.block_view("a").unwrap().text, "Before  after.");
            }
        }
    }
}

#[test]
fn independent_adjacent_intents_remain_distinct_and_continuations_are_explicit() {
    let mut d = seed();
    d.apply(&[replace(&d, suggest("first"), 12, 15, "")])
        .unwrap();
    d.apply(&[replace(&d, suggest("second"), 9, 12, "")])
        .unwrap();
    assert_eq!(d.proposals().unwrap().len(), 2);
    assert_eq!(
        d.proposal_target("first").unwrap().attachments[0].quote,
        "GET"
    );
    assert_eq!(
        d.proposal_target("second").unwrap().attachments[0].quote,
        "TAR"
    );
    let heads = d.heads();
    for action in [
        replace(&d, suggest("first"), 7, 9, ""),
        replace(&d, resume("first"), 7, 9, ""),
        replace(
            &d,
            EditMode::Continue {
                id: "first".into(),
                author: "Someone else".into(),
            },
            9,
            12,
            "",
        ),
    ] {
        assert!(d.apply(&[action]).is_err());
        assert_eq!(d.heads(), heads);
    }
}

#[test]
fn deletion_then_insertion_is_one_replacement_and_keeps_proposed_character_identity() {
    let mut d = seed();
    d.apply(&[replace(&d, suggest("change"), 9, 15, "")])
        .unwrap();
    d.reply_comment("reply", "change", "Agent", "Discuss")
        .unwrap();
    d.apply(&[replace(&d, resume("change"), 9, 9, "new")])
        .unwrap();
    let ranges = d.proposal_selection("change", 0, 3).unwrap();
    d.add_comment("c", "Agent", "New text", &ranges).unwrap();
    let segments = d.proposal("change").unwrap().segments;
    d.apply(&[Command::Edit {
        mode: resume("change"),
        action: EditAction::ReplaceText {
            at: d.proposal_point("change", 3).unwrap(),
            ranges: vec![],
            text: " text".into(),
        },
    }])
    .unwrap();
    assert_eq!(d.proposal("change").unwrap().segments, segments);
    assert_eq!(d.comment_target("c").unwrap().attachments[0].quote, "new");
    d.accept_proposal("change").unwrap();
    assert_eq!(d.block_view("a").unwrap().text, "Before 😀new text after.");
    assert_eq!(
        d.comment_target("c").unwrap().attachments[0].owner,
        TargetOwner::Block("a".into())
    );
    assert_eq!(
        d.comment("reply").unwrap().parent_id.as_deref(),
        Some("change")
    );
}

#[test]
fn a_request_can_address_proposed_sources_it_created_before_unrelated_remote_work() {
    let mut authority = seed();
    let mut builder = authority.command_builder("dependent", "now").unwrap();
    builder
        .push(vec![replace(&builder, suggest("change"), 9, 15, "new")])
        .unwrap();
    builder
        .push(vec![Command::Edit {
            mode: resume("change"),
            action: EditAction::ReplaceText {
                at: builder.proposal_point("change", 3).unwrap(),
                ranges: vec![],
                text: " text".into(),
            },
        }])
        .unwrap();
    builder
        .push(vec![Command::Edit {
            mode: suggest("format-unused"),
            action: EditAction::Format {
                ranges: builder.proposal_selection("change", 0, 8).unwrap(),
                name: "bold".into(),
                value: true.into(),
            },
        }])
        .unwrap();
    let (local, request) = builder.finish().unwrap();
    unrelated(&mut authority);
    authority.apply_request(&request).unwrap();
    assert_eq!(
        authority.proposal("change").unwrap(),
        local.proposal("change").unwrap()
    );
    authority.accept_proposal("change").unwrap();
    assert_eq!(
        authority.block_view("a").unwrap().text,
        "Before 😀new text after."
    );
}

#[test]
fn competing_target_changes_and_decisions_fail_without_partial_publication() {
    let mut base = seed();
    base.apply(&[replace(&base, suggest("remove"), 12, 15, "")])
        .unwrap();
    let mut builder = base.command_builder("continuation", "now").unwrap();
    builder
        .push(vec![replace(&builder, resume("remove"), 9, 12, "")])
        .unwrap();
    let (_, request) = builder.finish().unwrap();
    for change in 0..5 {
        let mut authority = base.fork();
        match change {
            0 => authority
                .insert_text(&authority.point("a", 13).unwrap(), "X")
                .unwrap(),
            1 => authority
                .insert_text(&authority.point("a", 10).unwrap(), "X")
                .unwrap(),
            2 => authority
                .insert_text(&authority.point("a", 12).unwrap(), "X")
                .unwrap(),
            3 => authority.reject_proposal("remove").unwrap(),
            _ => authority
                .apply(&[replace(&authority, resume("remove"), 9, 12, "")])
                .unwrap(),
        }
        let heads = authority.heads();
        assert!(
            authority.apply_request(&request).is_err(),
            "change {change}"
        );
        assert_eq!(authority.heads(), heads);
    }
}

#[test]
fn native_modes_cover_format_and_structure_and_reject_unsupported_actions_atomically() {
    let mut d = seed();
    d.apply(&[
        Command::Edit {
            mode: suggest("format"),
            action: EditAction::Format {
                ranges: d.selection("a", 9, 15).unwrap(),
                name: "bold".into(),
                value: true.into(),
            },
        },
        Command::Edit {
            mode: suggest("heading"),
            action: EditAction::SetBlock {
                block: "b".into(),
                proposal: None,
                kind: "h2".into(),
                attrs: Default::default(),
            },
        },
    ])
    .unwrap();
    assert!(matches!(
        d.proposal("format").unwrap().action,
        ProposalAction::Format { .. }
    ));
    assert_eq!(d.block("b").unwrap().kind, "p");
    d.apply(&[Command::Edit {
        mode: suggest("split"),
        action: EditAction::SplitBlock {
            block: "a".into(),
            proposal: None,
            at: d.point("a", 9).unwrap(),
            new_block: "new".into(),
        },
    }])
    .unwrap();
    assert!(matches!(
        d.proposal("split").unwrap().action,
        ProposalAction::SplitBlock { .. }
    ));
    assert!(d.block("new").is_err());
    let heads = d.heads();
    let action = Command::Edit {
        mode: suggest("move-text"),
        action: EditAction::MoveText {
            block: "a".into(),
            proposal: None,
            start: d.point("a", 0).unwrap(),
            end: d.point("a", 1).unwrap(),
            to: d.point("a", 2).unwrap(),
        },
    };
    assert!(
        d.apply(&[replace(&d, EditMode::Direct, 0, 0, "leak"), action])
            .is_err()
    );
    assert_eq!(d.heads(), heads);
    d.accept_proposal("heading").unwrap();
    assert_eq!(d.block("b").unwrap().kind, "h2");
}

#[test]
fn cross_block_selection_replacement_matches_deletion_then_typing() {
    let original = seed();
    let ranges: Vec<_> = original
        .selection("a", 9, 22)
        .unwrap()
        .into_iter()
        .chain(original.selection("b", 0, 4).unwrap())
        .collect();
    let mut combined = original.fork();
    combined
        .apply(&[Command::Edit {
            mode: suggest("change"),
            action: EditAction::ReplaceText {
                at: original.point("a", 9).unwrap(),
                ranges: ranges.clone(),
                text: "New".into(),
            },
        }])
        .unwrap();
    let mut separate = original.fork();
    separate
        .apply(&[Command::Edit {
            mode: suggest("change"),
            action: EditAction::ReplaceText {
                at: original.point("a", 9).unwrap(),
                ranges,
                text: String::new(),
            },
        }])
        .unwrap();
    separate
        .apply(&[replace(&separate, resume("change"), 9, 9, "New")])
        .unwrap();
    assert_eq!(
        combined.proposal("change").unwrap().action,
        separate.proposal("change").unwrap().action
    );
    combined.accept_proposal("change").unwrap();
    separate.accept_proposal("change").unwrap();
    assert_eq!(
        combined.block_view("a").unwrap().text,
        separate.block_view("a").unwrap().text
    );
    assert_eq!(
        combined.block_view("b").unwrap().text,
        separate.block_view("b").unwrap().text
    );
}
