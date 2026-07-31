I've completed the auth-lens review of the web-ui component. Summary of what I traced:

**Reviewed paths (all in scope `ui/src`, `ui/public`):**
- `api/client.ts` — full read: every REST call is same-origin `fetch` with no cookies, no `Authorization` header, no credentials; the API has no client-carried auth state to bypass or forge. Mutation concurrency uses `If-Match` etags (no TOCTOU race).
- `App.tsx:225` — `?token=` invite token read from URL, passed only to the collab WS provider (`document-body.tsx:66`, `rust-ws-provider.ts:127`) and to `fetchAgentPrompt`. It never authorizes REST writes (nothing does — capability model), so no auth-bypass path exists client-side; token-in-URL persistence is the product's invite-link design (server even redacts capability secrets from logs).
- `rust-ws-provider.ts` — token goes only as a query param to a same-host WS (`window.location.host`); checkpoint frames only feed the save-state indicator, no access decision.
- `workspace-event-stream.ts` + `App.tsx:458-524` — SSE payloads drive cache invalidation and same-origin SPA navigation only (`openLibraryDocument` builds internal `/lib/...` routes via `workspace-navigation.ts`); no open redirect.
- `wiki-link-element.tsx`, `PlateMarkdownEditor.tsx:1128` — link navigation is user-gesture opening of a visible link; the non-http-scheme concern is an injection-lens question, and library sanitization can't be verified (no `node_modules`), so no complete auth-lens path is provable.
- Viewer-vs-editor invite role: the client has no role concept; enforcement lives server-side in `crates/quarry-server` (out of scan scope), and `quarry-storage` does validate roles — no provable client-side privilege-escalation gap.
- CSRF/SSRF: no cookie-based auth exists to forge, and the client never asks the browser or server to fetch attacker-chosen origins.

No complete source-to-sink attack path within the authentication/authorization lens survived verification.

```json
{
  "findings": []
}
```