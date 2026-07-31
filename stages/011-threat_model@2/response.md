{
  "component": "web-ui",
  "job_id": "threat:002-web-ui-40ce0b0c",
  "summary": "React/Plate.js browser frontend for collaborative markdown editing. Untrusted input arrives via the REST API (/v1/*), an SSE event stream, Yjs-over-WebSocket collab sessions (including tmp documents joined purely by a URL secret and library documents joined via an invite token in the URL query), pasted/dropped files, URL route/query parameters, and localStorage. Document content is untrusted markdown/Yjs data from other collaborators or agents; it is deserialized into Plate nodes, rendered (including Mermaid SVG via dangerouslySetInnerHTML and link/image URLs taken from document content), and serialized back out.",
  "entryPoints": [
    "ui/src/app/App.tsx:224 — collab invite token read from URL query (?token=) and forwarded to the collab session; URL is attacker-controllable via shared links",
    "ui/src/app/App.tsx:222 — BrowserRouter routes /lib/:library/documents/* and /tmp/:secret; library, path, and tmp secret come from the URL",
    "ui/src/api/client.ts:147 — fetch(documentRefUrl(ref)); all document content, versions, search results, and presence data enter as server JSON/markdown",
    "ui/src/app/workspace-event-stream.ts:70 — new EventSource(url); JSON.parse of server-sent events (line 98) drives navigation and document reload decisions",
    "ui/src/features/collab/rust-ws-provider.ts:129 — y-websocket provider to /v1/collab or /v1/tmp/collab/:secret; remote Yjs updates from any collaborator/agent holding the token or tmp secret hydrate the shared editor doc",
    "ui/src/features/editor/PlateMarkdownEditor.tsx:382 — content prop (server or tmp-doc markdown) deserialized by MarkdownPlugin into editor nodes; primary untrusted-markdown ingest",
    "ui/src/features/editor/markdown-codec.ts:79 — markdownToPlateValue; remark/mdast parsing of untrusted markdown (GFM, inline marks, wiki-links, mermaid, raw html mdast nodes)",
    "ui/src/features/editor/image.ts:45 — reader.readAsDataURL(file) on pasted/dropped files; upload flow inserts the resulting URL into the document",
    "ui/src/app/App.tsx:1035 — markdown file upload input; uploaded content replaces the current document",
    "ui/src/features/review/identity.ts:13 — localStorage quarry:author read and used as collaborator identity/actor headers",
    "ui/src/features/collab/collab-debug.ts:16 — URLSearchParams on location.search enables debug behaviors"
  ],
  "sinks": [
    "ui/src/features/editor/mermaid-block.tsx:80 — dangerouslySetInnerHTML with mermaid.render output from document-controlled diagram source; relies entirely on mermaid securityLevel 'strict' (line 40) for sanitization",
    "ui/src/features/editor/PlateMarkdownEditor.tsx:1116 — LinkElement renders <a> with href from document content (getLinkAttributes) and window.open(attributes.href) at line 1128; no visible URL-scheme allowlist for javascript:/data: hrefs from untrusted markdown",
    "ui/src/features/editor/image-element.tsx:48 — <img src={resolveSrc(url)}> where url comes from document markdown; scheme/content of src unvalidated in this component",
    "ui/src/app/App.tsx:1008 — URL.createObjectURL + programmatic anchor click to download server-supplied bytes; anchor.download name derived from document path",
    "ui/src/features/editor/raw-markdown.ts:33 — raw_markdown blocks serialize as mdast 'html' nodes emitting attacker-controlled markdown verbatim into the mirror/download pipeline",
    "ui/src/features/review/endmatter.ts:29 — YAML parse of trailing endmatter from untrusted document markdown into review metadata objects",
    "ui/src/features/editor/mirror-serializer.ts:59 — Web Worker (comlink) running markdown (de)serialization on untrusted content off the main thread",
    "ui/src/features/collab/rust-ws-provider.ts:141 — lib0 decoding of binary WebSocket frames (checkpoint snapshots, Yjs updates) from the server/peers",
    "ui/src/api/client.ts:449 — PUT of full document content to the server with X-Quarry-Transaction-Actor derived from localStorage author (client.ts:470); self-asserted identity",
    "ui/src/app/workspace-event-stream.ts:98 — JSON.parse of SSE payloads whose path/doc_id fields influence which documents are opened/reloaded",
    "ui/src/app/App.tsx:1102 — createCollabInvite mints document-scoped editor invite tokens and copies a token-bearing URL to the clipboard (share surface)"
  ],
  "assumptions": [
    "ui/src/features/editor/mermaid-block.tsx:79 — comment asserts 'mermaid sanitizes its output (securityLevel: strict)'; no independent sanitization of SVG before dangerouslySetInnerHTML",
    "ui/src/features/editor/PlateMarkdownEditor.tsx:324 — LinkPlugin used without a custom isUrl/allowedSchemes config; assumes Plate defaults reject dangerous href schemes",
    "ui/src/features/editor/PlateMarkdownEditor.tsx:426 — assumes the server drops blank/invalid collaborator names; awareness name comes from client-controlled localStorage",
    "ui/src/api/client.ts:466 — assumes the server treats X-Quarry-Transaction-Actor as authoritative attribution; client self-asserts the actor",
    "ui/src/features/collab/rust-ws-provider.ts:127 — assumes token-in-WS-query (sourced from the page URL) is an acceptable credential channel and that invite tokens are scoped/expired server-side",
    "ui/src/features/editor/image-element.tsx:17 — assumes resolveSrc/upload (provided by App) confine images to same-origin asset paths; the component itself performs no check",
    "ui/src/app/workspace-event-stream.ts:96 — assumes SSE payloads are well-formed and their path/doc_id values refer to legitimate documents; only 'type' is checked",
    "ui/src/api/document-ref.ts:1 — assumes encodeURIComponent path segments suffice to keep documentRefUrl same-origin and path-confined"
  ],
  "trustBoundaries": [
    "ui/src/app/App.tsx:225 — URL query -> routeCollabToken -> collab WS params: anyone with the link becomes an authenticated editor",
    "ui/src/features/collab/rust-ws-provider.ts:129 — remote Yjs updates from arbitrary token/secret holders merge into the local doc and are rendered and later persisted as document content",
    "ui/src/features/editor/markdown-codec.ts:79 — server/peer-supplied markdown string -> mdast -> Plate node tree -> React rendering (XSS-relevant crossing)",
    "ui/src/features/editor/mermaid-block.tsx:41 — document-controlled mermaid source -> SVG string -> DOM injection; the sharpest less-trusted-to-DOM boundary",
    "ui/src/app/workspace-event-stream.ts:73 — server SSE events -> workspace navigation/reload behavior",
    "ui/src/features/review/identity.ts:13 — localStorage author -> transaction actor headers and awareness identity sent to server and peers",
    "ui/src/app/App.tsx:4139 — JSON.parse of localStorage (recent-libraries, tree state) into app state"
  ],
  "hotFiles": [
    "ui/src/features/editor/mermaid-block.tsx — only dangerouslySetInnerHTML in the component; sanitization depends solely on mermaid strict mode",
    "ui/src/features/editor/PlateMarkdownEditor.tsx — link/image rendering, collab session wiring, token handling, markdown plugin config; central to the attack surface",
    "ui/src/features/editor/markdown-codec.ts — untrusted-markdown deserialization rules, including raw html mdast nodes",
    "ui/src/features/editor/raw-markdown.ts — verbatim html-node serialization escape hatch",
    "ui/src/features/collab/rust-ws-provider.ts — WS URL/token construction, binary message decoding, tmp-secret base URLs",
    "ui/src/app/App.tsx — routing, invite-token flow, agent invite/prompt fetches, downloads, localStorage state; largest file with several entry points",
    "ui/src/api/client.ts — all REST I/O, URL construction, identity headers, tmp document creation",
    "ui/src/app/workspace-event-stream.ts — SSE parsing and event handling",
    "ui/src/features/editor/image.ts — file drop/paste ingest and upload path construction",
    "ui/src/features/review/endmatter.ts — YAML parsing of document-controlled endmatter into metadata"
  ]
}