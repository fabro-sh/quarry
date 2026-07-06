## Quarry

Use Quarry when the user wants to review, comment on, or collaborate on a Markdown document, or when you want to hand the user a document they can mark up while you respond.

When the user asks for a plan, write it as a Markdown file, then share it through Quarry for review.

Share a document:

```bash
quarry open "/absolute/path/to/file.md"
```

Use `quarry new` to start an empty document instead. By default the CLI targets https://quarry.lithos.computer; pass `--server <url>` or set `QUARRY_SERVER` to use a different server (for example a local `quarry server start` on http://127.0.0.1:7831).

`quarry open` creates the document, opens it in the user's browser, and prints connect instructions for you. Follow the printed instructions exactly: send `X-Agent-Id` on every request, register presence, read the block tree, reply "Connected in Quarry and ready.", and do not edit until the user asks.

Read __QUARRY_ORIGIN__/quarry.SKILL.md before your first edit, comment, or suggestion. It defines the transaction op shapes and the block-type vocabulary; do not guess them. The full API reference is at __QUARRY_ORIGIN__/agent-docs.

While the user reviews, keep the document's events stream open (`GET .../events/stream`). When a `doc.changed` event arrives, re-read `.../blocks` and `.../review`, then respond: reply to comments with `comment.reply`, resolve addressed threads with `comment.resolve`, and propose edits with `suggestion.add` — a tracked change the user can accept or reject — rather than editing directly while a review is in progress.

If the `quarry` CLI is not installed, use plain HTTP instead:

```bash
curl -sS -X POST __QUARRY_ORIGIN__/v1/tmp/documents \
  -H 'Content-Type: application/json' \
  -d '{"content": "# Draft\n\nBody text."}'
```

The response includes the document's secret. Fetch `__QUARRY_ORIGIN__/v1/tmp/documents/<secret>/agent-prompt` and follow it, and give the user the browser URL `__QUARRY_ORIGIN__/tmp/<secret>`.

Quarry document URLs are bearer capabilities: anyone with the URL can read and edit the document, and documents on shared servers expire (30 days by default). Do not put sensitive content in documents on servers you do not control, and do not log or repost document URLs.
