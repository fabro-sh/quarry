```json
{
  "job_id": "research:006-storage-49a25f9f:injection-and-input:2",
  "findings": [
    {
      "ruleId": "improper-input-validation.document-ttl",
      "category": "improper-input-validation",
      "title": "Unvalidated expires_at string lets anonymous callers pin tmp documents forever",
      "rationale": "Tmp document TTL values arrive from anonymous REST callers as raw strings, are stored verbatim, and are only ever compared lexicographically against the current RFC3339 timestamp. No validation exists on any hop, so any string sorting after 'now' defeats the 30-day expiry and CAS garbage collection — a complete path from untrusted input to a broken lifecycle control.",
      "identity": {
        "anchor": "tmp-document-ttl-validation"
      },
      "file": "crates/quarry-storage/src/tmp_documents.rs",
      "line": 198,
      "symbol": "put_tmp_document_with_transaction_and_creation_ip",
      "snippet": "TmpTtl::ExpiresAt(expires_at) => expires_at,",
      "severity": "MEDIUM",
      "difficulty": "LOW",
      "confidence": "HIGH",
      "impact": "An anonymous, unauthenticated caller can supply an arbitrary string as a tmp document's expires_at. Because every liveness check and the CAS garbage collector compare expires_at lexicographically against the current RFC3339 timestamp, any string that sorts after 'now' (e.g. \"9999-01-01T00:00:00Z\", or simply \"z\") makes the document permanently readable and pins its content blobs against GC forever. This defeats the 30-day tmp-document lifecycle — the only reclamation and abuse-remediation mechanism on the anonymous surface — and converts the ephemeral scratch space (1 MiB per document, unlimited documents) into unbounded permanent attacker-controlled storage.",
      "evidence": [
        "crates/quarry-server/src/tmp_document_handlers.rs:36 — the anonymous POST /v1/tmp/documents request body deserializes expires_at as a raw Option<String> (data flow crosses from quarry-server into the storage component here).",
        "crates/quarry-server/src/tmp_document_handlers.rs:80 — request.expires_at.map(quarry_storage::TmpTtl::ExpiresAt) wraps the untrusted string with no validation of any kind.",
        "crates/quarry-storage/src/tmp_documents.rs:198 — the TmpTtl::ExpiresAt arm stores the caller's string verbatim; only the Default/Unchanged arms produce canonical RFC3339 timestamps, and no parse or shape check exists anywhere on this path (verified by reading the full function and grepping for expires_at validation).",
        "crates/quarry-storage/src/tmp_documents.rs:239 — UPDATE documents SET expires_at = ?1 WHERE id = ?2 persists the raw string (parameterized, so the flaw is semantic, not SQL injection).",
        "crates/quarry-storage/src/tmp_documents.rs:407 — set_tmp_document_ttl (PATCH /v1/tmp/documents/{secret}/ttl, handler at tmp_document_handlers.rs:566) stores a second unvalidated caller string the same way, so the TTL of an existing document can also be pinned.",
        "crates/quarry-storage/src/lib.rs:472 — tmp document lookups authorize reads with the TEXT comparison `expires_at > ?2` against now_timestamp(); SQLite's binary collation makes any string sorting after the current timestamp pass forever (same pattern at lib.rs:840 and lib.rs:937).",
        "crates/quarry-storage/src/lib.rs:222 — the GC reachability query keeps blobs for any document where d.expires_at > now, so a pinned document's CAS blobs are never reclaimed; guards checked: the 1 MiB content cap (tmp_documents.rs:618) limits per-document size only, not count or lifetime."
      ],
      "exploitScenarios": [
        "POST /v1/tmp/documents with body {\"content\": \"...\", \"expires_at\": \"9999-01-01T00:00:00Z\"} — no authentication required.",
        "Repeat to create an unlimited number of tmp documents (each up to 1 MiB) whose expires_at sorts after any real timestamp.",
        "The documents stay readable indefinitely and every GC run treats their blobs as reachable, so storage grows without bound and abuse content cannot be aged out.",
        "Optionally, PATCH /v1/tmp/documents/{secret}/ttl with {\"expires_at\": \"z\"} pins any already-created document the caller holds the secret for."
      ],
      "preconditions": [
        "The server is built with the `tmp-documents` feature and the /v1/tmp/documents routes are exposed.",
        "Attacker can reach the create endpoint (anonymous by design); the TTL-reset variant additionally requires holding a document's 32-hex capability secret."
      ],
      "recommendations": [
        "Root cause: validate expires_at at the trust boundary — parse TmpTtl::ExpiresAt values and set_tmp_document_ttl/set_document_ttl inputs with chrono::DateTime::parse_from_rfc3339, reject anything unparsable with InvalidInput, and re-emit a canonical timestamp before storage.",
        "Hardening: clamp caller-supplied TTLs to a maximum horizon (e.g. the 30-day default) so the anonymous surface cannot extend retention beyond policy.",
        "Regression test: creating a tmp document with expires_at \"z\", \"99999\", or a malformed timestamp must fail with 400/InvalidInput, and a far-future valid timestamp must be rejected or clamped; GC must collect blobs of documents whose TTL was not extended through the validated path."
      ]
    },
    {
      "ruleId": "dos.frontmatter-yaml-depth",
      "category": "dos",
      "title": "Unguarded serde_yaml parse of attacker frontmatter on every Markdown write",
      "rationale": "Anonymous 1 MiB tmp writes (and unbounded authenticated library writes) feed attacker-controlled YAML frontmatter into serde_yaml 0.9.34+deprecated with no size or depth guard; serde_yaml's recursive Value deserialization has no recursion limit, so deeply nested frontmatter can exhaust the worker stack and abort the daemon. The unguarded parse path is verified in code; the crash outcome is inferred, not executed, hence LOW confidence.",
      "identity": {
        "anchor": "markdown-frontmatter-yaml-parse"
      },
      "file": "crates/quarry-storage/src/lib.rs",
      "line": 1887,
      "symbol": "split_markdown_frontmatter",
      "snippet": "let frontmatter = serde_json::to_value(serde_yaml::from_str::<serde_yaml::Value>(yaml)?)?;",
      "severity": "MEDIUM",
      "difficulty": "LOW",
      "confidence": "LOW",
      "impact": "Every Markdown write parses attacker-controlled YAML frontmatter with serde_yaml 0.9.34+deprecated, which has no recursion-depth limit and whose Value deserialization recurses once per nesting level. A single anonymous tmp-document write of up to 1 MiB can carry frontmatter nested ~500,000 levels deep (`key: [[[[...]]]]`), which is expected to exhaust the worker-thread stack and abort the whole daemon process, denying service to all users. Confidence is LOW because executing a proof was not permitted; the unguarded parse of untrusted YAML on an anonymous path is verified in code, while the stack-overflow outcome is inferred from serde_yaml's recursive deserialization model rather than observed.",
      "evidence": [
        "crates/quarry-server/src/tmp_document_handlers.rs:61 — the anonymous POST /v1/tmp/documents handler accepts caller Markdown content and forwards it to storage (boundary crossing).",
        "crates/quarry-storage/src/tmp_documents.rs:182 — validate_tmp_markdown_write is the only content guard on the tmp path: it enforces the 1 MiB byte cap and UTF-8 validity (tmp_documents.rs:617-628) and nothing about YAML structure or depth.",
        "crates/quarry-storage/src/lib.rs:684 — insert_version_conn runs merge_markdown_frontmatter_metadata on every version insert whose content type is Markdown, including anonymous tmp writes.",
        "crates/quarry-storage/src/lib.rs:1870 — markdown_frontmatter_metadata feeds the untrusted text to split_markdown_frontmatter, which extracts the delimited frontmatter block.",
        "crates/quarry-storage/src/lib.rs:1887 — serde_yaml::from_str::<serde_yaml::Value> parses the attacker YAML with no size or depth guard; Cargo.lock pins serde_yaml 0.9.34+deprecated (unmaintained, no recursion limit analogous to serde_json's).",
        "crates/quarry-storage/src/documents.rs:84 — the same parser is reachable through put_document for authenticated library writes, where content has no byte cap at all, so nesting depth is limited only by the HTTP body limit."
      ],
      "exploitScenarios": [
        "POST /v1/tmp/documents with content starting `---\\nkey: ` followed by ~500,000 nested `[` characters, a matching close, and `\\n---\\n` (fits within the 1 MiB cap).",
        "Storage validates only size/UTF-8, then insert_version_conn parses the frontmatter.",
        "serde_yaml descends one native stack frame per nesting level, overflows the async worker's stack, and the process aborts (stack overflow is not a catchable Rust panic).",
        "Repeat after restart for sustained denial of service."
      ],
      "preconditions": [
        "tmp-documents feature enabled for the anonymous variant (any unauthenticated network caller); otherwise an authenticated library writer can reach the same parser with unbounded document size.",
        "Frontmatter must open with `---` and close with a later `---` delimiter line.",
        "Crash outcome depends on nesting depth exceeding the worker thread's usable stack; not verified by execution."
      ],
      "recommendations": [
        "Root cause: bound the YAML attack surface before parsing — reject frontmatter blocks above a small size cap (e.g. 64 KiB) and enforce a nesting-depth/alias budget, or migrate off the deprecated serde_yaml to a parser with configurable depth limits.",
        "Hardening: treat frontmatter parse failures as non-fatal to the write (store content, skip metadata merge) so malformed frontmatter can never reject or destabilize a write, and apply a document-size cap to library puts as well.",
        "Regression test: a tmp write with frontmatter nested tens of thousands of levels deep must complete with a clean 4xx or a stored-but-unparsed result, and must not abort or hang the process."
      ]
    },
    {
      "ruleId": "dos.search-full-scan",
      "category": "dos",
      "title": "Search performs a full-library scan with per-document CAS disk reads per query",
      "rationale": "search_documents clamps only the result count; every query re-loads up to 10,000 entries and re-reads each textual document body (one CAS disk read for anything over 64 KiB) with no index or cache. An authenticated reader can loop cheap search requests into sustained disk saturation — a complete, verified cost-amplification path whose real-world impact depends on library size, hence MEDIUM confidence.",
      "identity": {
        "anchor": "search-full-library-scan"
      },
      "file": "crates/quarry-storage/src/search.rs",
      "line": 19,
      "symbol": "search_documents",
      "snippet": ".document_entries_for_library_conn(&conn, &library_record.id, 10_000)",
      "severity": "MEDIUM",
      "difficulty": "LOW",
      "confidence": "MEDIUM",
      "impact": "Each search request scans up to 10,000 document entries and, for every textual document, re-reads the full body — including one CAS disk read per document larger than 64 KiB — with no index, cache, or per-request work budget. An authenticated reader can issue cheap search requests in a loop and multiply them into tens of thousands of disk reads per request, degrading storage and API availability for all other users of the daemon.",
      "evidence": [
        "crates/quarry-storage/src/search.rs:15 — only the returned-result limit is clamped (min 100); nothing bounds the scan or the per-document body reads.",
        "crates/quarry-storage/src/search.rs:19 — every query loads up to 10,000 library document entries regardless of the query or limit.",
        "crates/quarry-storage/src/search.rs:46 — for each entry with a textual content type and a non-empty query, self.get_document fetches the complete document body just to run a substring find.",
        "crates/quarry-storage/src/lib.rs:987 — get_document resolves non-inline content via self.cas.read(hash), one filesystem read per document per query (quarry-cas/src/lib.rs:111-118); guards checked: the 10,000-entry cap exists but is the amplification ceiling, not a defense, and no result or body cache exists anywhere in search.rs.",
        "crates/quarry-storage/src/search.rs:55-57 — an additional links_for_source_conn SQL query is issued per document per search, compounding the per-request cost."
      ],
      "exploitScenarios": [
        "Authenticate to a library containing many large textual documents (or first create them, since the same role can write).",
        "Issue repeated GET search requests with any non-empty query term.",
        "Each request re-scans up to 10,000 entries and performs one CAS disk read per large textual document; concurrent requests saturate disk I/O and stall unrelated reads and writes on the shared store."
      ],
      "preconditions": [
        "Attacker holds read access to a library (authenticated; the anonymous tmp surface cannot reach search).",
        "Cost scales with library size; meaningful impact needs a substantial corpus of textual documents."
      ],
      "recommendations": [
        "Root cause: stop re-reading document bodies per query — maintain a real search index (e.g. SQLite FTS5 over content) updated on write, so query cost is independent of library size.",
        "Hardening: until an index exists, cap the number of body reads per request, cache decoded bodies, and rate-limit the search endpoint per caller.",
        "Regression test: searching a library of N large documents must perform a bounded number of storage reads independent of N; a test asserting query cost stays flat as documents are added catches regressions."
      ]
    }
  ]
}
```