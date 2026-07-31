I've completed the review. Here's a summary of what I traced before concluding:

**Sinks examined in full (each traced source→sink with every guard):**

1. **`mermaid-block.tsx:80` (`dangerouslySetInnerHTML`)** — the only DOM-injection sink. Document-controlled mermaid source flows through `markdown-codec.ts:79` → `applyMermaid` → `mermaid.render` with `securityLevel: 'strict'` (line 40), which sanitizes via DOMPurify on a current mermaid (^11.15.0). No bypass identifiable from code reading, and the server CSP is a second barrier. No complete path.
2. **`PlateMarkdownEditor.tsx:1116–1128` (link `href` + `window.open`) and `LinkOpenButton` (line 1248)** — no URL-scheme allowlist anywhere in app code, and remark passes `javascript:` destinations through verbatim. However, the only dangerous outcome (script execution) is neutralized by an **effective defense I verified**: `crates/quarry-server/src/lib.rs:281–285` sets `Content-Security-Policy: script-src 'self'` (no `unsafe-inline`) unconditionally on every response (`security_headers_middleware`, layered at lib.rs:217, covering the `browser_asset` SPA fallback at lib.rs:213). `javascript:` URL execution and inline handlers are blocked by that CSP; top-level `data:` navigation is browser-blocked. Per the reporting bar (complete path with no effective defense), this is not reportable.
3. **`image-element.tsx:48` / `image.ts:51`** — `resolveImageSrc` passes http(s)/data/blob through or confines relatives to the same-origin serve endpoint; `javascript:` in `<img src>` is inert. Safe.
4. **`raw-markdown.ts:33`** — html mdast nodes emit verbatim into a *markdown text file* (download/diff mirror); no HTML rendering context. Safe. The browser renderer (`raw-markdown-block.tsx`) shows it as escaped `<pre>` text.
5. **`endmatter.ts:29` (YAML)**, **`workspace-event-stream.ts:98` (SSE JSON → only same-origin cache invalidation/navigation)**, **`rust-ws-provider.ts` (lib0/Yjs decoding)**, **`App.tsx:1008` (objectURL download, basename-sanitized)** — all traced, no dangerous operation reachable.
6. **ReDoS sweep** — `WIKILINK_RE` (wiki-link.ts:30), inline-mark tag regexes, accept-reject CriticMarkup regexes (lazy quantifiers, escaped ids), endmatter delimiter — all linear.
7. **Prompt-injection check** — the agent invite prompt (`agent_prompt.rs`) is a static template interpolating only origin/locator/token; no document-controlled content reaches it. Rendered instructions are plain text (App.tsx:1882).

No finding met the bar of a complete, undefended source-to-sink path.

```json
{
  "findings": []
}
```