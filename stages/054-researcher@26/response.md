{
  "category": "memory-and-unsafe",
  "title": "Unbounded YAML frontmatter recursion aborts the daemon via anonymous tmp-document writes",
  "rationale": "The workspace forbids unsafe_code and the crate contains zero unsafe blocks, so buffer overflows, use-after-free, type confusion, and unchecked unsafe FFI are structurally absent; turso is pure Rust. All slicing in links.rs, search.rs, and blocks.rs is boundary-checked (ASCII marker offsets, char-boundary adjustment, validated UTF-16 anchors with u32::try_from), DB integer casts feed only API payloads and sort keys, and the links graph is an iterative depth-capped BFS. The one complete attack path under this lens is stack exhaustion through unbounded recursion: serde_yaml 0.9 builds serde_yaml::Value recursively with no nesting limit, and the in-crate merge_json recurses at the same attacker-controlled depth, reachable from the anonymous tmp-document write path whose 1 MiB cap bounds bytes but not nesting (~1 byte per level). A single request drives the parser past the thread stack and aborts the whole process. The exact crashing depth could not be confirmed without executing (forbidden by review rules), so confidence is MEDIUM.",
  "findings": [
    {
      "category": "memory-and-unsafe",
      "title": "Unbounded recursion in Markdown frontmatter YAML parse causes unauthenticated process abort",
      "rationale": "split_markdown_frontmatter parses attacker-controlled frontmatter with serde_yaml::from_str into a recursive serde_yaml::Value with no nesting-depth limit, and merge_json recurses at the same attacker-controlled depth. The anonymous tmp-document write path validates only byte length (1 MiB) and UTF-8 before reaching this parse, and nesting costs about one byte per level in YAML flow style, so a single request drives the thread stack to exhaustion. Stack overflow in Rust is a process abort, not a catchable panic, so the whole daemon goes down and the request can be replayed after every restart. The identical root control is also reachable from authenticated library put_document, reported once under the root control. The exact crashing depth could not be confirmed without executing, so confidence is MEDIUM.",
      "ruleId": "dos.unbounded-recursion",
      "identity": {
        "anchor": "markdown-frontmatter-yaml-parse"
      },
      "severity": "HIGH",
      "difficulty": "LOW",
      "confidence": "MEDIUM",
      "file": "crates/quarry-storage/src/lib.rs",
      "line": 1887,
      "symbol": "split_markdown_frontmatter",
      "snippet": "    let frontmatter = serde_json::to_value(serde_yaml::from_str::<serde_yaml::Value>(yaml)?)?;",
      "impact": "An unauthenticated attacker aborts the entire Quarry daemon process with one request. Frontmatter YAML nesting depth is fully attacker-controlled (about 1 byte per level in flow style, so up to ~1,000,000 levels within the 1 MiB tmp-document byte cap), while serde_yaml 0.9 builds serde_yaml::Value by recursion with no nesting limit and the in-crate merge_json recurses at the same depth. The resulting stack exhaustion cannot be caught as a panic; it aborts the process, denying service to every user and library the daemon hosts, and the crashing request can simply be replayed after each restart.",
      "evidence": [
        "crates/quarry-storage/src/tmp_documents.rs:58 — put_tmp_document is the anonymous internet-facing write path: the 32-hex path secret is the only authorization, and create_tmp_document (tmp_documents.rs:80) mints that secret for any caller, so the content bytes are fully attacker-controlled.",
        "crates/quarry-storage/src/tmp_documents.rs:181 — the only validation before storage is validate_tmp_markdown_write(content, metadata, content_type).",
        "crates/quarry-storage/src/tmp_documents.rs:617 — validate_tmp_markdown_bytes enforces only a 1 MiB byte cap (TMP_DOCUMENT_MARKDOWN_MAX_BYTES, tmp_documents.rs:5) and UTF-8 validity; it does not inspect YAML structure or nesting depth, so the guard is ineffective against deep-nesting payloads (nesting costs ~1 byte per level).",
        "crates/quarry-storage/src/tmp_documents.rs:217 — the write transaction passes the attacker content into store.insert_version_conn(conn, &doc_id, &tx.id, content, metadata, &content_type); tmp content types are forced to Markdown (normalize_tmp_markdown_content_type, tmp_documents.rs:26), so the frontmatter path always runs.",
        "crates/quarry-storage/src/lib.rs:684 — insert_version_conn calls merge_markdown_frontmatter_metadata(&content, metadata, content_type) on every version insert.",
        "crates/quarry-storage/src/lib.rs:1861 — merge_markdown_frontmatter_metadata calls markdown_frontmatter_metadata for Markdown content, then merge_json merges the result; merge_json (lib.rs:1842-1851) itself recurses once per JSON nesting level, mirroring the attacker-controlled depth.",
        "crates/quarry-storage/src/lib.rs:1882 — split_markdown_frontmatter slices the frontmatter block out of the attacker text using only the ASCII --- markers; there is no depth check before parsing.",
        "crates/quarry-storage/src/lib.rs:1887 — serde_yaml::from_str::<serde_yaml::Value>(yaml) parses the attacker-controlled frontmatter; serde_yaml 0.9 (workspace Cargo.toml) constructs Value via recursive deserialization with no nesting-depth limit, and the serde_json::to_value conversion on the same line recurses again over the same depth, so a deeply nested sequence like key: [[[...]]] drives the thread stack to exhaustion and aborts the process.",
        "crates/quarry-storage/src/documents.rs:119 — the same root control is reachable by authenticated library writes: put_document passes uncapped caller content into the identical insert_version_conn call, confirming this is one vulnerable control with two entry points (reported once under the root control)."
      ],
      "exploitScenarios": [
        "Attacker crafts a Markdown body of well under 1 MiB that starts with ---\\n, contains a single YAML key whose value is a flow sequence nested hundreds of thousands of levels deep (e.g. a: [[[[...]]]]), and closes with \\n---\\n.",
        "Attacker POSTs the body to the anonymous tmp-document create/put endpoint (or, if authenticated, PUTs it as a Markdown library document).",
        "The store validates only byte length and UTF-8, then insert_version_conn invokes split_markdown_frontmatter on the body.",
        "serde_yaml::from_str::<serde_yaml::Value> recurses once per nesting level with no depth cap; the stack overflows and the Rust runtime aborts the whole daemon process.",
        "All libraries, sessions, and other users served by the process go down; the attacker replays the same request after every restart to keep the service down."
      ],
      "preconditions": [
        "The tmp-document API is exposed to the attacker (it is the documented anonymous surface), or the attacker has any authenticated library write access.",
        "The request body stays within the 1 MiB tmp-document cap (a deep-nesting payload needs only a few hundred KB) or uses the uncapped library put path.",
        "The document is written with a Markdown content type, which is mandatory for tmp documents.",
        "serde_yaml 0.9 is the dependency (pinned in the workspace manifest) and provides no nesting-depth limit."
      ],
      "recommendations": [
        "Root cause: bound the YAML nesting depth before/while parsing frontmatter — e.g. pre-scan the frontmatter slice and reject bracket/indent nesting beyond a small fixed limit (32–64 levels), or deserialize through a depth-tracking serde Deserializer wrapper that errors instead of recursing; make merge_json iterative or depth-limited as well.",
        "Hardening: cap the frontmatter block's byte size independently of the document body, and treat any frontmatter parse failure as InvalidInput without ever recursing into attacker-shaped data (also drop or replace serde_yaml, whose unsafe-libyaml parser and unlimited recursion are the underlying exposure).",
        "Regression test: a property/unit test that PUTs a tmp document whose frontmatter contains nesting at, say, 100,000 levels and asserts the store returns an InvalidInput error and the process stays alive."
      ]
    }
  ]
}