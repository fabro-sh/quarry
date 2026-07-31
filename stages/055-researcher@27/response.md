{
  "findings": [
    {
      "ruleId": "dos.uncontrolled-recursion",
      "category": "dos",
      "title": "Unbounded recursion in the Markdown event parser lets a crafted document write crash the server",
      "rationale": "EventParser in markdown.rs descends recursively once per nesting level of attacker-controlled Markdown (nested lists via parse_list_item -> parse_list at markdown.rs:447, nested inline marks via parse_marked_inline -> parse_inline_until at markdown.rs:794) with no depth budget anywhere in the crate. Nesting costs only ~6 bytes per level, so the ~1-2 MiB body limits permit hundreds of thousands of frames against a ~2 MiB tokio worker stack. A stack overflow in Rust aborts the process rather than unwinding, so a single write takes down the whole server. Confidence is MEDIUM because the crash magnitude (per-frame stack cost, runtime stack size) was inferred from code reading rather than executed.",
      "identity": {
        "anchor": "markdown-event-parser-recursion"
      },
      "file": "crates/quarry-collab-codec/src/markdown.rs",
      "line": 447,
      "symbol": "parse_list_item",
      "snippet": "                    self.parse_list(start, out, ctx.indent)?;",
      "severity": "HIGH",
      "difficulty": "LOW",
      "confidence": "MEDIUM",
      "impact": "A single crafted Markdown write aborts the entire quarry-server process via stack exhaustion (stack overflow in Rust is a SIGSEGV/abort, not a catchable unwind), denying service to every user and live collab session and discarding in-flight writes; the attacker can repeat the crash after every restart.",
      "evidence": [
        "crates/quarry-server/src/document_handlers.rs:509 — put_document accepts the raw HTTP request body (Bytes) for PUT /v1/libraries/{library}/documents/{*path} and routes BlockDocument writes into the markdown write path; the library routes carry no DefaultBodyLimit layer (crates/quarry-server/src/lib.rs:397-408), so only the ~2 MiB axum default applies — this is the boundary crossing from server into the codec scope.",
        "crates/quarry-server/src/markdown_write.rs:688 — write_markdown_with passes the attacker-controlled body (frontmatter-stripped incoming_body) to quarry_collab_codec::reconcile.",
        "crates/quarry-collab-codec/src/reconcile.rs:257 — reconcile calls parse_top_blocks(incoming_markdown), which at reconcile.rs:689 calls markdown_to_block_rows on the untrusted text.",
        "crates/quarry-collab-codec/src/rows.rs:107 — markdown_to_block_rows feeds every parsed top-level segment to segment_rows, which at rows.rs:363 calls slate_from_block_events(segment.events); rows.rs:311-347 (top_level_segments) is iterative and adds no depth bound.",
        "crates/quarry-collab-codec/src/markdown.rs:161 — slate_from_block_events constructs EventParser { events, index: 0 } and calls parse_top_level with no depth budget field anywhere in the struct.",
        "crates/quarry-collab-codec/src/markdown.rs:447 — parse_list_item recurses into parse_list (markdown.rs:395) for every nested list level; nesting depth is attacker-controlled at roughly 6 bytes of Markdown per level (\"  - x\\n\"), so the ~1–2 MiB body limits permit 300k+ recursion frames against a ~2 MiB tokio worker stack.",
        "crates/quarry-collab-codec/src/markdown.rs:794 — the same unbounded recursion exists inline: parse_marked_inline recurses into parse_inline_until (markdown.rs:606) per nested emphasis/strong/strikethrough/superscript level, reachable the same way.",
        "Guard check: searching crates/quarry-collab-codec/src for depth/recursion/stack/LIMIT finds only LCS/diff work budgets (reconcile.rs:146-150, text_diff.rs:58) — no nesting-depth guard exists on the parse path; contains_critic_markup (markdown.rs:18) and the pulldown-cmark parse itself are iterative and impose no depth cap on the event stream the recursive EventParser consumes."
      ],
      "exploitScenarios": [
        "Authenticate to the API (or mount a FUSE filesystem / run a CLI import) and obtain write access to any Markdown block document.",
        "Generate a document body of ~50 KB–2 MB consisting of a nested list, each level indented two more spaces than the last (\"- x\", \"  - x\", \"    - x\", ...), or equivalently thousands of nested emphasis markers.",
        "PUT the body to /v1/libraries/{library}/documents/{path} with content_type text/markdown.",
        "The server runs reconcile → markdown_to_block_rows → slate_from_block_events; EventParser recurses once per nesting level, exhausts the tokio worker thread stack, and the process aborts.",
        "Repeat the request after each restart to keep the service down."
      ],
      "preconditions": [
        "Attacker can write a Markdown block document (REST PUT, FUSE write, or CLI import).",
        "Request body fits the transport limit (~1 MiB for tmp documents, ~2 MiB axum default for library documents); ~50 KB already suffices for thousands of recursion frames.",
        "Server handles the write on a finite-stack thread (tokio worker, default ~2 MiB); no recursion-depth guard exists in quarry-collab-codec.",
        "Crash magnitude inferred from code reading (per-frame stack cost and thread stack size not measured by execution), hence MEDIUM confidence."
      ],
      "recommendations": [
        "Root fix: add a depth budget to EventParser — track current nesting depth in parse_list/parse_list_item and parse_inline_until/parse_marked_inline and return Err(Unsupported::new(\"nesting too deep\")) past a small bound (e.g. 64–256), or rewrite the descent over an explicit heap stack.",
        "Hardening: enforce an explicit maximum Markdown body size for library-scope block-document writes (mirroring TMP_DOCUMENT_MARKDOWN_MAX_BYTES) so depth-per-byte amplification has a low ceiling, and consider running codec work on a dedicated large-stack worker with catch_unwind isolation.",
        "Regression test: import a document with e.g. 10 000 nested list levels and 10 000 nested emphasis levels via markdown_to_block_rows and block_markdown_to_slate; assert both return Err(Unsupported) promptly instead of overflowing the stack."
      ]
    },
    {
      "ruleId": "dos.uncontrolled-recursion",
      "category": "dos",
      "title": "Unbounded recursion projecting peer-controlled Yjs documents lets a session peer crash the server",
      "rationale": "xmltext_to_slate and element_from_embedded_text in yjs_builder.rs are mutually recursive (yjs_builder.rs:253 and :276) with depth equal to the embed nesting of the live shared document, which any session peer can grow arbitrarily at a cost of one small CRDT item per level; collect_block in session_doc.rs:615 then recurses again over the same attacker-depth tree at checkpoint. No depth guard exists in either file, and validate_node only inspects server-built nodes. A stack overflow aborts the server process, and because the malicious structure persists in the CRDT the crash can recur on every checkpoint. Confidence is MEDIUM because update-size ingress limits and the crash itself were not executed.",
      "identity": {
        "anchor": "xmltext-slate-recursion"
      },
      "file": "crates/quarry-collab-codec/src/yjs_builder.rs",
      "line": 253,
      "symbol": "text_children_to_slate",
      "snippet": "                children.push(element_from_embedded_text(txn, attrs, &child)?);",
      "severity": "HIGH",
      "difficulty": "MEDIUM",
      "confidence": "MEDIUM",
      "impact": "A malicious session peer crashes the quarry-server process via stack exhaustion during server-side projection/reconcile of the shared document (stack overflow aborts, it does not unwind), taking down all users and sessions; the malicious structure persists in the CRDT, so the crash can recur on every checkpoint until the document is purged.",
      "evidence": [
        "crates/quarry-collab-codec/src/session_doc.rs:1469 — current_children calls xmltext_to_slate(txn, parent) on the live shared document during reconcile_session_children, and session_doc.rs:1361 calls it again in reconcile_mark_runs; the document's content and embed structure are written by any session peer over the collab websocket (crates/quarry-server/src/lib.rs:335-336,361-362).",
        "crates/quarry-collab-codec/src/yjs_builder.rs:106 — xmltext_to_slate begins the projection of the peer-influenced XmlText with no depth parameter or budget.",
        "crates/quarry-collab-codec/src/yjs_builder.rs:253 — text_children_to_slate recurses into element_from_embedded_text for every embedded XmlText (Out::YXmlText) a peer inserted.",
        "crates/quarry-collab-codec/src/yjs_builder.rs:276 — element_from_embedded_text recurses back into text_children_to_slate, so recursion depth equals the peer-controlled embed nesting depth; each nesting level costs a peer only one small Yjs item (tens of bytes of update), and no guard caps it.",
        "crates/quarry-collab-codec/src/session_doc.rs:615 — after projection, collect_block recurses again per nesting level of the same attacker-depth node tree during checkpoint projection (project_session_nodes, session_doc.rs:515), so fixing only yjs_builder would leave a second uncapped descent on the same data.",
        "Guard check: no depth/limit logic exists in yjs_builder.rs or the projection half of session_doc.rs (grep for depth|LIMIT finds none there); validate_node (yjs_builder.rs:144) checks only JSON value shapes of server-built nodes and never runs on the inbound peer tree before the recursive projection."
      ],
      "exploitScenarios": [
        "Join a collaborative session as a peer (library collab websocket with document access, or a tmp collab room whose secret the attacker holds).",
        "Using a custom Yjs client, send an update that builds tens of thousands of nested XmlText embeds inside the document root (each level is one small CRDT item).",
        "Wait for (or trigger) server-side projection: a gateway reconcile via reconcile_session_children, or a checkpoint via project_session_nodes.",
        "The server's recursive xmltext_to_slate/collect_block descent exhausts the worker thread stack and the process aborts; because the nested structure is persisted in the CRDT, subsequent checkpoints crash again."
      ],
      "preconditions": [
        "Attacker is a session peer able to push Yjs updates for the target document (collab websocket access; tmp rooms are gated only by the room secret).",
        "The server projects or reconciles the poisoned document (checkpoint scheduling or any semantic mutation through the gateway).",
        "No update-size or embed-depth limit is enforced at the sync ingress (none found in the codec; server ingress limits not verifiable from this crate), contributing to MEDIUM confidence alongside the unexecuted crash."
      ],
      "recommendations": [
        "Root fix: thread a depth budget through xmltext_to_slate/element_from_embedded_text and through collect_block in session_doc.rs, returning Err(Unsupported) when peer-controlled nesting exceeds a small bound (e.g. 64–128 levels), or convert both descents to explicit heap-stack iteration.",
        "Hardening: reject oversized or structurally pathological Yjs updates at the collab websocket ingress (message size cap and/or embed-depth validation before applying to the shared doc).",
        "Regression test: build a Yjs document with e.g. 10 000 nested XmlText embeds and assert xmltext_to_slate and project_session_nodes return Err(Unsupported) instead of overflowing the stack."
      ]
    }
  ]
}