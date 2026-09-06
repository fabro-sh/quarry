#![allow(
    clippy::unwrap_used,
    reason = "assertions use small, explicit document fixtures"
)]

use quarry_document::{DiscussionState, Document, SeedBlock, TargetOwner, TargetState};
use std::collections::BTreeMap;

fn seed(text: &str, second: &str) -> Document {
    Document::from_blocks(&[seed_block("a", text, 0), seed_block("b", second, 1)]).unwrap()
}

fn seed_block(id: &str, text: &str, position: usize) -> SeedBlock {
    SeedBlock {
        id: id.into(),
        kind: "p".into(),
        parent: None,
        position,
        attrs: BTreeMap::new(),
        text: text.into(),
    }
}

fn target_offset(text: &str) -> usize {
    text[..text.find("TARGET").unwrap()].encode_utf16().count()
}

fn annotate_and_type(doc: &mut Document, block: &str) {
    let text = doc.block_view(block).unwrap().text;
    let start = target_offset(&text);
    let selection = doc.selection(block, start, start + 6).unwrap();
    doc.add_comment("c", "reviewer", "Keep this", &selection)
        .unwrap();
    let at = doc.point(block, start + 3).unwrap();
    doc.insert_text(&at, "!").unwrap();
}

#[test]
fn moves_and_splits_preserve_delayed_comments_and_typing_in_both_merge_orders() {
    for text in ["See TARGET here.", "😀 e\u{301} See TARGET here."] {
        for repeated in [false, true] {
            for reverse in [false, true] {
                let base = seed(
                    text,
                    if repeated {
                        "See TARGET elsewhere."
                    } else {
                        "Second paragraph."
                    },
                );
                let mut agent = base.fork();
                let mut browser = base.fork();
                let at = agent.point("a", target_offset(text) + 3).unwrap();
                agent.split_block("a", &at, "tail").unwrap();
                agent.move_block("tail", None, None).unwrap();
                annotate_and_type(&mut browser, "a");
                let mut result = if reverse {
                    browser.fork()
                } else {
                    agent.fork()
                };
                result
                    .merge(if reverse { &agent } else { &browser })
                    .unwrap();
                assert_eq!(
                    result.block_view("b").unwrap().text,
                    if repeated {
                        "See TARGET elsewhere."
                    } else {
                        "Second paragraph."
                    }
                );
                // A character inserted exactly at the concurrent split may end
                // up on either side of the new marker. It must remain inside
                // the original target, never in the unrelated paragraph.
                let left = result.block_view("a").unwrap().text;
                let right = result.block_view("tail").unwrap().text;
                assert_eq!(format!("{left}{right}"), text.replace("TARGET", "TAR!GET"));
                let target = result.comment_target("c").unwrap();
                assert_eq!(target.state, TargetState::Attached);
                assert!(target.attachments.iter().all(
                    |a| matches!(&a.owner,TargetOwner::Block(id) if id == "a" || id == "tail")
                ));
                let quote: String = target
                    .attachments
                    .iter()
                    .map(|a| a.quote.as_str())
                    .collect();
                assert_eq!(quote, "TAR!GET");
                let restored = Document::load(&result.save()).unwrap();
                assert_eq!(restored.comment_target("c").unwrap(), target);
            }
        }
    }
}

#[test]
fn move_does_not_redirect_repeated_text() {
    let base = seed("See TARGET here.", "See TARGET elsewhere.");
    let mut a = base.fork();
    let mut b = base.fork();
    a.move_block("a", None, None).unwrap();
    annotate_and_type(&mut b, "a");
    a.merge(&b).unwrap();
    assert_eq!(
        a.blocks()
            .unwrap()
            .iter()
            .map(|b| b.id.as_str())
            .collect::<Vec<_>>(),
        ["b", "a"]
    );
    assert_eq!(a.block_view("a").unwrap().text, "See TAR!GET here.");
    assert_eq!(a.block_view("b").unwrap().text, "See TARGET elsewhere.");
    assert_eq!(
        a.comment_target("c").unwrap().attachments[0].owner,
        TargetOwner::Block("a".into())
    );
}

