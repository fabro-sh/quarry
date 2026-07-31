The last greps confirm no other auth-relevant surface (no cookies, credentials, redirects, or postMessage in `ui/src`). My review is complete.

## Summary

I read the web-ui hot paths in full (`client.ts`, `document-ref.ts`, `App.tsx` routing/token/event sections, `workspace-navigation.ts`, `workspace-event-stream.ts`, `rust-ws-provider.ts`, `collab-editor-session.ts`, `identity.ts`, `collab-debug.ts`, `image.ts`, `image-element.tsx`, `document-body.tsx`, `PlateMarkdownEditor.tsx` link/collab sections, `vite.config.ts`, `index.html`, `ui/scripts`) and traced every auth-lens candidate from source to sink, including a sub-agent sweep of the server side to verify which guards actually exist.

Candidates evaluated and rejected:

- **Unauthenticated REST/WS, unvalidated invite tokens, self-asserted actor headers, no Origin checks** — all real, but every root control lives in `crates/quarry-server` (router with no auth middleware, `collab_handlers.rs` never reading the `token` param, `agent-prompt` checking only non-emptiness), where the code explicitly documents a deliberate phase-one loopback posture (`lib.rs` "no auth" warning on non-loopback binds, `session.rs` comment). Not a web-ui root control; nothing in the UI creates or widens these exposures.
- **Invite token left in `?token=` URL/history** (`App.tsx:224`) — possession of the token grants nothing: the server never validates it, so there is no gated operation an attacker gains. `index.html:6` sets `no-referrer`, killing the Referer leak.
- **External `<img src>` from untrusted markdown → browser-side request forgery/tracking** (`image-element.tsx:48`, `image.ts:52` passthrough, tmp scope with no `resolveSrc` at all) — defeated by an effective defense: the server stamps every response with CSP `img-src 'self' data: blob:` (`crates/quarry-server/src/lib.rs:281-285`), so browsers block external image loads; `data:`/`blob:` in img context make no network fetch.
- **Open redirect / CSRF / IDOR / race conditions** — no cookie-based auth exists for CSRF to ride; all navigation is route-internal via React Router with `encodeURIComponent`; IDOR enforcement is server-side; the UI makes no security-relevant access decisions that could race.
- `window.open(attributes.href)` (`PlateMarkdownEditor.tsx:1128`) and the mermaid `dangerouslySetInnerHTML` are injection-lens items, outside this category.

```json
{
  "job_id": "research:002-web-ui-40ce0b0c:auth-and-access",
  "component": "web-ui",
  "lens": "authentication and authorization",
  "findings": [],
  "notes": "No panel-verifiable finding with a root control inside the web-ui component. The material authz gaps (unauthenticated REST/collab WebSocket, unvalidated invite tokens, missing Origin checks, self-asserted X-Quarry-Transaction-Actor) are all rooted in crates/quarry-server and are explicitly documented there as a deliberate phase-one loopback posture with a non-loopback startup warning; they belong to the server component's auth scan. The one UI-rooted candidate (document-controlled external image URLs rendered via image-element.tsx:48 and image.ts:52) is neutralized by the server-enforced CSP img-src 'self' data: blob: (crates/quarry-server/src/lib.rs:281-285). Invite tokens retained in the page URL grant no access because the server never validates them, and index.html sets referrer no-referrer."
}
```