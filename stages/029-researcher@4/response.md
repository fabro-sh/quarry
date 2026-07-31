{
  "findings": [
    {
      "title": "Tmp document capability secrets written raw to the default debug log via storage event logging",
      "category": "crypto-and-exposure",
      "file": "crates/quarry-storage/src/events.rs",
      "line": 232,
      "symbol": "log_store_event",
      "ruleId": "info-disclosure.sensitive-data-in-logs",
      "identity": {
        "anchor": "tmp-secret-store-event-logging"
      },
      "severity": "HIGH",
      "difficulty": "MEDIUM",
      "confidence": "HIGH",
      "rationale": "Tmp documents are anonymous, internet-facing scratch documents whose only protection is a 128-bit capability secret in the URL. The quarry-server crate contains a dedicated redaction module (log_redaction.rs) whose stated purpose is keeping these secrets out of logs and error bodies, but the storage layer's emit_event path calls log_store_event unconditionally, and log_store_event writes event.path() — which for every tmp document is the raw secret — into a tracing::debug! record with no redaction. The default log filter for the foreground server commands (quarry server start / quarry start) explicitly enables quarry_storage=debug, so the secret is logged on every tmp create/PUT by default; only the Docker image's RUST_LOG override suppresses it. Every hop from the unauthenticated HTTP source to the log sink was read and verified statically, including the two guards that exist (server-side request-path redaction and error-body redaction) and confirmation that neither covers this sink; the finding was not executed, which is reflected only in the deployment-configuration precondition, not in the code path itself.",
      "impact": "Every write to an anonymous tmp document logs its 32-hex bearer capability secret in cleartext. Anyone who can read the server's log output (local users, journald, centralized log drains) obtains full read/write/delete access to every live tmp document — the secret is the sole authentication for GET/PUT/DELETE /v1/tmp/documents/{secret} and the /v1/tmp/collab/{secret}/{room} WebSocket — for up to the default 30-day TTL. This defeats the capability-URL confidentiality model that is the only protection for the internet-facing anonymous tmp-document deployment, exposing all users' scratch documents cross-user. Note the scope crossing: the untrusted source enters through the quarry-server HTTP component (crates/quarry-server), whose dedicated redaction module is bypassed because the sink lives in the quarry-storage crate.",
      "evidence": [
        "crates/quarry-server/src/tmp_document_handlers.rs:99 — the unauthenticated create_tmp_document handler passes the attacker request to state.store.create_tmp_document (PUT equivalent at tmp_document_handlers.rs:525), crossing the component boundary from quarry-server into quarry-storage.",
        "crates/quarry-storage/src/tmp_documents.rs:119 — create_tmp_document_inner mints the sole bearer credential with TmpDocumentSecret::generate() (tmp_documents.rs:38-40: UUID v4 simple hex, 32 chars) and uses it as the document's storage path.",
        "crates/quarry-storage/src/tmp_documents.rs:179-180 — for direct PUTs, TmpDocumentSecret::parse(path) makes the URL path segment itself the secret, and the document row is keyed by it; the WriteOutcome entry is built from that secret path at tmp_documents.rs:245, so outcome.document.path is always the raw secret.",
        "crates/quarry-storage/src/tmp_documents.rs:255 — after every tmp create/PUT commits, self.emit_document_put_events(&outcome, origin_id) fires; staged tmp block transactions reach the same emission via crates/quarry-storage/src/transactions.rs:294-306 with change.path = secret.",
        "crates/quarry-storage/src/documents.rs:174-186 — emit_document_put_events builds StoreEvent::document_put and StoreEvent::links_indexed with path = outcome.document.path, i.e., the raw tmp secret.",
        "crates/quarry-storage/src/store.rs:148-150 — emit_event calls log_store_event(&event) unconditionally for every store event before broadcasting it.",
        "crates/quarry-storage/src/events.rs:232 — log_store_event emits tracing::debug!(\"storage.event.emitted\") with path = event.path().unwrap_or(\"\") and no redaction; this is the sink where the raw capability secret enters the log stream.",
        "Guard checked and ineffective: crates/quarry-server/src/log_redaction.rs:10-46 provides redact_path/redact_tmp_document_identifier/redact_secret_tokens, but they are applied only to quarry-server's own logging (crates/quarry-server/src/lib.rs:493 request tracing) and error bodies (crates/quarry-server/src/error.rs:224); the storage-layer event log never passes through any of them, so the server crate's redaction control is structurally bypassed.",
        "Reachability: crates/quarry-cli/src/lib.rs:198 with crates/quarry-cli/src/lib.rs:32 — the foreground server command (quarry server start / quarry start) defaults RUST_LOG to DEVELOPMENT_FILTER, which explicitly enables quarry_storage=debug, so the secret-bearing debug event is emitted by default; only the official Dockerfile (Dockerfile:63, RUST_LOG=info,quarry=info) suppresses it, and only until an operator overrides it."
      ],
      "snippet": "        path = event.path().unwrap_or(\"\"),",
      "exploitScenarios": [
        "An operator runs the server with its default logging configuration (quarry server start), which enables quarry_storage=debug output to stderr/journal.",
        "Any unauthenticated client creates or writes a tmp document (POST /v1/tmp/documents or PUT /v1/tmp/documents/{secret}); each write emits a storage.event.emitted debug record containing path=<32-hex secret> (two records per write: doc.changed plus links.indexed).",
        "The attacker obtains read access to the server's logs — as a local user, via journald, or via a centralized log collection/aggregation pipeline that the ops team and tooling can read.",
        "The attacker extracts 32-character hex tokens from storage.event.emitted log lines.",
        "The attacker uses a recovered secret as the bearer capability — GET/PUT/DELETE /v1/tmp/documents/{secret} or the /v1/tmp/collab/{secret}/{room} WebSocket — to read, modify, or delete the victim's tmp document at any time until it expires (default 30 days)."
      ],
      "preconditions": [
        "The server runs with a log filter that enables quarry_storage debug events (the default for the quarry server start and quarry start commands; the official Docker image overrides RUST_LOG to info and is unaffected only while that override stands).",
        "The attacker can read the server's log output (local access, systemd journal, or a log aggregation platform).",
        "The targeted tmp documents are still within their TTL (30 days by default, extensible by their owners via the TTL endpoint)."
      ],
      "recommendations": [
        "Root cause fix: redact tmp-scoped paths inside log_store_event (crates/quarry-storage/src/events.rs:227) itself — when event.library_id() is the tmp scope (TMP_TRANSACTION_LIBRARY_ID, \"__tmp__\"), log a fixed placeholder instead of event.path() and event.new_path(); do not rely on downstream consumers to redact, since the storage crate emits these records independently of the server crate's redaction module.",
        "Hardening: audit every StoreEvent producer that carries a tmp path (crates/quarry-storage/src/documents.rs:174-186 and crates/quarry-storage/src/transactions.rs:294-323) so the secret is never placed in a log-facing field in the first place, and consider lowering the default server log filter for quarry_storage from debug to info.",
        "Regression test: add a test that performs a tmp document create/PUT against a running server with a capturing tracing subscriber (the harness at crates/quarry-server/tests/common/mod.rs:118 already builds one) and asserts the raw secret string appears in no emitted log record, including storage.event.emitted records."
      ]
    }
  ]
}