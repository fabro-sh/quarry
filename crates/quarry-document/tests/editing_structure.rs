#![allow(clippy::unwrap_used, reason = "explicit structural edit fixtures")]
use quarry_document::{
    Command, CommandRequest, Document, EditAction, EditMode, SeedBlock, TargetOwner,
};

fn seed(id: &str, kind: &str, parent: Option<&str>, position: usize, text: &str) -> SeedBlock {
    SeedBlock {
        id: id.into(),
        kind: kind.into(),
        attrs: Default::default(),
        parent: parent.map(str::to_owned),
        position,
        text: text.into(),
    }
}
fn edit(action: EditAction) -> Command {
    Command::Edit {
        mode: EditMode::Direct,
        action,
    }
}
fn fresh(d: &Document) {
    assert_eq!(
        d.view().unwrap(),
        Document::load(&d.save()).unwrap().view().unwrap()
    );
}
fn request(d: &Document, id: &str, commands: Vec<Command>) -> CommandRequest {
    CommandRequest {
        request_id: id.into(),
        base: d.heads().iter().map(ToString::to_string).collect(),
        commands,
        at: String::new(),
    }
}

#[test]
fn selection_replacement_is_atomic_direction_independent_and_preserves_surviving_review() {
    for count in [2, 3, 5, 9] {
        for reverse in [false, true] {
            for text in ["", "new 😀"] {
                let mut d = Document::from_blocks(
                    &(0..count)
                        .map(|n| seed(&n.to_string(), "p", None, n, "😀 TARGET tail"))
                        .collect::<Vec<_>>(),
                )
                .unwrap();
                let last = (count - 1).to_string();
                let late = request(
                    &d,
                    "late-comment",
                    vec![Command::AddComment {
                        id: "review".into(),
                        author: "Agent".into(),
                        body: "Keep".into(),
                        ranges: d.selection(&last, 3, 9).unwrap(),
                    }],
                );
                let mut reader = d.fork();
                fresh(&reader);
                let before = d.heads();
                let a = d.point("0", 2).unwrap();
                let b = d.point(&last, 3).unwrap();
                let action = edit(EditAction::ReplaceSelection {
                    anchor: if reverse { b.clone() } else { a.clone() },
                    focus: if reverse { a } else { b },
                    text: text.into(),
                });
                let mut builder = d.command_builder("replace", "").unwrap();
                builder.push(vec![action]).unwrap();
                let (local, command) = builder.finish().unwrap();
                d.apply_request(&command).unwrap();
                assert_eq!(local.view().unwrap(), d.view().unwrap());
                let after = d.heads();
                d.apply_request(&late).unwrap();
                let view = d.view().unwrap();
                assert_eq!(view.blocks.len(), 1);
                assert_eq!(view.blocks[0].text, format!("😀{text}TARGET tail"));
                assert_eq!(view.comments[0].target.attachments[0].quote, "TARGET");
                assert_eq!(
                    view.comments[0].target.attachments[0].owner,
                    TargetOwner::Block("0".into())
                );
                reader
                    .merge_changes(&d.save_after(&before).unwrap(), &before, &d.heads())
                    .unwrap();
                assert_eq!(reader.view().unwrap(), view);
                fresh(&reader);
                d.apply(&[Command::Revert {
                    before: before.iter().map(ToString::to_string).collect(),
                    after: after.iter().map(ToString::to_string).collect(),
                }])
                .unwrap();
                assert_eq!(d.view().unwrap().blocks.len(), count);
                assert_eq!(
                    d.view().unwrap().comments[0].target.attachments[0].quote,
                    "TARGET"
                );
                fresh(&d);
            }
        }
    }
}

