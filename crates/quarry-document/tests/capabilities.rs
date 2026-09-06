#![allow(clippy::unwrap_used, reason = "capability conformance fixtures")]
use quarry_document::{
    BlockContentModel, Command, CommandRequest, Document, SeedBlock, TargetState,
    block_capabilities, known_block_types,
};
use serde_json::json;

fn seed(id: &str, kind: &str, parent: Option<&str>, position: usize) -> SeedBlock {
    let capability = block_capabilities(kind).unwrap();
    let attrs = match kind {
        "img" => json!({"url":"/assets/example.png","alt":"Image"}),
        "mermaid" => json!({"code":"flowchart LR\nA --> B"}),
        "raw_markdown" => json!({"markdown":"[[TARGET]]"}),
        _ => json!({}),
    };
    SeedBlock {
        id: id.into(),
        kind: kind.into(),
        parent: parent.map(String::from),
        position,
        attrs: serde_json::from_value(attrs).unwrap(),
        text: if capability.content == BlockContentModel::Text {
            "TARGET 😀".into()
        } else {
            String::new()
        },
    }
}

#[test]
fn every_registered_block_supports_exact_subtree_decisions_and_late_comments_in_both_orders() {
    for kind in known_block_types() {
        for accept in [false, true] {
            for comment_first in [false, true] {
                let mut blocks = Vec::new();
                let parent = match kind {
                    "code_line" => {
                        blocks.push(seed("root", "code_block", None, 0));
                        Some("root")
                    }
                    "tr" => {
                        blocks.push(seed("root", "table", None, 0));
                        Some("root")
                    }
                    "td" | "th" => {
                        blocks.push(seed("root", "table", None, 0));
                        blocks.push(seed("row", "tr", Some("root"), 0));
                        Some("row")
                    }
                    _ => None,
                };
                blocks.push(seed("target", kind, parent, 0));
                let mut current = kind;
                let mut owner = "target".to_string();
                let mut count = 0;
                while let Some(child) = block_capabilities(current).unwrap().children.first() {
                    count += 1;
                    let id = format!("child{count}");
                    blocks.push(seed(&id, child, Some(&owner), 0));
                    owner = id;
                    current = child;
                }
                blocks.push(seed("neighbor", "p", None, 1));
                let base = Document::from_blocks(&blocks).unwrap();
                let late =
                    if block_capabilities(current).unwrap().content == BlockContentModel::Text {
                        Some(CommandRequest {
                            request_id: "late".into(),
                            base: base.heads().iter().map(ToString::to_string).collect(),
                            at: String::new(),
                            commands: vec![Command::AddComment {
                                id: "discussion".into(),
                                author: "Reviewer".into(),
                                body: "Keep this discussion".into(),
                                ranges: base.selection(&owner, 0, 6).unwrap(),
                            }],
                        })
                    } else {
                        None
                    };
                let mut document = base.fork();
                document
                    .propose_block_delete("decision", "Agent", "target")
                    .unwrap();
                if comment_first && let Some(late) = &late {
                    document.apply_request(late).unwrap();
                }
                if accept {
                    document.accept_proposal("decision").unwrap();
                } else {
                    document.reject_proposal("decision").unwrap();
                }
                if !comment_first && let Some(late) = &late {
                    document.apply_request(late).unwrap();
                }
                let view = document.view().unwrap();
                assert_eq!(
                    view.blocks.iter().any(|v| v.block.id == "target"),
                    !accept,
                    "{kind}"
                );
                assert!(
                    view.blocks
                        .iter()
                        .any(|v| v.block.id == "neighbor" && v.text == "TARGET 😀"),
                    "{kind}"
                );
                if late.is_some() {
                    assert_eq!(
                        view.comments[0].comment.body, "Keep this discussion",
                        "{kind}"
                    );
                    assert_eq!(
                        view.comments[0].target.state,
                        if accept {
                            TargetState::Hidden
                        } else {
                            TargetState::Attached
                        },
                        "{kind}"
                    );
                }
                assert_eq!(
                    Document::load(&document.save()).unwrap().view().unwrap(),
                    view,
                    "{kind}"
                );
            }
        }
    }
}
