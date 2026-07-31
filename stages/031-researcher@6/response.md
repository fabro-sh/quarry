Reviewed the web-ui crypto/secrets surface end-to-end. Summary of the candidates I traced and rejected:

1. **Invite token in URL/WS query** (`App.tsx:224-227` → `document-body.tsx:66` → `PlateMarkdownEditor.tsx:479` → `rust-ws-provider.ts:127`, plus `client.ts:273`): crossing the boundary to the server proves the token is *not an enforced credential* — `/v1/collab/{document_id}` takes no token (`collab_handlers.rs:19-31`), REST endpoints explicitly "do not currently enforce bearer-token auth" (`agent_prompt.rs:71`). Leaking it grants an attacker nothing beyond what unauthenticated network access already grants, so there is no complete attack path. (The unauthenticated-WS design itself is a server-side authorization question, outside this component and lens.)
2. **`Math.random` fallback** (`App.tsx:4281`): feeds only the echo-suppression origin ID and awareness cursor label; no security decision rests on it.
3. **SHA-256 image hashing** (`image.ts:29`): content addressing only, correct primitive.
4. **Hardcoded secrets / credential storage**: none in production code; localStorage holds only author name, theme, and UI state.
5. **Third-party credential leakage**: guarded — `<meta name="referrer" content="no-referrer">` (`index.html:6`), `noopener,noreferrer` on external link opens (`PlateMarkdownEditor.tsx:1128`, `1253`), and no external fetch targets anywhere in `ui/src`.
6. **Tmp secret capability URLs**: intended bearer-capability design, 128-bit server-generated, redacted from server logs (`log_redaction.rs`).

```json
{
  "findings": []
}
```