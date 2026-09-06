"""Run counterexamples through Quarry's production codec, then remove the temporary test target."""
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
source = ROOT / 'crates/quarry-collab-codec/tests/session_doc.rs'
target = source.with_name('review_engines_spike.rs')
assert not target.exists(), 'Refusing to overwrite an existing test file'
extra = r'''

#[test]
fn spike_comment_only_link_reconcile_replaces_target_identity() {
    for linked in [false, true] {
        let mut rows = vec![block("b1", None, 0, "p", "See docs and TARGET here.")];
        if linked { rows[0].links = vec![LinkRange { start: 4, end: 8, url: "https://example.test".into() }]; }
        let doc = seeded_doc(&rows, &[]);
        let cursor = place_sticky(&doc, "b1", 1);
        let before = doc_image(&rows, &[]);
        let desired = doc_image(&rows, &[comment("agent", "b1", 13, 19)]);
        reconcile(&doc, &before, &desired);
        let projection = project(&doc_children(&doc));
        assert_eq!(projection.rows, rows, "Document text and formatting did not change");
        assert!(projection.anchors.iter().any(|a| a.id == "agent"));
        assert_eq!(resolve_sticky(&doc, &cursor) == Some(1), !linked,
            "Only the linked paragraph loses existing character identity on a comment-only change");
    }
}

#[test]
fn spike_comment_on_proposed_insert_is_discarded_by_projection() {
    let nodes = vec![Node::element("p", [("id".into(), json!("b1"))].into_iter().collect(),
        vec![Node::text("Proposed TARGET", [
            ("comment_c1".into(), json!(true)),
            ("suggestion_s1".into(), json!({"type":"insert","userId":"ai:spike","createdAt":1}))
        ].into_iter().collect())])];
    let projection = project(&nodes);
    assert!(projection.anchors.iter().any(|a| a.id == "s1"));
    assert!(!projection.anchors.iter().any(|a| a.id == "c1"));
}
'''
try:
    target.write_text(source.read_text() + extra)
    run = subprocess.run(['cargo','test','--offline','-p','quarry-collab-codec','--test','review_engines_spike','spike_','--','--nocapture'],
                         cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    (HERE / 'baseline-results.log').write_text(run.stdout)
    print(run.stdout)
    raise SystemExit(run.returncode)
finally:
    target.unlink(missing_ok=True)
