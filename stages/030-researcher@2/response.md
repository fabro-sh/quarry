{
  "findings": [
    {
      "ruleId": "dos.unbounded-recursion",
      "category": "dos",
      "title": "Unbounded recursion in block-tree serialization lets an unauthenticated attacker crash the server process",
      "identity": {
        "anchor": "doc-model-recursive-row-serialization"
      },
      "file": "crates/quarry-server/src/gateway.rs",
      "line": 1063,
      "symbol": "DocModel::collect_block_rows",
      "severity": "HIGH",
      "difficulty": "LOW",
      "confidence": "HIGH",
      "snippet": "        self.collect_rows(Some(block_id), out);",
      "impact": "An unauthenticated remote attacker aborts the entire quarry-server process via stack overflow: DocModel::to_rows() serializes the block tree with mutual recursion whose depth equals attacker-controlled block nesting depth, so a sufficiently deep document overflows the tokio worker stack. Rust stack overflow is a process-wide abort (SIGABRT), not a catchable panic, killing every in-flight request, collab session, and SSE stream for all users. The deep tree persists in the store, so the attacker can re-crash the server at will after every restart with a single transaction against the same secret URL.",
      "rationale": "The tmp document API is the designed internet-facing surface and hands out capability secrets with no authentication. The transactions endpoint accepts an unbounded list of insert_block ops, and insert_block's only structural guard is that parent_block_id already exists, so an attacker can build a parent-child chain of arbitrary depth. Every commit then calls DocModel::to_rows(), whose collect_rows/collect_block_rows recurse once per tree level with no depth cap, converting attacker-controlled nesting depth into native stack depth on a tokio worker thread. Guards checked and found ineffective: the ops.is_empty() check (no count limit), the parent-existence check (no depth limit), the move_block cycle check (does not apply to inserts), and the ~1 MiB body limit (depth accumulates across persisted transactions). The exact overflow threshold was not executed (read-only review), but exploitability does not depend on it because depth grows monotonically across requests until any plausible stack/frame combination overflows.",
      "evidence": [
        "crates/quarry-server/src/lib.rs:355-357 — installs POST /v1/tmp/documents handled by create_tmp_document with no authentication, under the default-enabled tmp-documents feature (lib.rs:341-343).",
        "crates/quarry-server/src/tmp_document_handlers.rs:61-102 — create_tmp_document creates a document from the attacker's JSON body and returns the capability secret (WriteOutcome), so the attacker holds a valid secret for the next request.",
        "crates/quarry-server/src/lib.rs:363 — installs /v1/tmp/documents/{*path} with a POST handler and DefaultBodyLimit of TMP_DOCUMENT_HTTP_BODY_LIMIT (1 MiB + 16 KiB, lib.rs:117-118); no auth beyond secret possession in the URL.",
        "crates/quarry-server/src/tmp_document_handlers.rs:584-586 — post_tmp_document_action routes the Transactions subresource to gateway::tmp_document_block_transactions, passing the raw attacker JSON body through.",
        "crates/quarry-server/src/gateway.rs:733-752 — parse_transaction deserializes the untrusted body into BlockTransactionRequest and parses every op; the only guard is ops.is_empty() at line 742 — there is no op-count limit and no structural depth check.",
        "crates/quarry-server/src/gateway.rs:3028 — apply_rows_transaction calls apply_ops on the parsed ops; this runs inline on the async worker, before any store commit, so storage-side limits cannot intervene.",
        "crates/quarry-server/src/gateway.rs:1156-1217 — ApplyContext::insert_block accepts an attacker-chosen block_id and any parent_block_id that already exists; the only guard checked is parent existence (lines 1183-1187), so ops can chain parent->child arbitrarily deep. The cycle check at lines 1321-1326 exists only in move_block and does not apply to inserts, and attach (lines 1102-1109) adds no depth guard.",
        "crates/quarry-server/src/gateway.rs:2023 — after applying the ops, apply_ops evaluates ctx.model.to_rows() to produce the rows to commit.",
        "crates/quarry-server/src/gateway.rs:1036-1064 — to_rows -> collect_rows -> collect_block_rows recurse once per tree level (collect_block_rows calls collect_rows(Some(block_id)) at line 1063) with no depth bound, so attacker-controlled nesting depth becomes native stack depth on the tokio worker thread.",
        "Guard search: grep for depth/nesting/MAX_* caps across crates/quarry-server/src finds no nesting-depth limit anywhere in the insert, transaction, or serialization path; the body limit only caps per-request size, while tree depth accumulates across transactions in persisted rows."
      ],
      "exploitScenarios": [
        "POST /v1/tmp/documents with a small JSON body and read the returned capability secret from the WriteOutcome response.",
        "POST /v1/tmp/documents/{secret}/transactions with a client_tx_id, a minimal actor, and thousands of insert_block ops, each with an explicit block_id and parent_block_id set to the previous op's block_id, building one deep parent-child chain (roughly 8,000 ops fit in the ~1 MiB body limit).",
        "The server applies the ops and calls DocModel::to_rows(), recursing one collect_rows/collect_block_rows frame pair per nesting level on the tokio worker thread.",
        "If a single request's depth does not overflow the ~2 MiB default worker stack, repeat step 2 with fresh client_tx_id values: the chain persists in the store, each commit re-walks the full accumulated depth, and a few requests push depth past any plausible stack/frame combination.",
        "The worker thread overflows its stack, the Rust runtime aborts the whole server process, and all users' requests, collab sessions, and SSE streams die; the attacker re-triggers the crash after each restart with one more transaction against the persisted deep document."
      ],
      "preconditions": [
        "The server is built with the tmp-documents feature, which is in the default feature set.",
        "The attacker can reach the tmp HTTP API, which is the designed internet-facing, secret-capability surface (no account or token required).",
        "Nothing else: no authentication, no victim interaction, and no special timing are needed."
      ],
      "recommendations": [
        "Root cause: enforce a maximum block-nesting depth at the write boundary — reject insert_block and move_block ops (and markdown fragment inserts via parse_markdown_fragment) that would exceed a bounded depth such as 64 levels — so attacker-controlled depth can never reach the serializer; validate against the merged model in ApplyContext::insert_block/move_block before attach.",
        "Hardening: rewrite DocModel::collect_rows/collect_block_rows as an explicit iterative traversal with a heap-allocated stack so even corrupted or legacy deep rows cannot overflow the native stack, and audit the sibling recursive walkers reachable from untrusted depth (session seeding via quarry_collab_codec::seed_session_nodes and markdown rendering) for the same pattern; consider a per-transaction op-count cap as defense in depth.",
        "Regression test: a gateway test that submits a transaction chaining insert_block ops deeper than the configured maximum and asserts a typed rejection, plus a test that a document at exactly the maximum depth commits and serializes successfully."
      ]
    }
  ]
}