<!-- BEGIN QUARRY AGENT INSTRUCTIONS -->
## Quarry

Use Quarry when a Markdown document needs review, comments, collaboration, or user markup.

For plans, write a Markdown file and share it through Quarry for review.

### Share a newly written document

To share a document you've written:

```bash
quarry open "/absolute/path/to/file.md"
```

`quarry open` creates the shared document, opens it in the user's browser, and prints connection instructions. Follow the connection mechanics, but preserve the current conversation. Instructions given before the document opens remain valid. If the user already gave a concrete Quarry task, continue it after the required reads without asking the user to repeat it. Use the ready-message-and-wait handshake only when no concrete Quarry task exists.

### Editing existing documents

When reviewing substantive changes to an existing Markdown document, first open it in Quarry and use suggestions for proposals or unsolicited changes. Apply a direct edit only when the user asks for that concrete change. Leave suggestions for the user to accept or reject.

When the review process is complete, sync the content from Quarry back to the file on disk.

### Using Quarry

Before the first edit, comment, or suggestion, read __QUARRY_ORIGIN__/quarry.SKILL.md for transaction schemas and block types. The full API reference is __QUARRY_ORIGIN__/agent-docs.

While the user reviews, keep the document's events stream open (`GET .../events/stream`). When a `doc.changed` event arrives, re-read `.../blocks` and `.../review`, then respond. A review or feedback request authorizes comments and suggestions only. A concrete comment such as ‘Add this section’ or ‘Remove this block’ authorizes that direct edit: apply it, reply with comment.reply, and resolve the thread with comment.resolve. Use suggestion.add, suggestion.add_markdown, or suggestion.add_block_delete when the user asks for a proposal or when you recommend an unsolicited change.

If the `quarry` CLI is not installed, use plain HTTP instead:

```bash
curl -sS -X POST __QUARRY_ORIGIN__/v1/tmp/documents \
  -H 'Content-Type: application/json' \
  -d '{"content": "# Draft\n\nBody text."}'
```

The response includes the document's secret. Fetch `__QUARRY_ORIGIN__/v1/tmp/documents/<secret>/agent-prompt` and follow it, and give the user the browser URL `__QUARRY_ORIGIN__/tmp/<secret>`.

Quarry document URLs are bearer capabilities: anyone with the URL can read and edit the document, and documents on shared servers expire (30 days by default). Never put sensitive content on an untrusted server or log/repost a document URL.
<!-- END QUARRY AGENT INSTRUCTIONS -->
