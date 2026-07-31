{
  "findings": [
    {
      "category": "authorization",
      "title": "Unverified review attribution lets any collab peer forge comment/suggestion authorship",
      "rationale": "The checkpoint pipeline admits peer-written review attribution (`by`/`at`/`userId`) into the canonical review record with only JSON shape validation and no binding to the connecting peer's identity, so any editor can persist comments and suggestions under an arbitrary author and timestamp that are then broadcast, served by the review API, and exported to the document endmatter.",
      "file": "crates/quarry-collab-codec/src/session_doc.rs",
      "line": 1036,
      "ruleId": "improper-authorization.unverified-authorship-attribution",
      "identity": {
        "anchor": "review-attribution-acceptance"
      },
      "severity": "MEDIUM",
      "difficulty": "MEDIUM",
      "confidence": "HIGH",
      "symbol": "read_review_section",
      "snippet": "        if let Ok(entry) = serde_json::from_value::<crate::ReviewMetaEntry>(json) {",
      "impact": "Any collab peer (anyone holding a valid invite token or tmp capability URL) can forge the author and timestamps of review comments and suggestions. The forged attribution is committed to the canonical block_review_items record at checkpoint, broadcast to all session peers, served to humans and AI agents through the review API, and exported into the document's markdown endmatter. An attacker can impersonate a trusted reviewer or AI agent (e.g. by: \"ai:claude\") so a victim accepts a malicious suggestion believing it came from that identity, and can backdate or falsify the persisted review audit trail.",
      "evidence": [
        "crates/quarry-server/src/collab.rs:100 — every peer WebSocket frame is applied to the shared Yjs document via DefaultProtocol.handle with no filtering, so any authenticated collab peer can write the `review` root map and arbitrary `comment_*`/`suggestion_*` marks (boundary crossing into the codec's input).",
        "crates/quarry-collab-codec/src/session_doc.rs:1029 — read_review_section iterates every entry of the peer-writable `comments`/`suggestions` Yjs sub-maps during checkpoint projection.",
        "crates/quarry-collab-codec/src/session_doc.rs:1036 — entries are accepted solely on JSON shape via serde_json::from_value; `by`, `at`, and `editedAt` pass verbatim into ReviewMetaEntry with no comparison against the peer's authenticated identity (guard checked: serde shape validation only, which says nothing about authorship).",
        "crates/quarry-collab-codec/src/session_doc.rs:798-803 — parallel admission point in classify_marks: the peer-written suggestion mark's `userId` and `createdAt` are copied verbatim into SessionAnchorKind::Suggestion.by/at_ms; guards here are also shape-only (value.get(\"userId\").and_then(Value::as_str)).",
        "crates/quarry-server/src/session.rs:904-907 — every debounced checkpoint ingests the peer-written map into ReviewMeta via read_review_meta_from_map with no further verification.",
        "crates/quarry-server/src/session.rs:1303 — browser_created_item persists the peer-chosen attribution as the canonical record: author: meta_entry.map(|entry| entry.by.clone()).or(anchor_author); session.rs:1287-1290 likewise take created_at from the peer-supplied entry.at.",
        "crates/quarry-server/src/session.rs:1357 — reply_item persists author: Some(entry.by.clone()) for comment replies, again with no identity binding.",
        "crates/quarry-server/src/session.rs:782 and 843 — the reconciled items become the committed canonical review state for the document, and crates/quarry-server/src/review.rs:381/412/438 serve entry.by to review-API consumers (humans and agents) as the comment/suggestion author.",
        "Guard check: repository-wide search of crates/quarry-server/src found invite-token/capability-secret authentication for access, but no per-user identity bound to collab connections, and no code anywhere overwrites or validates `by`/`userId` against the connecting peer."
      ],
      "exploitScenarios": [
        "Obtain collab access to a target document (a valid invite token or tmp capability URL, i.e. ordinary editor access).",
        "Connect to the document's collab WebSocket with a stock Yjs client instead of the browser.",
        "Send a Yjs update that inserts a `comment_<id>` mark on a victim's text range plus a `review` root map entry `comments.<id> = {by: \"alice\", at: \"<backdated timestamp>\", body: \"...\"}` (or a `suggestion_<id>` mark with userId \"ai:claude\" plus a matching `suggestions.<id>` entry proposing a malicious edit).",
        "Trigger or await the debounced checkpoint; the server projects the doc, and read_review_meta_from_map/classify_marks accept the forged entry verbatim.",
        "reconcile_review_items creates a canonical BlockReviewItem with author \"alice\"/\"ai:claude\" and the attacker-chosen created_at, committing it to storage.",
        "Other users and review-consuming agents see the comment/suggestion as authored by the impersonated identity in the live room, the review API, and the exported markdown endmatter, and act on it under false trust."
      ],
      "preconditions": [
        "Attacker holds collab/edit access to the target document (invite token or tmp capability URL).",
        "The document uses the rows-backed session checkpoint flow that persists block_review_items (the default live-session path).",
        "The system has no per-user identity bound to collab connections, so the server cannot distinguish the attacker's claimed `by` from a genuine one (verified: none exists in the codebase)."
      ],
      "recommendations": [
        "Root cause: bind attribution to the connection at checkpoint — have the server overwrite every peer-supplied `by`/`userId` with the identity established (or display name claimed and server-echoed) when the collab session was joined, and set `at`/`created_at`/`editedAt` from the server clock, ignoring peer-supplied timestamps, before reconcile_review_items runs.",
        "Hardening: treat the `review` root map as server-owned state — strip or reject peer Yjs updates that write it directly, deriving it only from server-validated review operations, or namespace each peer's entries under a server-assigned key.",
        "Regression test: a checkpoint test in which a peer-written review map entry carries by: \"victim\" and a backdated `at`; assert the committed BlockReviewItem's author and created_at reflect the connection identity and server clock rather than the supplied values, covering both the read_review_meta_from_map and classify_marks (userId) admission paths."
      ]
    }
  ]
}