{
  "findings": [
    {
      "category": "dos",
      "title": "Unbounded-depth YAML frontmatter parsing crashes the daemon via serde_yaml stack overflow",
      "rationale": "Every Markdown version insert funnels attacker-controlled document bytes into split_markdown_frontmatter, which passes the raw frontmatter slice to serde_yaml 0.9.34 (deprecated, no nesting-depth limit, unsafe-libyaml FFI). Deserializing serde_yaml::Value recurses per nesting level, so a well-formed ~1 MiB document with hundreds of thousands of nested flow sequences overflows the worker thread stack and aborts the whole process — an uncatchable crash, not a normal error. The anonymous POST /v1/tmp/documents route reaches this sink with no authentication and only a 1 MiB byte cap and UTF-8 check as guards, neither of which bounds nesting depth; the authenticated library put_document path reaches the same sink with no size cap at all. Impact is bounded to availability (restartable process crash), hence MEDIUM severity; exploitation is a single curl, hence LOW difficulty; confidence is MEDIUM because the claim rests on serde_yaml's documented recursive deserialization behavior, which I could not execute to confirm under the read-only rules of this review.",
      "file": "crates/quarry-storage/src/lib.rs",
      "line": 1887,
      "symbol": "split_markdown_frontmatter",
      "ruleId": "dos.unbounded-recursion",
      "identity": {
        "anchor": "markdown-frontmatter-yaml-parse"
      },
      "severity": "MEDIUM",
      "difficulty": "LOW",
      "confidence": "MEDIUM",
      "impact": "An anonymous remote attacker crashes the entire quarry daemon process (uncatchable stack overflow / abort) with a single HTTP request by submitting a Markdown tmp document whose YAML frontmatter is nested hundreds of thousands of levels deep. serde_yaml 0.9.34 (deprecated, backed by unsafe-libyaml FFI) deserializes serde_yaml::Value recursively with no depth limit, so ~500k nested flow sequences that fit inside the 1 MiB tmp-document cap overflow the worker thread stack. All connected users lose service; an attacker can repeat the request to keep the daemon down. The same sink is reachable by any authenticated library writer via put_document, which has no content size cap at all.",
      "evidence": [
        "crates/quarry-server/src/lib.rs:354-357 — installs POST /v1/tmp/documents -> create_tmp_document with only a DefaultBodyLimit layer; the only middlewares (lib.rs:215-217: api_error_envelope, request_tracing, security_headers) perform no authentication, so this route is anonymous.",
        "crates/quarry-server/src/lib.rs:117-118 — TMP_DOCUMENT_HTTP_BODY_LIMIT = TMP_DOCUMENT_MARKDOWN_MAX_BYTES + 16 KiB, so a request body of ~1 MiB is accepted.",
        "crates/quarry-server/src/tmp_document_handlers.rs:84-99 — the handler passes request.content bytes and content_type text/markdown straight into store.create_tmp_document / create_tmp_document_with_creation_ip with no depth or structure checks.",
        "crates/quarry-storage/src/tmp_documents.rs:120 — create_tmp_document_inner delegates to put_tmp_document_with_transaction_and_creation_ip, which at tmp_documents.rs:181-182 calls validate_tmp_markdown_write before the write.",
        "crates/quarry-storage/src/tmp_documents.rs:617-627 — validate_tmp_markdown_bytes enforces only a 1 MiB byte cap and UTF-8 validity; no YAML nesting-depth or frontmatter-size guard exists (guard checked, ineffective against deep nesting: '---\\n' + '['*500000 + ']'*500000 + '\\n---\\n' fits in 1 MiB).",
        "crates/quarry-storage/src/tmp_documents.rs:217-226 — the validated attacker content is passed to store.insert_version_conn.",
        "crates/quarry-storage/src/lib.rs:684 — insert_version_conn calls merge_markdown_frontmatter_metadata(&content, metadata, content_type) on every version insert, for every Markdown write path (tmp create/put and library put_document at documents.rs:119-127, which has no size cap).",
        "crates/quarry-storage/src/lib.rs:1858-1861 — merge_markdown_frontmatter_metadata passes the gate because content_type is text/markdown and calls markdown_frontmatter_metadata, which at lib.rs:1870 calls split_markdown_frontmatter.",
        "crates/quarry-storage/src/lib.rs:1882-1886 — split_markdown_frontmatter extracts the attacker-controlled yaml slice between the '---' markers with no length or depth limit.",
        "crates/quarry-storage/src/lib.rs:1887 — sink: serde_yaml::from_str::<serde_yaml::Value>(yaml) recursively builds a Value per nesting level; Cargo.lock pins serde_yaml 0.9.34+deprecated (unsafe-libyaml 0.2.11), which has no recursion/depth limit, so deep nesting overflows the stack and aborts the process; the subsequent serde_json::to_value on the same line and version.metadata.to_string() at lib.rs:727 recurse over the same deep value."
      ],
      "snippet": "    let frontmatter = serde_json::to_value(serde_yaml::from_str::<serde_yaml::Value>(yaml)?)?;",
      "exploitScenarios": [
        "Attacker crafts a Markdown body: '---\\n' followed by ~500,000 '[' characters, then ~500,000 ']' characters, then '\\n---\\n' (well-formed, deeply nested YAML flow sequence, under 1 MiB total).",
        "Attacker sends it anonymously as the content field of POST /v1/tmp/documents (no credentials needed; the server even generates the capability secret).",
        "The server stores the document via create_tmp_document -> put_tmp_document_with_transaction_and_creation_ip -> insert_version_conn, which invokes split_markdown_frontmatter on the body.",
        "serde_yaml::from_str::<serde_yaml::Value> recurses ~500k frames deep while building the nested Value, overflowing the tokio worker thread stack.",
        "The stack overflow aborts the whole daemon process (not a catchable panic), denying service to every user; the attacker repeats the request after each restart for sustained downtime."
      ],
      "preconditions": [
        "The quarry-server 'tmp-documents' feature is enabled so /v1/tmp/documents is installed (on by default per cfg!(feature) gate at crates/quarry-server/src/lib.rs:341).",
        "The attacker can reach the REST port (phase one has no auth; the server itself warns when bound to a non-loopback address, crates/quarry-server/src/lib.rs:695-703).",
        "serde_yaml 0.9.34's recursive Value deserialization is present (pinned in Cargo.lock); behavior inferred from its documented implementation, not executed in this review."
      ],
      "recommendations": [
        "Root cause: bound YAML nesting depth before/while parsing frontmatter — replace serde_yaml (deprecated, no depth limit) with a parser that enforces a maximum depth, or pre-scan the frontmatter slice and reject inputs whose bracket/indent nesting exceeds a small limit (e.g. 64) before calling from_str.",
        "Hardening: cap the frontmatter byte size (e.g. 64 KiB) in split_markdown_frontmatter and add a content size cap to the library put_document path (documents.rs:25), which today has none.",
        "Regression test: a put_tmp_document / put_document whose frontmatter is '['*N + ']'*N with N large (e.g. 100_000) must return an InvalidInput-style error and the process must survive; run it in a thread with a small stack to make the failure mode observable in CI."
      ]
    }
  ]
}