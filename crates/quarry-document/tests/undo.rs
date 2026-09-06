#![allow(clippy::unwrap_used, reason = "explicit conformance fixtures")]
use quarry_document::{Command, CommandRequest, Document, SeedBlock, TargetState};

fn fixture() -> Document {
    let mut d = Document::from_blocks(&[SeedBlock {
        id: "p".into(),
        kind: "p".into(),
        parent: None,
        position: 0,
        attrs: Default::default(),
        text: "😀 TARGET end".into(),
    }])
    .unwrap();
    d.add_comment(
        "review",
        "Reviewer",
        "Keep the original identity",
        &d.selection("p", 3, 9).unwrap(),
    )
    .unwrap();
    d
}
fn text(d: &Document) -> String {
    d.view()
        .unwrap()
        .blocks
        .iter()
        .map(|b| b.text.as_str())
        .collect()
}
fn command(d: &mut Document, id: &str, commands: Vec<Command>) {
    d.apply_request(&CommandRequest {
        request_id: id.into(),
        base: d.heads().iter().map(ToString::to_string).collect(),
        commands,
        at: "2026-09-05T12:00:00Z".into(),
    })
    .unwrap();
}

#[test]
fn deletion_undo_and_redo_restore_the_original_comment_after_reload() {
    let mut d = fixture();
    let before = d.heads();
    let ranges = d.selection("p", 3, 9).unwrap();
    command(&mut d, "delete", vec![Command::DeleteText { ranges }]);
    let after = d.heads();
    assert_eq!(text(&d), "😀  end");
    assert_eq!(
        d.comment_target("review").unwrap().state,
        TargetState::Deleted
    );
    d = Document::load(&d.save()).unwrap();
    d.revert(&before, &after).unwrap();
    assert_eq!(text(&d), "😀 TARGET end");
    assert_eq!(
        d.comment_target("review")
            .unwrap()
            .attachments
            .iter()
            .map(|a| a.quote.as_str())
            .collect::<String>(),
        "TARGET"
    );
    let undone = d.heads();
    d.revert(&after, &undone).unwrap();
    assert_eq!(text(&d), "😀  end");
    assert_eq!(
        d.comment_target("review").unwrap().state,
        TargetState::Deleted
    );
}

#[test]
fn split_move_and_join_undo_keep_sources_and_late_review() {
    for action in ["split", "move", "join"] {
        let mut d = fixture();
        if action != "split" {
            d.split_block("p", &d.point("p", 6).unwrap(), "tail")
                .unwrap();
        }
        let before = d.heads();
        let sources: Vec<_> = d
            .view()
            .unwrap()
            .blocks
            .iter()
            .flat_map(|b| b.block.segments.iter().map(|s| s.source.clone()))
            .collect();
        match action {
            "split" => d
                .split_block("p", &d.point("p", 6).unwrap(), "tail")
                .unwrap(),
            "move" => d.move_block("tail", None, Some("p")).unwrap(),
            "join" => d.join_blocks("p", "tail").unwrap(),
            _ => unreachable!(),
        }
        let after = d.heads();
        let mut late = d.fork_at(&before).unwrap();
        late.add_comment(
            "late",
            "Agent",
            "Delayed",
            &late.selection("p", 3, 6).unwrap(),
        )
        .unwrap();
        d.merge(&late).unwrap();
        let before_undo = d.heads();
        d.revert(&before, &after).unwrap();
        assert_eq!(text(&d), "😀 TARGET end");
        assert_eq!(
            d.comment_target("late").unwrap().state,
            TargetState::Attached
        );
        assert!(
            d.view().unwrap().blocks.iter().all(|b| b
                .block
                .segments
                .iter()
                .all(|s| sources.contains(&s.source)))
        );
        let undone = d.heads();
        d.revert(&before_undo, &undone).unwrap();
        assert_eq!(
            d.blocks().unwrap().len(),
            if action == "join" { 1 } else { 2 }
        );
        assert_eq!(
            d.comment_target("review").unwrap().state,
            TargetState::Attached
        );
    }
}

#[test]
fn concurrent_insertions_inside_deleted_text_survive_both_delivery_orders() {
    for reverse in [false, true] {
        let base = fixture();
        let mut deletion = base.fork();
        deletion
            .delete_text(&deletion.selection("p", 3, 9).unwrap())
            .unwrap();
        let mut typing = base.fork();
        typing
            .insert_text(&typing.point("p", 6).unwrap(), "NEW")
            .unwrap();
        let mut result = if reverse {
            typing.fork()
        } else {
            deletion.fork()
        };
        result
            .merge(if reverse { &deletion } else { &typing })
            .unwrap();
        assert_eq!(text(&result), "😀 NEW end");
        result.revert(&base.heads(), &deletion.heads()).unwrap();
        assert_eq!(text(&result), "😀 TARNEWGET end");
    }
}

#[test]
fn undo_does_not_restore_another_authors_independent_deletion() {
    let base = fixture();
    let mut a = base.fork();
    a.delete_text(&a.selection("p", 3, 9).unwrap()).unwrap();
    let a_heads = a.heads();
    let mut b = base.fork();
    b.delete_text(&b.selection("p", 4, 8).unwrap()).unwrap();
    a.merge(&b).unwrap();
    a.revert(&base.heads(), &a_heads).unwrap();
    assert_eq!(text(&a), "😀 TT end");
}

#[test]
fn undo_typing_preserves_later_typing_and_reports_removed_targets() {
    let mut d = fixture();
    let before = d.heads();
    d.insert_text(&d.point("p", 6).unwrap(), "local").unwrap();
    let after = d.heads();
    d.add_comment(
        "new",
        "Agent",
        "new letters",
        &d.selection("p", 6, 11).unwrap(),
    )
    .unwrap();
    d.insert_text(&d.point("p", 8).unwrap(), "REMOTE").unwrap();
    let before_undo = d.heads();
    d.revert(&before, &after).unwrap();
    assert_eq!(text(&d), "😀 TARREMOTEGET end");
    let undone = d.heads();
    d.revert(&before_undo, &undone).unwrap();
    assert_eq!(text(&d), "😀 TARloREMOTEcalGET end");
    assert_eq!(
        d.comment_target("new").unwrap().state,
        TargetState::Attached
    );
}

#[test]
fn conflicting_later_structure_rejects_undo_atomically() {
    let mut d = fixture();
    let before = d.heads();
    d.set_block("p", "h1", Default::default()).unwrap();
    let after = d.heads();
    d.set_block("p", "h2", Default::default()).unwrap();
    let saved = d.save();
    assert!(d.revert(&before, &after).is_err());
    assert_eq!(d.save(), saved);
}
