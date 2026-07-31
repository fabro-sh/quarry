I've completed the review. Summary of the analysis under the authentication/authorization lens:

**Coverage:** Read in full `ui/src/api/client.ts`, `ui/src/api/document-ref.ts`, `ui/src/app/App.tsx` (4316 lines), `ui/src/app/document-body.tsx`, `ui/src/app/open-document-state.ts`, `ui/src/app/workspace-navigation.ts`, `ui/src/app/workspace-event-stream.ts`, `ui/src/app/agent-invite.ts`, `ui/src/features/collab/rust-ws-provider.ts`, `collab-editor-session.ts`, `use-collab-editor-session.ts`, `session-events.ts`, `RemoteCursorOverlay.tsx`, `ui/src/features/review/identity.ts`, `image.ts`, `image-element.tsx`, `wiki-link-element.tsx`, `mermaid-block.tsx`, `raw-markdown.ts`, the collab-token wiring in `PlateMarkdownEditor.tsx`, plus `ui/index.html` and `ui/public`. Traced each auth-relevant flow across the boundary into `crates/quarry-server` to verify guards (stated crossing below).

**Candidates traced and rejected:**

1. **Invite token in URL query strings** (`client.ts:273` agent-prompt `?token=`; `rust-ws-provider.ts:127` ws query param; `App.tsx:225` `?token=` persisting in history) — verified server-side that the token authorizes nothing: `crates/quarry-server/src/collab_handlers.rs:19-31` accepts the WS upgrade with only the path `document_id` and never reads the token, and `agent_prompt.rs:71` states REST endpoints do not enforce bearer-token auth. No privilege is attached to the token, so its exposure is not an attack path. Additionally `ui/index.html` sets `<meta name="referrer" content="no-referrer">`, closing the cross-origin Referer leak.
2. **CSRF/drive-by** — no cookie or ambient credentials exist (capability-URL model); all mutations use non-simple methods/content-types requiring preflight, and the server installs no CORS layer (grep: no `CorsLayer`/`Access-Control` in `crates/quarry-server/src`), so cross-origin mutation/read is blocked by default.
3. **Unauthenticated library REST access** — explicitly documented trusted-localhost design decision server-side, not a UI defect.
4. **Open redirect / navigation injection** — every navigation is same-origin react-router with per-segment `encodeURIComponent` (`agent-invite.ts:13-15`, `workspace-navigation.ts`); `resolveImageSrc` prefixes non-allowlisted URLs with the same-origin serve path.
5. **Races in access decisions** — all optimistic-concurrency mutations (`uploadCurrentMarkdownFile` App.tsx:1049, conflict resolution, promote) send `If-Match` etags and fail safe on 412; editor `readOnly` transitions are UX state, not enforcement.
6. **Role bypass (viewer vs editor)** — the UI is not the enforcement point; roles are currently unenforced server-side by design.
7. **localStorage author identity** — self-declared attribution is the documented identity model ("no user accounts; humans self-declare"), and parsed localStorage JSON is strictly type-filtered.

The mermaid `dangerouslySetInnerHTML` and `window.open(href)` candidates belong to the injection/XSS lens already assigned to a different scan and are out of this lens.

No complete source-to-sink attack path with a root control in the web-ui component survives verification under this lens.

```json
{
  "job_id": "research:002-web-ui-40ce0b0c:auth-and-access:1",
  "findings": []
}
```