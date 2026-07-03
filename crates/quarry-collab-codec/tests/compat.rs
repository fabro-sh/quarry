#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap to inspect fixture manifests"
)]

use quarry_collab_codec::{
    Node, Unsupported, apply_built, block_markdown_to_slate, build_nodes, review_markdown_to_slate,
    xmltext_to_slate,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;
use yrs::{Doc, OffsetKind, Options, Transact, WriteTxn, XmlTextRef};

#[derive(Deserialize)]
struct Manifest {
    cases: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    name: String,
    markdown: String,
    supported: bool,
    #[serde(default)]
    codec: Option<String>,
    canonical_plate: Option<Value>,
    observable_state: Option<Value>,
}

impl Fixture {
    /// Convert the fixture markdown to Slate nodes using whichever UI codec is
    /// the oracle for this case (the editor/collab codec by default, or the
    /// review codec for CriticMarkup cases).
    fn to_slate(&self) -> Result<Vec<Node>, Unsupported> {
        match self.codec.as_deref() {
            Some("review") => review_markdown_to_slate(&self.markdown),
            _ => block_markdown_to_slate(&self.markdown),
        }
    }
}

#[test]
fn slate_yjs_compat_fixtures_match_ui_oracle() {
    let fixture_root = fixture_root();
    let manifest: Manifest = read_json(fixture_root.join("manifest.json"));

    for case in manifest.cases {
        let fixture: Fixture = read_json(fixture_root.join("cases").join(format!("{case}.json")));
        if fixture.supported {
            assert_supported_fixture_matches(&fixture);
        } else {
            assert!(
                fixture.to_slate().is_err(),
                "unsupported fixture {} unexpectedly parsed",
                fixture.name
            );
        }
    }
}

#[test]
fn compat_manifest_covers_review_mark_intersections() {
    let fixture_root = fixture_root();
    let manifest: Manifest = read_json(fixture_root.join("manifest.json"));
    let cases = manifest.cases.into_iter().collect::<BTreeSet<_>>();
    let required = [
        "review-heading-suggestion",
        "review-list-comment",
        "review-table-suggestion",
        "review-blockquote-comment",
        "review-code-literal",
    ];

    for case in required {
        assert!(
            cases.contains(case),
            "missing required review compatibility fixture {case}"
        );
    }
}

fn assert_supported_fixture_matches(fixture: &Fixture) {
    let nodes = fixture
        .to_slate()
        .unwrap_or_else(|error| panic!("fixture {} should parse: {error}", fixture.name));
    assert_eq!(
        serde_json::to_value(&nodes).unwrap(),
        fixture
            .canonical_plate
            .clone()
            .unwrap_or_else(|| panic!("fixture {} missing canonicalPlate", fixture.name)),
        "fixture {} canonical Plate JSON drifted",
        fixture.name
    );

    let built = build_nodes(&nodes)
        .unwrap_or_else(|error| panic!("fixture {} should build: {error}", fixture.name));
    let doc = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        ..Default::default()
    });
    let root = {
        let mut txn = doc.transact_mut();
        let text = txn.get_or_insert_text("content");
        let root: &XmlTextRef = text.as_ref();
        apply_built(&mut txn, root, 0, &built);
        root.clone()
    };
    let txn = doc.transact();
    let Node::Element { children, .. } = xmltext_to_slate(&txn, &root)
        .unwrap_or_else(|error| panic!("fixture {} should read back: {error}", fixture.name))
    else {
        panic!("fixture {} did not read back as a fragment", fixture.name);
    };
    assert_eq!(
        serde_json::to_value(children).unwrap(),
        fixture
            .observable_state
            .clone()
            .unwrap_or_else(|| panic!("fixture {} missing observableState", fixture.name)),
        "fixture {} observable Yjs state drifted",
        fixture.name
    );
}

fn read_json<T: for<'de> Deserialize<'de>>(path: PathBuf) -> T {
    let json = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&json).unwrap_or_else(|error| {
        panic!(
            "failed to parse {} as fixture JSON: {error}",
            path.display()
        )
    })
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures/slate-yjs-compat")
}
