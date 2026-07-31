{
  "findings": [
    {
      "category": "crypto-and-secrets",
      "title": "Collab invite token persists in page URL and is sent as WebSocket query parameter",
      "rationale": "The collab invite token is a server-minted, never-expiring bearer credential that grants editor access to a document. The UI circulates it in URLs (?token=), reads it from location.search but never strips it from the address bar, so it persists in browser history, and then retransmits it as a query parameter on the collab WebSocket connect URL, which servers and proxies commonly log. Anyone with access to the victim's browser history or the server/proxy logs can recover the token and gain live editor access to the document until it is manually revoked. The no-referrer meta only blocks Referer leakage to third parties and does not address history or log retention.",
      "file": "ui/src/app/App.tsx",
      "line": 225,
      "symbol": "Workspace",
      "ruleId": "info-disclosure.credential-in-url",
      "identity": {
        "anchor": "collab-invite-token-in-url"
      },
      "severity": "MEDIUM",
      "difficulty": "MEDIUM",
      "confidence": "MEDIUM",
      "impact": "The collab invite token is a never-expiring bearer credential granting editor (read/write) access to a document. Because the UI keeps it in the page URL for the whole session and retransmits it as a WebSocket query parameter, anyone who can read the victim's browser history (shared machine, synced browser profile, malware) or the quarry server/reverse-proxy request logs can recover the token and join the document as an editor until the token is manually revoked.",
      "evidence": [
        "crates/quarry-storage/src/documents.rs:290-297 — (scope crossing, server side, for impact characterization) the invite token is minted as `Uuid::new_v4()` with only created_at/revoked_at fields: it never expires and stays valid until explicit revocation.",
        "ui/src/app/App.tsx:1101-1104 — the UI mints an editor-role invite via createCollabInvite and circulates the token id in URLs (the shareable ?token= link and the /agent-prompt?token= request at ui/src/api/client.ts:273), establishing the token as a URL-borne credential.",
        "ui/src/app/App.tsx:225 — `routeCollabToken` reads the token from `location.search`; a full grep of ui/src for replaceState/navigate-based cleanup shows no code ever removes `?token=` from the address bar, and ui/src/app/workspace-navigation.ts:126-162 (the only route-rewriting effect) fires only on library mismatch, so the token persists in the URL and browser history for the entire session.",
        "ui/src/app/App.tsx:1464 — the token is passed into the editor as `collabToken={isTmpDocument ? undefined : routeCollabToken}`.",
        "ui/src/features/editor/PlateMarkdownEditor.tsx:479 — the token is forwarded into the rust-ws provider options (`token: collabToken`).",
        "ui/src/features/collab/rust-ws-provider.ts:127 — `if (options.token) params.token = options.token;` places the credential into the y-websocket `params` object, which y-websocket serializes into the WebSocket connect URL query string (`/v1/collab/<room>?token=...`), a URL form that HTTP servers and reverse proxies routinely write to access logs.",
        "Guards checked and found insufficient: ui/index.html:6 sets `<meta name=\"referrer\" content=\"no-referrer\">`, which blocks Referer leakage to third-party origins but does nothing for browser-history or server-log retention; the token is never written to localStorage (good hygiene), but retaining it in the address bar for the session defeats that; no token expiry, one-time-use, or session exchange exists as a compensating control."
      ],
      "snippet": "    () => new URLSearchParams(location.search).get('token') ?? undefined,",
      "exploitScenarios": [
        "Victim opens an invite link such as http://quarry/lib/notes/documents/plan.md?token=<uuid>; the UI consumes the token to authenticate the collab WebSocket but leaves it in the address bar.",
        "The token-laden URL is stored in the victim's browser history (and any synced history) and the WebSocket upgrade URL /v1/collab/<room>?token=<uuid> is recorded in server or reverse-proxy request logs.",
        "Attacker reads the victim's browser history on a shared/compromised machine, a synced browser profile, or the quarry host's access logs, and recovers the token.",
        "Attacker opens the same document URL with the stolen ?token= value; the UI passes it to the collab WebSocket, the server accepts it (tokens never expire), and the attacker gains live editor access — reading and modifying the document — until someone manually revokes the token."
      ],
      "preconditions": [
        "Attacker gains read access to the victim's browser history (shared machine, malware, or compromised synced browser profile) or to the quarry server/reverse-proxy request logs.",
        "The invite token has not been manually revoked (there is no automatic expiry)."
      ],
      "recommendations": [
        "Root cause: stop keeping the credential in the URL after use — once `routeCollabToken` is consumed, immediately strip the `token` query param via history.replaceState/React Router `navigate(..., { replace: true })` so it never persists in the address bar or history.",
        "Hardening: give invite tokens a server-side expiry and/or treat the URL token as a one-time bootstrap that the client exchanges for a session credential sent in the WebSocket's first message (or subprotocol) instead of the connect URL query string; ensure server/proxy logging redacts the `token` parameter.",
        "Regression test: loading a document route with ?token=<value> must, after the collab session initializes, leave window.location without the token parameter while the session remains authenticated."
      ]
    }
  ]
}