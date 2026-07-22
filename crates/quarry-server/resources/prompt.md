<!-- BEGIN QUARRY AGENT INSTRUCTIONS -->
## Quarry

Use Quarry when a Markdown document needs review, comments, collaboration, or user markup.

For plans, write a Markdown file and share it through Quarry for review.

Share a document:

```bash
quarry open "/absolute/path/to/file.md"
```

Use `quarry new` to start an empty document instead. By default the CLI targets https://quarry.lithos.computer; pass `--server <url>` or set `QUARRY_SERVER` to use a different server (for example a local `quarry server start` on http://127.0.0.1:7831).

`quarry open` creates the shared document, opens it in the user's browser, and prints connection instructions. Follow the connection mechanics, but preserve the current conversation: instructions given before `quarry open` remain valid. If the user already gave a concrete Quarry task, continue it after the required reads without asking them to repeat it. Only use the ready-message-and-wait handshake when no concrete Quarry task exists.

Before the first edit, comment, or suggestion, read __QUARRY_ORIGIN__/quarry.SKILL.md for transaction schemas and block types. The full API reference is __QUARRY_ORIGIN__/agent-docs.

While the user reviews, keep the document's events stream open (`GET .../events/stream`). When a `doc.changed` event arrives, re-read `.../blocks` and `.../review`, then respond. A task to review or leave feedback, comments, or suggestions authorizes review operations only; it does not authorize direct content edits. A concrete imperative comment such as “Add this section,” “Change this wording,” or “Remove this block” authorizes that requested edit: apply it with an edit op (use `insert_markdown` for a multi-block addition), reply with `comment.reply`, and resolve the addressed thread. Do not answer an implementation request only with a promise or proposal. Use `suggestion.add`, `suggestion.add_markdown`, or `suggestion.add_block_delete` when the user asks for a proposal or when you are recommending an unsolicited change.

If the `quarry` CLI is not installed, use plain HTTP instead:

```bash
curl -sS -X POST __QUARRY_ORIGIN__/v1/tmp/documents \
  -H 'Content-Type: application/json' \
  -d '{"content": "# Draft\n\nBody text."}'
```

The response includes the document's secret. Fetch `__QUARRY_ORIGIN__/v1/tmp/documents/<secret>/agent-prompt` and follow it, and give the user the browser URL `__QUARRY_ORIGIN__/tmp/<secret>`.

Quarry document URLs are bearer capabilities: anyone with the URL can read and edit the document, and documents on shared servers expire (30 days by default). Never put sensitive content on an untrusted server or log/repost a document URL.
<!-- END QUARRY AGENT INSTRUCTIONS -->