#[test]
fn delayed_selection_preserves_endpoint_insertions_and_rejects_unseen_middle_content() {
    for middle in [false, true] {
        let mut d = Document::from_blocks(&[
            seed("a", "p", None, 0, "before"),
            seed("b", "p", None, 1, "middle"),
            seed("c", "p", None, 2, "TARGET"),
        ])
        .unwrap();
        let command = request(
            &d,
            "replace",
            vec![edit(EditAction::ReplaceSelection {
                anchor: d.point("a", 3).unwrap(),
                focus: d.point("c", 0).unwrap(),
                text: String::new(),
            })],
        );
        let block = if middle { "b" } else { "c" };
        d.insert_text(&d.point(block, 3).unwrap(), "AGENT").unwrap();
        let before = d.save();
        let result = d.apply_request(&command);
        if middle {
            assert!(result.is_err());
            assert_eq!(before, d.save());
        } else {
            result.unwrap();
            assert_eq!(d.view().unwrap().blocks[0].text, "befTARAGENTGET");
        }
        fresh(&d);
    }
}

#[test]
fn text_moves_keep_sources_comments_concurrent_typing_and_undo() {
    for same_block in [false, true] {
        for offset in [0, 13] {
            let mut d = Document::from_blocks(&[
                seed("a", "p", None, 0, "😀 TARGET end"),
                seed("b", "p", None, 1, "destination!!"),
            ])
            .unwrap();
            let destination = if same_block { "a" } else { "b" };
            let offset = if same_block && offset == 13 {
                13
            } else {
                offset
            };
            let command = request(
                &d,
                "move",
                vec![edit(EditAction::MoveText {
                    proposal: None,
                    block: "a".into(),
                    start: d.point("a", 3).unwrap(),
                    end: d.point("a", 9).unwrap(),
                    to: d.point(destination, offset).unwrap(),
                })],
            );
            let late = request(
                &d,
                "review",
                vec![Command::AddComment {
                    id: "c".into(),
                    author: "A".into(),
                    body: "Keep".into(),
                    ranges: d.selection("a", 3, 9).unwrap(),
                }],
            );
            let sources = d
                .block("a")
                .unwrap()
                .segments
                .iter()
                .map(|r| r.source.clone())
                .collect::<Vec<_>>();
            d.insert_text(&d.point("a", 6).unwrap(), "!").unwrap();
            let before = d.heads();
            d.apply_request(&command).unwrap();
            let after = d.heads();
            d.apply_request(&late).unwrap();
            let attachment = &d.view().unwrap().comments[0].target.attachments[0];
            assert_eq!(attachment.owner, TargetOwner::Block(destination.into()));
            assert!(d.block_view(destination).unwrap().text.contains("TAR!GET"));
            assert!(
                d.block(destination)
                    .unwrap()
                    .segments
                    .iter()
                    .any(|r| sources.contains(&r.source))
            );
            fresh(&d);
            d.apply(&[Command::Revert {
                before: before.iter().map(ToString::to_string).collect(),
                after: after.iter().map(ToString::to_string).collect(),
            }])
            .unwrap();
            assert_eq!(d.block_view("a").unwrap().text, "😀 TAR!GET end");
            fresh(&d);
        }
    }
}

#[test]
fn containers_split_and_join_in_one_ordered_builder() {
    let d = Document::from_blocks(&[
        seed("code", "code_block", None, 0, ""),
        seed("a", "code_line", Some("code"), 0, "first"),
        seed("b", "code_line", Some("code"), 1, "second"),
    ])
    .unwrap();
    let mut builder = d.command_builder("split-join", "").unwrap();
    builder
        .push(vec![edit(EditAction::SplitContainer {
            block: "code".into(),
            at: 1,
            new_block: "next".into(),
        })])
        .unwrap();
    assert_eq!(builder.block("b").unwrap().parent.as_deref(), Some("next"));
    builder
        .push(vec![edit(EditAction::JoinBlocks {
            left: "code".into(),
            right: "next".into(),
            proposal: None,
        })])
        .unwrap();
    let (next, request) = builder.finish().unwrap();
    let mut authority = d.fork();
    authority.apply_request(&request).unwrap();
    assert_eq!(next.view().unwrap(), authority.view().unwrap());
    assert_eq!(next.block("b").unwrap().parent.as_deref(), Some("code"));
    fresh(&next);
}