#[test]
fn join_then_split_after_reordering_keeps_both_text_sources() {
    let base = seed("See TARGET here.", "Another TARGET.");
    let mut a = base.fork();
    let mut b = base.fork();
    a.move_block("b", None, Some("a")).unwrap();
    a.join_blocks("b", "a").unwrap();
    let at = a
        .point("b", "Another TARGET.See TAR".encode_utf16().count())
        .unwrap();
    a.split_block("b", &at, "tail").unwrap();
    annotate_and_type(&mut b, "a");
    a.merge(&b).unwrap();
    assert_eq!(
        a.block_view("b").unwrap().text + a.block_view("tail").unwrap().text.as_str(),
        "Another TARGET.See TAR!GET here."
    );
    assert_eq!(
        a.comment_target("c")
            .unwrap()
            .attachments
            .iter()
            .map(|a| a.quote.as_str())
            .collect::<String>(),
        "TAR!GET"
    );
}

#[test]
fn comment_created_before_split_can_be_received_after_a_restart() {
    let base = seed("See TARGET here.", "Second");
    let mut a = base.fork();
    let mut b = base.fork();
    let at = a.point("a", 4).unwrap();
    a.split_block("a", &at, "tail").unwrap();
    annotate_and_type(&mut b, "a");
    let mut restarted = Document::load(&a.save()).unwrap();
    restarted.merge(&b).unwrap();
    assert_eq!(restarted.block_view("tail").unwrap().text, "TAR!GET here.");
    assert_eq!(
        restarted.comment_target("c").unwrap().attachments[0].owner,
        TargetOwner::Block("tail".into())
    );
}

#[test]
fn comment_boundaries_exclude_adjacent_typing_and_keep_interior_typing() {
    let mut d = seed("See TARGET here.", "Second");
    let selection = d.selection("a", 4, 10).unwrap();
    d.add_comment("c", "user", "body", &selection).unwrap();
    let start = d.point("a", 4).unwrap();
    d.insert_text(&start, "+").unwrap();
    let end = d.point("a", 11).unwrap();
    d.insert_text(&end, "+").unwrap();
    assert_eq!(
        d.comment_target("c").unwrap().attachments[0].quote,
        "TARGET"
    );
    let mid = d.point("a", 8).unwrap();
    d.insert_text(&mid, "!").unwrap();
    assert_eq!(
        d.comment_target("c").unwrap().attachments[0].quote,
        "TAR!GET"
    );
}

#[test]
fn typing_at_each_side_of_a_split_belongs_to_that_side() {
    let mut d = seed("LEFTRIGHT", "Second");
    let at = d.point("a", 4).unwrap();
    d.split_block("a", &at, "right").unwrap();
    let left = d.point("a", 4).unwrap();
    d.insert_text(&left, "<").unwrap();
    let right = d.point("right", 0).unwrap();
    d.insert_text(&right, ">").unwrap();
    assert_eq!(d.block_view("a").unwrap().text, "LEFT<");
    assert_eq!(d.block_view("right").unwrap().text, ">RIGHT");
}

#[test]
fn deleting_text_or_blocks_preserves_discussion_state() {
    let mut d = seed("See TARGET here.", "Second");
    let range = d.selection("a", 4, 10).unwrap();
    d.add_comment("c", "user", "body", &range).unwrap();
    let partial = d.selection("a", 4, 7).unwrap();
    d.delete_text(&partial).unwrap();
    assert_eq!(d.comment_target("c").unwrap().attachments[0].quote, "GET");
    let mut removed = d.fork();
    removed.delete_block("a").unwrap();
    assert_eq!(
        removed.comment_target("c").unwrap().state,
        TargetState::Hidden
    );
    let rest = d.selection("a", 4, 7).unwrap();
    d.delete_text(&rest).unwrap();
    assert_eq!(d.comment_target("c").unwrap().state, TargetState::Deleted);
    assert_eq!(d.comment("c").unwrap().state, DiscussionState::Open);
    d.resolve_comment("c", true).unwrap();
    assert_eq!(d.comment_target("c").unwrap().state, TargetState::Deleted);
}

