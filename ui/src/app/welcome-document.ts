// Seeds the document created by the homepage's "Start a document" link
// (`/tmp/new`). The first-run tour lives in the document itself: the fastest
// way to explain a collaborative editor is to hand the visitor a live one.
export const WELCOME_DOCUMENT = {
  title: 'Welcome to Quarry',
  content: `# Welcome to Quarry

This is a live, versioned Markdown document — and it's yours. Edit anything.

## Try this

- **Invite your coding agent.** Click **Add agent** in the toolbar and paste the instructions into Claude Code, Codex, or any agent that can run shell commands. It joins this document with its own cursor, comments, and suggestions.
- **Share this page's URL** with someone to write together in real time.
- **Switch to Suggesting** in the toolbar to propose tracked changes instead of editing directly.
- **It's all Markdown.** Download or upload the document from the toolbar menu — it round-trips cleanly to your repo or editor.

## Good to know

- Anyone with this page's URL can open it — the URL is the key, so share it deliberately.
- Scratch documents like this one expire 30 days after their last edit.
- Press ⌘K for the command palette.
`,
};
