{
  "findings": [
    {
      "ruleId": "dos.unbounded-allocation",
      "identity": {
        "anchor": "list-item-indent-allocation"
      },
      "category": "memory",
      "title": "Peer-controlled list indent attribute drives an unbounded allocation in the Markdown writer",
      "rationale": "A collaboration peer's Yjs element attributes flow verbatim through the checkpoint projection into canonical BlockRow attrs, and every checkpoint exports those rows with block_rows_to_markdown. The writer reads attrs[\"indent\"] as an unbounded u64 and allocates 4*(indent-1) bytes via str::repeat; the only guard (.max(1)) prevents underflow but not magnitude, so an attacker-chosen integer causes a capacity-overflow panic or an allocation-failure/OOM abort that kills the server process.",
      "file": "crates/quarry-collab-codec/src/markdown_writer.rs",
      "line": 236,
      "symbol": "render_list_item",
      "snippet": "    let prefix = \"    \".repeat(key.indent as usize - 1);",
      "severity": "HIGH",
      "difficulty": "MEDIUM",
      "confidence": "HIGH",
      "impact": "Any collaboration peer who can join a document's Yjs room can force the quarry-server process to attempt an allocation of attacker-chosen size (up to ~4 x 2^63 bytes) during the debounced session checkpoint or any later Markdown export. Depending on the chosen value this is an immediate capacity-overflow panic wedging every checkpoint/export of the document, or an allocation-failure/OOM abort that kills the entire server process for all users. The poisoned attrs persist in canonical rows, so the crash re-detonates on every subsequent export.",
      "evidence": [
        "crates/quarry-server/src/collab.rs:98-101 — binary Yjs updates received from any connected peer are applied verbatim to the shared document via DefaultProtocol.handle with no content or attribute validation.",
        "crates/quarry-server/src/session.rs:887 — the checkpoint task projects the live doc with quarry_collab_codec::xmltext_to_slate, and session.rs:893 calls project_session_nodes on the result.",
        "crates/quarry-collab-codec/src/yjs_builder.rs:287-294 — xml_attrs_to_slate copies every peer-set XML attribute into Slate attrs; yjs_builder.rs:330 and 348-355 map BigInt and whole-valued f64 attribute numbers back to integer JSON values, so an attacker-chosen integer survives the Yjs->Value round trip.",
        "crates/quarry-collab-codec/src/session_doc.rs:573 — collect_block clones the element's attrs unfiltered (only \"id\" and a matching \"suggestion\" attr are removed, session_doc.rs:574-590 and 651-669), and session_doc.rs:593-598 stores them in the BlockRow attrs.",
        "crates/quarry-server/src/session.rs:814 — every checkpoint calls block_rows_to_markdown(&projection.rows) to build normalized_markdown; the `?` only handles Unsupported errors, not panics or allocation aborts.",
        "crates/quarry-collab-codec/src/rows.rs:177-180 — row_to_node copies row.attrs back onto the rebuilt node, so the peer-controlled indent reaches the writer.",
        "crates/quarry-collab-codec/src/markdown_writer.rs:94-99 — list_item_key reads attrs[\"indent\"] as an unbounded u64 (unwrap_or(1).max(1) guards only the zero/underflow case, not the magnitude) for any \"p\" element carrying a listStyleType attr.",
        "crates/quarry-collab-codec/src/markdown_writer.rs:236 — sink: \"    \".repeat(key.indent as usize - 1) allocates 4*(indent-1) bytes; indent near 2^62 panics on capacity overflow and indent near 2^33 attempts a multi-GiB allocation that aborts the process on failure. No effective guard exists anywhere on the path."
      ],
      "exploitScenarios": [
        "Join the document's collaboration room (GET /v1/collab/{document_id} takes an internal id and no per-room secret; a tmp room needs only its invite secret).",
        "Using any Yjs client, insert an XmlText embed with attributes {\"type\": \"p\", \"listStyleType\": \"decimal\", \"indent\": 4294967296} (a plain JS number) into the shared doc and send the update.",
        "Wait for the 2-second debounced checkpoint (or trigger any session-mode gateway transaction, which force-checkpoints).",
        "The checkpoint's block_rows_to_markdown call reaches render_list_item and tries to allocate ~16 GiB for the indent prefix, aborting the server process (or, with indent near 2^62, panicking the checkpoint task and wedging the document's persistence and export permanently)."
      ],
      "preconditions": [
        "Attacker can join a collaboration session for some document (valid document id for /v1/collab/{document_id}, or a tmp room secret).",
        "Attacker can craft raw Yjs updates (any yjs/yrs client library; the stock browser UI never emits such attributes).",
        "A checkpoint or Markdown export runs after the malicious update (debounced every 2s, forced by session-mode gateway transactions, or triggered by any later export)."
      ],
      "recommendations": [
        "Root cause: validate and clamp numeric attributes at the trust boundary — when projecting peer-controlled content in project_session_nodes/xmltext_to_slate, enforce a small maximum for layout integers such as indent (e.g. <= 64) and reject or clamp out-of-range values before they enter BlockRow attrs.",
        "Hardening: in list_item_key/render_list_item, cap the repeat count defensively (e.g. indent.min(MAX_INDENT) as usize) so no attr value can size an allocation, and consider a workspace-wide audit of repeat/with_capacity calls whose length derives from attrs.",
        "Regression test: build a BlockRow for a \"p\" block with listStyleType and indent = i64::MAX as u64, call block_rows_to_markdown, and assert it returns Err(Unsupported) (or a clamped render) instead of panicking or aborting; add an end-to-end checkpoint test injecting the same attrs through a Yjs update."
      ]
    },
    {
      "ruleId": "dos.unbounded-recursion",
      "identity": {
        "anchor": "yjs-projection-recursion"
      },
      "category": "memory",
      "title": "Unbounded recursion projecting peer-shaped Yjs trees overflows the server stack at checkpoint",
      "rationale": "Peer Yjs updates are applied verbatim, and the checkpoint projects the live doc with xmltext_to_slate/project_session_nodes, which recurse once per nesting level of peer-controlled XmlText embeds (and per level of nested attribute maps in any_to_value) with no depth limit. Running on a default ~2 MiB tokio task stack with no catch_unwind anywhere in the workspace, a few thousand nesting levels — a small crafted update — overflow the stack and abort the whole process with SIGSEGV.",
      "file": "crates/quarry-collab-codec/src/yjs_builder.rs",
      "line": 276,
      "symbol": "element_from_embedded_text",
      "snippet": "    let mut children = text_children_to_slate(txn, child)?;",
      "severity": "HIGH",
      "difficulty": "MEDIUM",
      "confidence": "MEDIUM",
      "impact": "A collaboration peer can crash the entire quarry-server process with a stack-overflow SIGSEGV (uncatchable, unlike a panic) by sending a Yjs update containing a few thousand levels of nested XmlText embeds or deeply nested attribute maps. The debounced checkpoint (or any session-mode gateway transaction) then projects the peer-shaped tree with unbounded recursion on a default ~2 MiB tokio task stack, denying service to every user. Confidence is MEDIUM because the exact nesting depth needed depends on frame sizes and yrs's own update-application path, which were not executed.",
      "evidence": [
        "crates/quarry-server/src/collab.rs:98-101 — peer Yjs update payloads are applied verbatim via DefaultProtocol.handle; no nesting or depth validation exists on the inbound path.",
        "crates/quarry-server/src/session.rs:887 — the checkpoint task calls quarry_collab_codec::xmltext_to_slate on the live doc root, and session.rs:893 calls project_session_nodes on the resulting tree; session_doc.rs:1469 does the same on the reconcile path.",
        "crates/quarry-collab-codec/src/yjs_builder.rs:252-258 — every Out::YXmlText/Out::YText embed in a peer-controlled XmlText becomes an element child via element_from_embedded_text.",
        "crates/quarry-collab-codec/src/yjs_builder.rs:276 — sink: element_from_embedded_text recurses into text_children_to_slate per nesting level with no depth limit; validate_node (yjs_builder.rs:144-168) has no depth cap either and is not even invoked on this inbound path (build_nodes, yjs_builder.rs:16-21, runs only on server-authored trees).",
        "crates/quarry-collab-codec/src/yjs_builder.rs:325-345 — any_to_value likewise recurses without bound over peer-controlled nested Any::Map/Any::Array attribute values, an independent recursion vector on the same projection.",
        "crates/quarry-collab-codec/src/session_doc.rs:615 — collect_block recurses per block-nesting level over the same peer-controlled depth, and the recursive Drop glue of the Node tree (slate.rs:9-20) recurses again when the tree is freed; any one of these overflows first.",
        "crates/quarry-server/src/session.rs:611-634 — the checkpoint runs on an ordinary tokio task (default ~2 MiB stack); a workspace-wide search finds no catch_unwind, panic hook, or custom stack size, so a stack overflow aborts the whole process."
      ],
      "exploitScenarios": [
        "Join a document's collaboration room as a peer.",
        "Craft a Yjs update that inserts an XmlText embed nested inside itself several thousand times (a small update — each level is a few dozen bytes of lib0 encoding), e.g. depth 10000 within a few hundred KB.",
        "Send the update over the collab websocket; the server applies it verbatim.",
        "On the next checkpoint (2s debounce) or session-mode transaction, xmltext_to_slate/project_session_nodes recurse per nesting level, overflow the tokio task stack, and the process aborts with SIGSEGV."
      ],
      "preconditions": [
        "Attacker can join a collaboration session for some document.",
        "Attacker can emit hand-crafted Yjs updates containing nested XmlText embeds or deeply nested attribute maps (custom client, not the stock UI).",
        "A checkpoint or session-mode gateway transaction runs after the update is applied."
      ],
      "recommendations": [
        "Root cause: add an explicit depth budget to the Yjs->Slate projection — thread a depth counter through text_children_to_slate/element_from_embedded_text (and any_to_value for attribute values) and return Unsupported past a small limit (e.g. 64 levels), so peer-shaped trees are rejected instead of recursed.",
        "Hardening: apply the same depth cap in project_session_nodes/collect_block and consider running projections on a dedicated thread with a known stack size plus catch_unwind, since the recursive Drop of Node alone can overflow even after parsers are made iterative.",
        "Regression test: apply a Yjs update with XmlText embeds nested past the limit and assert the checkpoint projection returns an error (and the process survives); add a matching test for deeply nested Any::Map attribute values."
      ]
    },
    {
      "ruleId": "dos.unbounded-recursion",
      "identity": {
        "anchor": "markdown-parser-recursion"
      },
      "category": "memory",
      "title": "Unbounded recursion in the Markdown event parser lets a document writer overflow the server stack",
      "rationale": "Untrusted whole-document Markdown from REST/FUSE/CLI writes is converted by a recursive EventParser that descends one frame chain per emphasis/image/list nesting level, and CommonMark nesting depth is linear in input size (~4 bytes per level). The library PUT route has no repo-level body limit (axum's ~2 MiB default allows ~500k levels) and FUSE/CLI have none at all; the parse runs inline on a ~2 MiB tokio handler stack with no catch_unwind, so deep nesting aborts the whole process via stack-overflow SIGSEGV.",
      "file": "crates/quarry-collab-codec/src/markdown.rs",
      "line": 794,
      "symbol": "parse_marked_inline",
      "snippet": "        children.extend(self.parse_inline_until(end, InlineContext { marks })?);",
      "severity": "HIGH",
      "difficulty": "LOW",
      "confidence": "MEDIUM",
      "impact": "Any user with write access to a library document can crash the quarry-server process via stack overflow by writing Markdown with deeply nested inline constructs (e.g. tens of thousands of nested emphasis or image delimiters — roughly 4 bytes of input per nesting level). The recursive EventParser descends one frame chain per nesting level on the request handler's ~2 MiB tokio stack, and with no catch_unwind anywhere the overflow SIGSEGVs the whole server, denying service to all users. FUSE and CLI write paths buffer whole files with no size limit at all, removing even the HTTP body cap. Confidence is MEDIUM because pulldown-cmark's own behavior at extreme nesting depths and the exact frames-per-level cost were not executed; if pulldown-cmark recurses internally it overflows even earlier in the same call chain.",
      "evidence": [
        "crates/quarry-server/src/lib.rs:406-414 — the library document route (PUT /v1/libraries/{library}/documents/{*path}) has no DefaultBodyLimit layer (unlike the tmp route at lib.rs:340-363), so only axum's implicit ~2 MiB body cap applies, and no repo-level Markdown size limit exists anywhere in markdown_write.rs, blocks.rs, or quarry-fuse.",
        "crates/quarry-server/src/markdown_write.rs:220-225 — put_scoped_block_document converts the request body to a String with no length check; markdown_write.rs:688-696 calls reconcile(base, &incoming_body, ...) inline in the handler task, and reconcile.rs:687-694 calls markdown_to_block_rows.",
        "crates/quarry-collab-codec/src/rows.rs:358-375 — segment_rows feeds each top-level block's events to slate_from_block_events, building the recursive EventParser (markdown.rs:158-163).",
        "crates/quarry-collab-codec/src/markdown.rs:53 — Parser::new_ext runs over the fully untrusted input; CommonMark nesting of emphasis/strong/image delimiters is linear in input size (~4 bytes per level, e.g. \"*a *b *c*d*e*\" or \"![![![x](u)](u)](u)\"), and markdown.rs:165-189 enables the relevant extensions.",
        "crates/quarry-collab-codec/src/markdown.rs:794 — sink: parse_marked_inline recurses into parse_inline_until (markdown.rs:606-640) via apply_inline_event/apply_inline_start (markdown.rs:682-783) once per emphasis/strong/strikethrough/sup/sub/link/image nesting level with no depth cap; parse_list/parse_list_item (markdown.rs:395-491) add the same unbounded recursion for nested lists.",
        "crates/quarry-server/src/gateway.rs:3027 — the parse runs inline in the axum handler task under the per-document mutex; a workspace-wide search finds no catch_unwind or custom stack size, so a stack overflow aborts the entire process (and even the recursive Drop of the resulting Node tree would overflow if parsing were made iterative)."
      ],
      "exploitScenarios": [
        "Authenticate as any user with write access to a library document (or mount via FUSE / use the CLI, which impose no size limit).",
        "Generate a Markdown body with ~100k levels of nested emphasis (about 400 KB: \"*a \" repeated, then matching closers) or nested images.",
        "PUT the body to /v1/libraries/{library}/documents/{path} (or write the file over FUSE).",
        "The server's reconcile/import path parses it; EventParser recursion exceeds the handler task's stack and the process aborts with SIGSEGV."
      ],
      "preconditions": [
        "Attacker has write access to at least one document via REST PUT, FUSE, or CLI import.",
        "The request body stays under axum's ~2 MiB default body limit on the REST path (ample for ~500k nesting levels); FUSE/CLI paths have no such cap."
      ],
      "recommendations": [
        "Root cause: enforce a nesting-depth limit in the codec — thread a depth counter through parse_inline_until/parse_marked_inline/parse_list (and reject with Unsupported past a small cap such as 64-128), since Markdown semantics require no deep nesting.",
        "Hardening: add an explicit document body size limit for the library PUT route and the FUSE/CLI write paths mirroring TMP_DOCUMENT_MARKDOWN_MAX_BYTES, and note that Drop of the recursive Node enum must also be depth-bounded or made iterative for the cap to be effective.",
        "Regression test: feed a generated document with nesting past the limit through markdown_to_block_rows and reconcile and assert Err(Unsupported) with no crash; add a server-level test asserting the request is rejected before parsing when it exceeds the body limit."
      ]
    }
  ]
}