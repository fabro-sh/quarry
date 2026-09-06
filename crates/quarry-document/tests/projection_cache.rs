#![allow(clippy::unwrap_used, reason = "explicit conformance fixtures")]
use quarry_document::{Command, CommandRequest, Document, SeedBlock};
use std::collections::BTreeMap;

fn fixture() -> Document {
    Document::from_blocks(
        &(0..8)
            .map(|n| SeedBlock {
                id: format!("p{n}"),
                kind: "p".into(),
                parent: None,
                position: n,
                attrs: BTreeMap::new(),
                text: "Same 😀 TARGET repeated text.".into(),
            })
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn compare_fresh(document: &Document) {
    document.validate().unwrap();
    let fresh = Document::load(&document.save()).unwrap();
    assert_eq!(document.view().unwrap(), fresh.view().unwrap());
    for block in document.view().unwrap().blocks {
        for offset in [0, block.text.encode_utf16().count()] {
            let point = document.point(&block.block.id, offset).unwrap();
            assert_eq!(point, fresh.point(&block.block.id, offset).unwrap());
            assert_eq!(
                document.locate_point(&point).unwrap(),
                fresh.locate_point(&point).unwrap()
            );
        }
    }
}

#[test]
fn incremental_join_after_concurrent_typing_invalidates_every_deleted_source() {
    let base = fixture();
    let mut authority = base.fork();
    let mut reader = base.fork();
    compare_fresh(&reader);
    let initial = authority.heads();
    authority
        .insert_text(&authority.point("p2", 8).unwrap(), "!")
        .unwrap();
    reader
        .merge_changes(
            &authority.save_after(&initial).unwrap(),
            &initial,
            &authority.heads(),
        )
        .unwrap();
    compare_fresh(&reader);
    let before_join = authority.heads();
    authority
        .apply_request(&CommandRequest {
            request_id: "delayed-delete-and-join".into(),
            base: base.heads().iter().map(ToString::to_string).collect(),
            at: String::new(),
            commands: vec![
                Command::DeleteText {
                    ranges: base.selection("p0", 5, 29).unwrap(),
                },
                Command::DeleteText {
                    ranges: base.selection("p2", 0, 5).unwrap(),
                },
                Command::DeleteBlock { block: "p1".into() },
                Command::JoinBlocks {
                    left: "p0".into(),
                    right: "p2".into(),
                },
            ],
        })
        .unwrap();
    compare_fresh(&authority);
    reader
        .merge_changes(
            &authority.save_after(&before_join).unwrap(),
            &before_join,
            &authority.heads(),
        )
        .unwrap();
    assert_eq!(reader.view().unwrap(), authority.view().unwrap());
    compare_fresh(&reader);
}

#[test]
fn incremental_marks_on_multiple_sources_and_their_undo_match_fresh_archives() {
    for deletion in [false, true] {
        let mut authority = fixture();
        let mut reader = authority.fork();
        compare_fresh(&reader);
        let before = authority.heads();
        let commands = ["p0", "p2", "p5"].map(|block| {
            let ranges = authority.selection(block, 8, 14).unwrap();
            if deletion {
                Command::DeleteText { ranges }
            } else {
                Command::Format {
                    ranges,
                    name: "bold".into(),
                    value: true.into(),
                }
            }
        });
        authority.apply(&commands).unwrap();
        let after = authority.heads();
        let delta = authority.save_after(&before).unwrap();
        reader.merge_changes(&delta, &before, &after).unwrap();
        assert_eq!(reader.view().unwrap(), authority.view().unwrap());
        compare_fresh(&reader);
        // Duplicate delivery must not introduce or discard derived changes.
        reader.merge_changes(&delta, &before, &after).unwrap();
        compare_fresh(&reader);
        authority
            .apply(&[Command::Revert {
                before: before.iter().map(ToString::to_string).collect(),
                after: after.iter().map(ToString::to_string).collect(),
            }])
            .unwrap();
        reader
            .merge_changes(
                &authority.save_after(&after).unwrap(),
                &after,
                &authority.heads(),
            )
            .unwrap();
        assert_eq!(reader.view().unwrap(), authority.view().unwrap());
        compare_fresh(&reader);
    }
}

#[test]
fn incremental_remote_changes_match_full_merges_with_warm_caches_and_local_typing() {
    let mut authority = fixture();
    let mut browser = authority.fork();
    for iteration in 0..24 {
        let base = authority.heads();
        compare_fresh(&browser);
        browser
            .insert_text(&browser.point("p0", 0).unwrap(), "local 😀 ")
            .unwrap();
        authority
            .insert_text(&authority.point("p1", 0).unwrap(), "remote ")
            .unwrap();
        authority
            .format(
                &authority.selection("p1", 0, 6).unwrap(),
                "bold",
                &true.into(),
            )
            .unwrap();
        let tail = format!("tail{iteration}");
        authority
            .apply_request(&CommandRequest {
                request_id: format!("split{iteration}"),
                base: authority.heads().iter().map(ToString::to_string).collect(),
                at: String::new(),
                commands: vec![Command::SplitBlock {
                    block: "p2".into(),
                    at: authority.point("p2", 5).unwrap(),
                    new_block: tail.clone(),
                }],
            })
            .unwrap();
        let heads = authority.heads();
        let delta = authority.save_after(&base).unwrap();
        let mut full = browser.fork();
        full.merge(&authority).unwrap();
        browser.merge_changes(&delta, &base, &heads).unwrap();
        assert_eq!(browser.view().unwrap(), full.view().unwrap());
        compare_fresh(&browser);
        let unchanged = browser.save();
        browser.merge_changes(&delta, &base, &heads).unwrap();
        assert_eq!(browser.save(), unchanged);
        authority.merge(&browser).unwrap();
        let base = authority.heads();
        authority
            .apply_request(&CommandRequest {
                request_id: format!("join{iteration}"),
                base: base.iter().map(ToString::to_string).collect(),
                at: String::new(),
                commands: vec![Command::JoinBlocks {
                    left: "p2".into(),
                    right: tail,
                }],
            })
            .unwrap();
        browser
            .merge_changes(
                &authority.save_after(&base).unwrap(),
                &base,
                &authority.heads(),
            )
            .unwrap();
        compare_fresh(&browser);
        assert_eq!(browser.view().unwrap(), authority.view().unwrap());
    }
}

#[test]
fn incomplete_foreign_and_corrupt_incremental_history_roll_back_without_poisoning_caches() {
    let mut browser = fixture();
    compare_fresh(&browser);
    let base = browser.heads();
    let before = browser.save();
    let mut remote = browser.fork();
    remote
        .insert_text(&remote.point("p0", 0).unwrap(), "first ")
        .unwrap();
    let intermediate = remote.heads();
    remote
        .insert_text(&remote.point("p0", 0).unwrap(), "second ")
        .unwrap();
    let heads = remote.heads();
    assert!(
        browser
            .merge_changes(&remote.save_after(&intermediate).unwrap(), &base, &heads)
            .is_err()
    );
    assert_eq!(browser.save(), before);
    assert!(browser.merge_changes(&[], &intermediate, &heads).is_err());
    assert!(browser.merge_changes(&[1, 2, 3, 4], &base, &heads).is_err());
    let foreign = Document::with_id(&browser.id().unwrap()).unwrap();
    assert!(
        browser
            .merge_changes(&foreign.save_after(&[]).unwrap(), &base, &foreign.heads())
            .is_err()
    );
    assert_eq!(browser.save(), before);
    compare_fresh(&browser);
    browser
        .merge_changes(&remote.save_after(&base).unwrap(), &base, &heads)
        .unwrap();
    assert_eq!(browser.view().unwrap(), remote.view().unwrap());
}

#[test]
fn warm_projections_match_fresh_archives_through_all_text_mutations_and_rollback() {
    let mut doc = fixture();
    compare_fresh(&doc);
    let base = doc.heads();
    let mut random = 0xabc0_1234_u64;
    for iteration in 0..64 {
        random = random
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let id = format!("p{}", (random >> 32) % 8);
        let before = doc.heads();
        let at = doc.point(&id, 0).unwrap();
        doc.insert_text(&at, "X").unwrap();
        compare_fresh(&doc);
        let range = doc.selection(&id, 0, 1).unwrap();
        doc.format(&range, "bold", &true.into()).unwrap();
        compare_fresh(&doc);
        if iteration % 2 == 0 {
            doc.delete_text(&range).unwrap();
        } else {
            let at = doc.point(&id, 1).unwrap();
            let tail = format!("tail{iteration}");
            doc.split_block(&id, &at, &tail).unwrap();
            compare_fresh(&doc);
            doc.join_blocks(&id, &tail).unwrap();
        }
        compare_fresh(&doc);
        let after = doc.heads();
        doc.apply_request(&CommandRequest {
            request_id: format!("undo{iteration}"),
            base: after.iter().map(ToString::to_string).collect(),
            at: String::new(),
            commands: vec![Command::Revert {
                before: before.iter().map(ToString::to_string).collect(),
                after: after.iter().map(ToString::to_string).collect(),
            }],
        })
        .unwrap();
        compare_fresh(&doc);
    }
    let mut fork = doc.fork();
    let at = fork.point("p0", 0).unwrap();
    fork.insert_text(&at, "remote ").unwrap();
    compare_fresh(&fork);
    doc.merge(&fork).unwrap();
    compare_fresh(&doc);
    assert!(doc.contains_history(&base));
    let historical = doc.fork_at(&base).unwrap();
    compare_fresh(&historical);
    assert!(!historical.contains_history(&doc.heads()));
    let unchanged = doc.view().unwrap();
    let at = doc.point("p0", 0).unwrap();
    assert!(
        doc.apply(&[
            Command::InsertText {
                at,
                text: "must roll back".into()
            },
            Command::DeleteBlock {
                block: "missing".into()
            }
        ])
        .is_err()
    );
    assert_eq!(doc.view().unwrap(), unchanged);
    compare_fresh(&doc);
}

#[test]
fn incremental_archives_retain_exact_native_state_after_each_edit_and_merge() {
    let mut doc = fixture();
    let mut bytes = doc.save();
    let mut heads = doc.heads();
    for n in 0..32 {
        let point = doc.point("p0", 0).unwrap();
        doc.insert_text(&point, "😀").unwrap();
        let appended = doc.save_after(&heads).unwrap();
        assert!(!appended.is_empty());
        bytes.extend(appended);
        heads = doc.heads();
        let restored = Document::load(&bytes).unwrap();
        assert_eq!(restored.view().unwrap(), doc.view().unwrap());
        assert!(doc.save_after(&heads).unwrap().is_empty());
        if n % 4 == 0 {
            let mut peer = doc.fork();
            let point = peer.point("p1", 0).unwrap();
            peer.insert_text(&point, "remote").unwrap();
            doc.merge(&peer).unwrap();
        }
    }
    bytes.extend(doc.save_after(&heads).unwrap());
    assert_eq!(
        Document::load(&bytes).unwrap().view().unwrap(),
        doc.view().unwrap()
    );
    assert!(doc.save_after(&Document::new().unwrap().heads()).is_err());
}

#[test]
fn record_caches_cannot_hide_swapped_native_map_keys() {
    use automerge::{AutoCommit, ROOT, ReadDoc, transaction::Transactable};
    let mut doc = fixture();
    compare_fresh(&doc);
    let before = doc.save();
    let base = doc.heads();
    let mut corrupt = AutoCommit::load(&doc.save()).unwrap();
    let blocks = corrupt.get(ROOT, "blocks").unwrap().unwrap().1;
    let first = corrupt
        .get(&blocks, "p0")
        .unwrap()
        .unwrap()
        .0
        .to_str()
        .unwrap()
        .to_string();
    let second = corrupt
        .get(&blocks, "p1")
        .unwrap()
        .unwrap()
        .0
        .to_str()
        .unwrap()
        .to_string();
    corrupt.put(&blocks, "p0", second).unwrap();
    corrupt.put(&blocks, "p1", first).unwrap();
    assert!(Document::load(&corrupt.save()).is_err());
    let heads = corrupt.get_heads();
    assert!(
        doc.merge_changes(&corrupt.save_after(&base), &base, &heads)
            .is_err()
    );
    assert_eq!(doc.save(), before);
    compare_fresh(&doc);
}