#[test]
fn accepting_proposal_preserves_in_flight_comments_and_edits_on_its_text() {
    let mut base = seed("Before after.", "Second");
    let at = base.point("a", 7).unwrap();
    base.propose_insertion("p", "agent", "a", &at, "TARGET ")
        .unwrap();
    let mut authority = base.fork();
    let mut reviewer = base.fork();
    authority.accept_proposal("p").unwrap();
    let selection = reviewer.proposal_selection("p", 0, 6).unwrap();
    reviewer
        .add_comment("c", "user", "Keep this", &selection)
        .unwrap();
    let point = reviewer.proposal_point("p", 3).unwrap();
    reviewer.insert_text(&point, "!").unwrap();
    authority.merge(&reviewer).unwrap();
    assert_eq!(
        authority.block_view("a").unwrap().text,
        "Before TAR!GET after."
    );
    let target = authority.comment_target("c").unwrap();
    assert_eq!(target.attachments[0].quote, "TAR!GET");
    assert_eq!(target.attachments[0].owner, TargetOwner::Block("a".into()));
    let before = authority.heads();
    assert!(authority.accept_proposal("p").is_err());
    assert_eq!(authority.heads(), before);
}

#[test]
fn invalid_commands_are_atomic_and_references_are_checked() {
    let mut d = seed("😀 TARGET", "Second");
    let before = d.heads();
    assert!(d.point("a", 1).is_err());
    assert!(d.move_block("a", Some("a".into()), None).is_err());
    assert_eq!(d.heads(), before);
    assert_eq!(d.block_view("a").unwrap().text, "😀 TARGET");
    let foreign = seed("Unrelated", "Other").point("a", 1).unwrap();
    assert!(d.insert_text(&foreign, "bad").is_err());
    assert_eq!(d.heads(), before);
}

#[test]
fn concurrent_boundary_typing_never_expands_a_late_comment() {
    for _ in 0..80 {
        for boundary in [3, 9] {
            let base = seed("SeeTARGETafter", "TARGET");
            let mut writer = base.fork();
            writer
                .insert_text(&writer.point("a", boundary).unwrap(), "OUTSIDE")
                .unwrap();
            let mut reviewer = base.fork();
            reviewer
                .add_comment(
                    "late",
                    "Reviewer",
                    "Exactly selected",
                    &reviewer.selection("a", 3, 9).unwrap(),
                )
                .unwrap();
            for reverse in [false, true] {
                let mut merged = if reverse {
                    writer.fork()
                } else {
                    reviewer.fork()
                };
                merged
                    .merge(if reverse { &reviewer } else { &writer })
                    .unwrap();
                let quote: String = merged
                    .comment_target("late")
                    .unwrap()
                    .attachments
                    .iter()
                    .map(|a| a.quote.as_str())
                    .collect();
                assert_eq!(quote, "TARGET", "boundary={boundary}, reverse={reverse}");
            }
        }
    }
}

#[test]
fn malformed_and_truncated_native_archives_never_panic_or_bypass_validation() {
    let document = Document::with_id("corruption-corpus").unwrap();
    let bytes = document.save();
    for length in 0..bytes.len() {
        assert!(Document::load(&bytes[..length]).is_err());
    }
    let mut random = 0x7688_e24b_bdfa_2345_u64;
    for length in 0..256 {
        let input: Vec<u8> = (0..length)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                random as u8
            })
            .collect();
        assert!(Document::load(&input).is_err());
    }
    for index in 0..bytes.len() {
        let mut changed = bytes.clone();
        changed[index] ^= 0x80;
        if let Ok(loaded) = Document::load(&changed) {
            assert_eq!(
                loaded.view().unwrap(),
                Document::load(&loaded.save()).unwrap().view().unwrap()
            );
        }
    }
}
