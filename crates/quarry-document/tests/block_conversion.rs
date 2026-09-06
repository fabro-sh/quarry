#![allow(clippy::unwrap_used, reason = "explicit conversion fixtures")]
use quarry_document::{
    BlockConversion, Command, Document, EditAction, EditMode, ListFormat, SeedBlock, TargetOwner,
};
use serde_json::json;

fn seed(style: &str) -> Document {
    Document::from_blocks(&[SeedBlock { id: "item".into(), kind: "p".into(), parent: None, position: 0,
        attrs: serde_json::from_value(json!({"listStyleType":style,"indent":2,"checked":true,"listStart":7,"listRestart":7,"listRestartPolite":7,"align":"right"})).unwrap(),
        text: "Before 😀TARGET after.".into() },
        SeedBlock { id: "next".into(), kind: "p".into(), parent: None, position: 1, attrs: Default::default(), text: "Next".into() }]).unwrap()
}
fn conversion(block: &str, mode: EditMode, target: BlockConversion) -> Command {
    Command::Edit {
        mode,
        action: EditAction::ConvertBlock {
            block: block.into(),
            proposal: None,
            target,
        },
    }
}
fn suggesting() -> EditMode {
    EditMode::Suggest {
        id: "conversion".into(),
        author: "Reviewer".into(),
    }
}
fn comment(d: &mut Document) {
    d.add_comment(
        "comment",
        "Reviewer",
        "Keep",
        &d.selection("item", 7, 15).unwrap(),
    )
    .unwrap();
}
fn assert_comment(d: &Document) {
    let target = d.view().unwrap().comments[0].target.clone();
    assert_eq!(target.attachments.len(), 1);
    assert_eq!(target.attachments[0].quote, "😀TARGET");
    assert_eq!(
        target.attachments[0].owner,
        TargetOwner::Block("item".into())
    );
}

#[test]
fn every_list_conversion_has_the_same_native_direct_and_review_result() {
    for style in ["disc", "decimal", "todo"] {
        for kind in [
            "p",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "blockquote",
            "code_block",
        ] {
            for suggested in [false, true] {
                let mut d = seed(style);
                comment(&mut d);
                let original = d.blocks().unwrap();
                let before = d.heads();
                let mut builder = d.command_builder("convert", "now").unwrap();
                builder
                    .push(vec![conversion(
                        "item",
                        if suggested {
                            suggesting()
                        } else {
                            EditMode::Direct
                        },
                        BlockConversion::plain(kind),
                    )])
                    .unwrap();
                let (browser, request) = builder.finish().unwrap();
                d.apply_request(&request).unwrap();
                assert_eq!(d.view().unwrap(), browser.view().unwrap());
                if suggested {
                    assert_eq!(d.blocks().unwrap(), original);
                    assert_eq!(d.proposals().unwrap().len(), 1);
                    d.insert_text(&d.point("item", 0).unwrap(), "Agent ")
                        .unwrap();
                    d.accept_proposal("conversion").unwrap();
                }
                let block = d.block("item").unwrap();
                assert_eq!(
                    block.kind,
                    if kind == "code_block" {
                        "code_line"
                    } else {
                        kind
                    }
                );
                assert_eq!(
                    block.attrs,
                    serde_json::from_value(json!({"align":"right"})).unwrap()
                );
                assert_eq!(block.segments, original[0].segments);
                assert_comment(&d);
                if !suggested {
                    let after = d.heads();
                    d.revert(&before, &after).unwrap();
                    assert_eq!(d.blocks().unwrap(), original);
                    assert_comment(&d);
                }
                Document::load(&d.save()).unwrap().validate().unwrap();
            }
        }
    }
}

#[test]
fn list_targets_are_explicit_and_raw_block_writes_remain_strict() {
    let mut d = seed("todo");
    let block = d.block("item").unwrap();
    assert!(
        d.apply(&[Command::SetBlock {
            block: "item".into(),
            kind: "h3".into(),
            attrs: block.attrs.clone()
        }])
        .is_err()
    );
    assert_eq!(d.block("item").unwrap(), block);
    let target = BlockConversion {
        kind: "p".into(),
        list: Some(ListFormat {
            style: "decimal".into(),
            start: Some(9),
            checked: None,
            indent: None,
        }),
    };
    d.apply(&[conversion("item", EditMode::Direct, target)])
        .unwrap();
    assert_eq!(
        d.block("item").unwrap().attrs,
        serde_json::from_value(
            json!({"listStyleType":"decimal","indent":2,"listStart":9,"align":"right"})
        )
        .unwrap()
    );
    d.apply(&[conversion(
        "item",
        EditMode::Direct,
        BlockConversion::plain("p"),
    )])
    .unwrap();
    assert_eq!(
        d.block("item").unwrap().attrs,
        serde_json::from_value(json!({"align":"right"})).unwrap()
    );
}

