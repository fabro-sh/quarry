{
  "job_id": "threat:002-web-ui-40ce0b0c",
  "component": "web-ui",
  "scope": [
    "ui/src",
    "ui/public"
  ],
  "entryPoints": [
    "ui/src/app/App.tsx:225 — url-param: collab invite token read from location.search (?token=) and later used to join collab sessions",
    "ui/src/api/client.ts:147 — http-response: remote markdown document content fetched via fetch(documentRefUrl(ref)); flows into markdownToPlateValue and the editor",
    "ui/src/app/workspace-event-stream.ts:70 — server-sent-events: EventSource consuming server-pushed workspace events; payloads JSON.parse'd at ui/src/app/workspace-event-stream.ts:97",
    "ui/src/features/collab/rust-ws-provider.ts:129 — websocket: y-websocket collab session; remote peers' Yjs updates and awareness states enter the shared doc; custom checkpoint frames handled at ui/src/features/collab/rust-ws-provider.ts:141-146",
    "ui/src/features/collab/RemoteCursorOverlay.tsx — websocket-awareness: remote peer cursor/selection and peer display names from Yjs awareness rendered into the DOM",
    "ui/src/features/editor/image-element.tsx:90 — file-upload: dropped/pasted File objects enter the upload pipeline (imageAssetPath hashing + putBinaryDocument, or fileToDataUrl)",
    "ui/src/app/App.tsx:1049 — file-upload: user-selected markdown file read via file.text() and PUT as the document body",
    "ui/src/features/review/identity.ts:13 — localStorage: author identity loaded from window.localStorage and used for attribution and awareness display",
    "ui/src/app/App.tsx:4139 — localStorage: JSON.parse of quarry:recent-libraries (also tree-open state at :4163, right-pane tab at :4184, theme at :276)"
  ],
  "sinks": [
    "ui/src/features/editor/mermaid-block.tsx:80 — html-injection: dangerouslySetInnerHTML with SVG from mermaid.render of attacker-controlled document content; relies solely on mermaid securityLevel 'strict' (line 40), no DOMPurify — sanitizer bypass or config change is direct stored XSS",
    "ui/src/features/editor/PlateMarkdownEditor.tsx:1128 — url-navigation: window.open(attributes.href, '_blank') on Cmd/Ctrl+click of a link node; href comes from markdown link URLs with no scheme allowlist in this code",
    "ui/src/features/editor/image-element.tsx:48 — resource-load: <img src={resolveSrc(url)}>; url is document-controlled, passed through or mapped to the serve endpoint",
    "ui/src/features/editor/image.ts:52 — scheme-validation: regex ^(?:https?:|data:|blob:) allowlist on image src; data: SVG passes through to an <img> sink",
    "ui/src/features/editor/markdown-codec.ts:81 — deserialization: api.markdown.deserialize(markdown) converts untrusted remote markdown (incl. raw HTML mdast nodes, GFM) into Plate nodes",
    "ui/src/features/editor/remark-inline-marks.ts:67 — html-serialization: custom to-markdown handler emits <name>...</name> verbatim for mdxJsxTextElement; parser pairs raw HTML tags from untrusted input into mark nodes",
    "ui/src/features/editor/raw-markdown.ts:35 — serialization: raw_markdown blocks serialize as mdast html with verbatim node.markdown value into the local mirror (download/diff output)",
    "ui/src/app/App.tsx:1008 — object-url-download: URL.createObjectURL on fetched document blob plus programmatic anchor click to force download of remote content",
    "ui/src/api/client.ts:273 — token-in-url: collab invite token interpolated into /agent-prompt?token=... query string (URL log/history exposure); ws token passed as query param at ui/src/features/collab/rust-ws-provider.ts:127",
    "ui/src/features/editor/image.ts:29 — crypto: crypto.subtle.digest('SHA-256', file bytes) for content-addressed asset paths",
    "ui/src/features/editor/mirror-serializer.worker.ts — worker-deserialization: web worker running markdown serialize/mirror of editor content off-thread",
    "ui/src/features/collab/rust-ws-provider.ts:142 — binary-deserialization: lib0 decoding.readVarUint8Array on server-supplied checkpoint frames"
  ],
  "assumptions": [
    "ui/src/features/editor/mermaid-block.tsx:79 — assumes mermaid's built-in 'strict' securityLevel fully sanitizes all attacker-controlled diagram source; no second-layer sanitizer before dangerouslySetInnerHTML",
    "ui/src/features/editor/PlateMarkdownEditor.tsx:1117 — assumes @platejs/link getLinkAttributes/URL handling rejects dangerous href schemes (javascript:, data:); the UI performs no scheme check before window.open",
    "ui/src/features/editor/markdown-codec.ts:81 — assumes @platejs/markdown deserialize never maps raw HTML mdast nodes into executable DOM; raw HTML is expected to fall into raw_markdown text blocks (ui/src/features/editor/raw-markdown-block.tsx:12 renders as <pre> text)",
    "ui/src/features/editor/wiki-link-element.tsx:35 — wiki-link targets from the backend link index are trusted; open(path) navigates internally without further validation",
    "ui/src/app/App.tsx:1021 — assumes documentHref/tmpDocumentHref outputs are same-origin relative paths when constructing the copyable raw link",
    "ui/src/features/collab/rust-ws-provider.ts:81 — assumes window.location.host is a trusted collab endpoint (plain ws when the page is http)",
    "ui/src/features/review/identity.ts:13 — localStorage author name is treated as trusted display identity; any XSS or shared-profile tampering becomes spoofed attribution/awareness"
  ],
  "trustBoundaries": [
    "ui/src/app/document-body.tsx:58 — server-stored markdown (writable by collaborators, agents, Git/FUSE imports) crosses into the React/Plate rendering pipeline",
    "ui/src/features/collab/rust-ws-provider.ts:123 — Yjs shared doc merges updates from any peer holding the room/invite token into local editor state",
    "ui/src/features/editor/mermaid-block.tsx:41 — untrusted document text crosses into HTML via mermaid.render to dangerouslySetInnerHTML",
    "ui/src/features/editor/image.ts:51 — document-controlled image URLs cross into network fetches (<img src>) against the serve endpoint or arbitrary remote/data origins",
    "ui/src/app/App.tsx:225 — URL query (?token=) crosses into collab session authentication state",
    "ui/src/app/workspace-event-stream.ts:97 — server event JSON crosses into navigation/refetch decisions without schema validation beyond type being a string",
    "ui/src/features/editor/image-element.tsx:67 — local filesystem File bytes cross into stored server assets (content-addressed) and into the document as image references"
  ],
  "hotFiles": [
    "ui/src/features/editor/mermaid-block.tsx",
    "ui/src/features/editor/markdown-codec.ts",
    "ui/src/features/editor/remark-inline-marks.ts",
    "ui/src/features/editor/raw-markdown.ts",
    "ui/src/features/editor/raw-markdown-block.tsx",
    "ui/src/features/editor/image.ts",
    "ui/src/features/editor/image-element.tsx",
    "ui/src/features/editor/PlateMarkdownEditor.tsx",
    "ui/src/features/collab/rust-ws-provider.ts",
    "ui/src/features/collab/RemoteCursorOverlay.tsx",
    "ui/src/app/workspace-event-stream.ts",
    "ui/src/api/client.ts",
    "ui/src/api/document-ref.ts",
    "ui/src/app/App.tsx",
    "ui/src/app/document-body.tsx",
    "ui/src/features/review/identity.ts"
  ]
}