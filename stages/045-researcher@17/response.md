{
  "findings": [
    {
      "category": "info-disclosure",
      "title": "Collab invite tokens transported in URL query strings and persisted in browser history",
      "rationale": "Collab invite tokens are bearer capabilities that never expire (only manual revocation), yet the UI places them in URL query strings on three paths: the ?token= address-bar entry that is never stripped and so persists in browser history, the /agent-prompt?token= GET request, and the y-websocket connect URL query. Query strings are routinely recorded in server and reverse-proxy access logs, so anyone with log or browser-history access recovers a live editor/viewer capability for the shared document. The no-referrer meta tag mitigates Referer leakage but not history or log exposure, and no header- or subprotocol-based transport is used instead.",
      "file": "ui/src/api/client.ts",
      "line": 273,
      "symbol": "fetchAgentPrompt",
      "ruleId": "info-disclosure.token-in-url",
      "identity": {
        "anchor": "collab-token-url-transport"
      },
      "severity": "MEDIUM",
      "difficulty": "HIGH",
      "confidence": "HIGH",
      "snippet": ": `/v1/libraries/${segment(params.library)}/documents/${pathSegments(params.path)}/agent-prompt?token=${segment(params.token)}`;",
      "evidence": [
        "ui/src/app/App.tsx:225 — the collab invite token is read from location.search (?token=) and is never stripped from the address bar (no history.replaceState anywhere in ui/src), so every invited collaborator's browser history durably stores the live token.",
        "ui/src/app/App.tsx:1101-1103 — openAddAgentModal mints a fresh editor-role invite via createCollabInvite and passes token.id into fetchAgentPrompt.",
        "ui/src/api/client.ts:273 — fetchAgentPrompt interpolates that bearer token into the /agent-prompt?token= query string of a GET request, which web servers, reverse proxies, and log aggregators record in access logs by default.",
        "ui/src/app/App.tsx:1464 — the route token is passed as collabToken into DocumentBody for every library markdown document.",
        "ui/src/app/document-body.tsx:66 — DocumentBody forwards the token into the CollabEditorConfig consumed by MarkdownEditor.",
        "ui/src/features/editor/PlateMarkdownEditor.tsx:479 — the editor passes token: collabToken into the RUST_WS_PROVIDER_TYPE provider options.",
        "ui/src/features/collab/rust-ws-provider.ts:127 — `if (options.token) params.token = options.token;` places the token into y-websocket params, which y-websocket serializes into the WebSocket connect URL query string (ws(s)://host/v1/collab/<room>?token=...), again exposing it to server/proxy upgrade-request logs.",
        "crates/quarry-core/src/lib.rs:399-406 and crates/quarry-storage/src/documents.rs:290-297 — CollabInviteToken has no expires_at field and is created with revoked_at: None, so a leaked token remains a valid editor/viewer capability until someone manually revokes it; this confirms the exposure window is unbounded.",
        "Guard checked and found insufficient: ui/index.html:6 sets <meta name=\"referrer\" content=\"no-referrer\">, which blocks Referer leakage of ?token= but does nothing for browser-history retention or server-side request logging; no header- or subprotocol-based token transport exists as an alternative."
      ],
      "impact": "An attacker who reads server/reverse-proxy access logs, centralized log stores, or a collaborator's browser history recovers a live, non-expiring collab invite token and can join the document's Yjs collab session with the token's role (editor tokens grant write access), reading and persistently modifying the shared document under a spoofed identity until the token is manually revoked.",
      "exploitScenarios": [
        "A collaborator opens an invite link (…/documents/x.md?token=<uuid>) or a user clicks Add agent, which mints an editor token; the token is now present in the browser address bar/history and in outbound request URLs.",
        "The UI issues GET /v1/libraries/<lib>/documents/<path>/agent-prompt?token=<uuid> and connects the collab WebSocket at /v1/collab/<room>?token=<uuid>; both query strings are written to server and proxy access logs.",
        "The attacker obtains the token by reading those logs (ops access, log-aggregator breach, shared hosting) or the victim's browser history (shared machine, synced profile).",
        "The attacker opens the invite link or connects to /v1/collab/<room>?token=<stolen> and is admitted to the session with the token's role, gaining ongoing read/write access to the document."
      ],
      "preconditions": [
        "The collab/invite feature is used (invite links shared or the Add-agent flow run).",
        "The attacker has read access to server or reverse-proxy request logs, a log pipeline, or the victim's browser history.",
        "The leaked token has not been manually revoked (tokens never expire on their own)."
      ],
      "recommendations": [
        "Root cause: stop transporting the bearer token in URLs — send it in an Authorization (or custom) header for the agent-prompt HTTP request, and for the WebSocket use the Sec-WebSocket-Protocol header or a first-message auth handshake instead of a query parameter; after reading ?token= from location.search, remove it with history.replaceState so it does not persist in history.",
        "Hardening: add server-side expiry/rotation for collab invite tokens so leaked tokens have a bounded lifetime, and redact query strings containing token= in server access logging.",
        "Regression test: mock fetch and the WebSocket factory, run the invite-join and Add-agent flows, and assert no request URL contains the token value and that location.search no longer contains token= after the session starts."
      ]
    }
  ]
}