#[test]
fn ordered_proposed_edits_keep_existing_text_and_reject_invalid_final_trees() {
    use quarry_document::ProposedBlockEdit;
    let mut d = Document::from_blocks(&[seed("base", "p", None, 0, "Base")]).unwrap();
    d.apply(&[Command::ProposeBlocks {
        id: "proposal".into(),
        author: "A".into(),
        parent: None,
        before: None,
        blocks: vec![
            seed("code", "code_block", None, 0, ""),
            seed("a", "code_line", Some("code"), 0, "first"),
            seed("b", "code_line", Some("code"), 1, "TARGET"),
        ],
    }])
    .unwrap();
    let ranges = d.proposed_block_selection("proposal", "b", 0, 6).unwrap();
    d.add_comment("c", "A", "Keep", &ranges).unwrap();
    let change = |action| {
        edit(EditAction::EditProposedBlocks {
            proposal: "proposal".into(),
            action,
        })
    };
    let mut builder = d.command_builder("proposed", "").unwrap();
    builder
        .push(vec![
            change(ProposedBlockEdit::SplitContainer {
                block: "code".into(),
                at: 1,
                new_block: "next".into(),
            }),
            change(ProposedBlockEdit::Insert {
                parent: Some("next".into()),
                before: Some("b".into()),
                blocks: vec![seed("new", "code_line", None, 0, "New")],
            }),
            change(ProposedBlockEdit::Move {
                block: "new".into(),
                parent: Some("code".into()),
                before: None,
            }),
            change(ProposedBlockEdit::JoinContainers {
                left: "code".into(),
                right: "next".into(),
            }),
            change(ProposedBlockEdit::Delete {
                block: "new".into(),
            }),
        ])
        .unwrap();
    let (local, request) = builder.finish().unwrap();
    d.apply_request(&request).unwrap();
    assert_eq!(d.view().unwrap(), local.view().unwrap());
    assert_eq!(
        d.view().unwrap().comments[0].target.attachments[0].quote,
        "TARGET"
    );
    fresh(&d);
    let before = d.save();
    let mut invalid = d.command_builder("invalid", "").unwrap();
    invalid
        .push(vec![change(ProposedBlockEdit::Move {
            block: "b".into(),
            parent: Some("a".into()),
            before: None,
        })])
        .unwrap();
    assert!(invalid.finish().is_err());
    assert_eq!(d.save(), before);
}

#[test]
fn replacing_across_containers_keeps_native_boundaries_and_unselected_characters() {
    let mut d = Document::from_blocks(&[
        seed("a", "p", None, 0, "First"),
        seed("code", "code_block", None, 1, ""),
        seed("b", "code_line", Some("code"), 0, "Code"),
        seed("c", "p", None, 2, "TARGET"),
    ])
    .unwrap();
    let blocks = d.blocks().unwrap();
    d.apply(&[edit(EditAction::ReplaceSelection {
        anchor: d.point("a", 2).unwrap(),
        focus: d.point("c", 0).unwrap(),
        text: "new".into(),
    })])
    .unwrap();
    let view = d.view().unwrap();
    assert_eq!(
        view.blocks
            .iter()
            .map(|v| (&v.block.id, &v.block.kind, &v.block.parent))
            .collect::<Vec<_>>(),
        blocks
            .iter()
            .map(|b| (&b.id, &b.kind, &b.parent))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        view.blocks
            .iter()
            .map(|v| v.text.as_str())
            .collect::<Vec<_>>(),
        ["Finew", "", "", "TARGET"]
    );
    fresh(&d);
}

#[test]
fn a_delayed_container_join_preserves_concurrent_child_text() {
    let mut d = Document::from_blocks(&[
        seed("left", "code_block", None, 0, ""),
        seed("a", "code_line", Some("left"), 0, "first"),
        seed("right", "code_block", None, 1, ""),
        seed("b", "code_line", Some("right"), 0, "TARGET"),
    ])
    .unwrap();
    let join = request(
        &d,
        "join",
        vec![edit(EditAction::JoinBlocks {
            left: "left".into(),
            right: "right".into(),
            proposal: None,
        })],
    );
    d.insert_text(&d.point("b", 3).unwrap(), "!").unwrap();
    d.apply_request(&join).unwrap();
    assert_eq!(d.block("b").unwrap().parent.as_deref(), Some("left"));
    assert_eq!(d.block_view("b").unwrap().text, "TAR!GET");
    fresh(&d);
}