fn code() -> Document {
    Document::from_blocks(&[
        SeedBlock {
            id: "code".into(),
            kind: "code_block".into(),
            parent: None,
            position: 0,
            attrs: serde_json::from_value(json!({"lang":"rust"})).unwrap(),
            text: String::new(),
        },
        SeedBlock {
            id: "first".into(),
            kind: "code_line".into(),
            parent: Some("code".into()),
            position: 0,
            attrs: Default::default(),
            text: "First".into(),
        },
        SeedBlock {
            id: "item".into(),
            kind: "code_line".into(),
            parent: Some("code".into()),
            position: 1,
            attrs: Default::default(),
            text: "Before 😀TARGET after.".into(),
        },
        SeedBlock {
            id: "last".into(),
            kind: "code_line".into(),
            parent: Some("code".into()),
            position: 2,
            attrs: Default::default(),
            text: "Last".into(),
        },
        SeedBlock {
            id: "next".into(),
            kind: "p".into(),
            parent: None,
            position: 1,
            attrs: Default::default(),
            text: "Next".into(),
        },
    ])
    .unwrap()
}

#[test]
fn lifting_any_code_line_preserves_order_sources_and_remaining_code() {
    for id in ["first", "item", "last", "code"] {
        for suggested in [false, true] {
            let mut d = code();
            comment(&mut d);
            let original = d.blocks().unwrap();
            let before = d.heads();
            d.apply(&[conversion(
                id,
                if suggested {
                    suggesting()
                } else {
                    EditMode::Direct
                },
                BlockConversion::plain("h3"),
            )])
            .unwrap();
            if suggested {
                assert_eq!(d.blocks().unwrap(), original);
                d.insert_text(&d.point("item", 0).unwrap(), "Agent ")
                    .unwrap();
                d.accept_proposal("conversion").unwrap();
            }
            let blocks = d.blocks().unwrap();
            assert_eq!(
                blocks
                    .iter()
                    .filter(|b| ["first", "item", "last", "next"].contains(&b.id.as_str()))
                    .map(|b| b.id.as_str())
                    .collect::<Vec<_>>(),
                ["first", "item", "last", "next"]
            );
            for old in original.iter().filter(|b| b.kind == "code_line") {
                let block = d.block(&old.id).unwrap();
                assert_eq!(block.segments, old.segments);
                assert_eq!(
                    block.kind,
                    if id == "code" || id == old.id {
                        "h3"
                    } else {
                        "code_line"
                    }
                );
            }
            assert_comment(&d);
            if !suggested {
                let after = d.heads();
                d.revert(&before, &after).unwrap();
                assert_eq!(d.blocks().unwrap(), original);
            }
        }
    }
}

#[test]
fn a_code_conversion_rejects_competing_structure_without_losing_text() {
    let mut d = code();
    d.apply(&[conversion(
        "item",
        suggesting(),
        BlockConversion::plain("h3"),
    )])
    .unwrap();
    d.split_block("item", &d.point("item", 7).unwrap(), "split")
        .unwrap();
    let before = d.view().unwrap();
    assert!(d.accept_proposal("conversion").is_err());
    assert_eq!(d.view().unwrap(), before);
}

#[test]
fn proposed_code_conversion_keeps_its_existing_review_identity() {
    let mut d = seed("disc");
    d.propose_blocks(
        "insertion",
        "Agent",
        None,
        None,
        &[SeedBlock {
            id: "proposed".into(),
            kind: "p".into(),
            parent: None,
            position: 0,
            attrs: serde_json::from_value(
                json!({"listStyleType":"todo","indent":1,"checked":true}),
            )
            .unwrap(),
            text: "TARGET".into(),
        }],
    )
    .unwrap();
    d.add_comment(
        "proposed-comment",
        "Reviewer",
        "Keep",
        &d.proposed_block_selection("insertion", "proposed", 0, 6)
            .unwrap(),
    )
    .unwrap();
    for kind in ["code_block", "h3", "code_block"] {
        d.apply(&[Command::Edit {
            mode: suggesting(),
            action: EditAction::ConvertBlock {
                block: "proposed".into(),
                proposal: Some("insertion".into()),
                target: BlockConversion::plain(kind),
            },
        }])
        .unwrap();
        assert_eq!(d.proposals().unwrap().len(), 1);
    }
    d.accept_proposal("insertion").unwrap();
    assert_eq!(d.block("proposed").unwrap().kind, "code_line");
    let attachment = &d.view().unwrap().comments[0].target.attachments[0];
    assert_eq!(attachment.owner, TargetOwner::Block("proposed".into()));
    assert_eq!(attachment.quote, "TARGET");
}
