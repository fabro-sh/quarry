# Quarry Agent Setup

You are setting yourself up to use Quarry, a shared real-time document workspace for people and AI agents.

Quarry opens Markdown documents in a browser editor where the user reads, edits, comments, and suggests changes while you stay connected to the same document over plain HTTP — with live cursors, presence, threaded comments, and tracked-change suggestions.

## Check Installation

Check whether the Quarry CLI is available:

```bash
quarry help
```

If Quarry is missing and the user has asked you to install it, install it with Homebrew (macOS and Linux):

```bash
brew tap fabro-sh/quarry https://github.com/fabro-sh/quarry.git
brew install fabro-sh/quarry/quarry
```

If Homebrew is configured to require tap trust, run this after `brew tap` and before `brew install`:

```bash
brew trust --tap fabro-sh/quarry
```

If the user did not explicitly ask you to install software, ask before installing.

The CLI is a convenience, not a requirement: every Quarry operation is plain HTTP against __QUARRY_ORIGIN__ (see __QUARRY_ORIGIN__/agent-docs). If you cannot install software, you can still create and join documents with `curl`.

## Open an Example Document

Create and share an example document so the user can try the review workflow:

```bash
example_file="$(mktemp -d)/quarry-example.md"
curl -fsSL __QUARRY_ORIGIN__/example.md -o "$example_file"
quarry open "$example_file"
```

`quarry open` creates the shared document on the server (https://quarry.lithos.computer by default; pass `--server` to target another), opens it in the user's browser, and prints connection instructions. Follow them exactly.

Then seed the review so the user has something to react to:

1. Read __QUARRY_ORIGIN__/quarry.SKILL.md first for the exact comment and suggestion op shapes. Do not guess them.
2. Add one comment anchored to text in the "Try it yourself" section, inviting the user to reply.
3. Add one suggestion fixing the grammar mistake in the "A sentence that needs work" section, so the user can accept or reject a tracked change.

While the user explores, keep the document's events stream open (`GET .../events/stream`). When a `doc.changed` event arrives, re-read `.../blocks` and `.../review` and respond to the user's comments in the document.

## Install or Refresh Your Persistent Instructions

Add Quarry guidance to the persistent instruction file this agent will actually load. Prefer global or user-level instructions, because Quarry is a cross-project workflow.

First inspect the user's existing setup. Do not create a new instruction file when an appropriate one already exists.

Common current locations:

```text
OpenAI Codex:        ${CODEX_HOME:-$HOME/.codex}/AGENTS.md
Claude Code:         $HOME/.claude/CLAUDE.md
Gemini CLI:          $HOME/.gemini/GEMINI.md
opencode:            ${XDG_CONFIG_HOME:-$HOME/.config}/opencode/AGENTS.md
Cursor:              Cursor Settings > Rules for global user rules; project AGENTS.md or .cursor/rules/*
VS Code Copilot:     GitHub/VS Code settings for personal instructions; project .github/copilot-instructions.md, .github/instructions/*.instructions.md, or AGENTS.md
```

Check for existing files before editing:

```bash
find \
  "${CODEX_HOME:-$HOME/.codex}" \
  "$HOME/.claude" \
  "$HOME/.gemini" \
  "${XDG_CONFIG_HOME:-$HOME/.config}/opencode" \
  "$PWD" \
  -maxdepth 3 \
  \( -name "AGENTS.md" -o -name "CLAUDE.md" -o -name "GEMINI.md" -o -name "copilot-instructions.md" -o -name "*.instructions.md" \) \
  2>/dev/null
```

If one or more files exist, choose the one the current agent loads and refresh its Quarry block using the process below. If the current agent cannot determine which file it loads, use its built-in memory or settings command when available, such as Claude Code's `/memory`.

If no persistent instruction file exists and the user has not specified a tool, create a portable canonical file at `${XDG_CONFIG_HOME:-$HOME/.config}/agents/AGENTS.md`, then connect vendor-specific global files to it. Do not overwrite existing files.

Use imports or symlinks only when the target agent supports normal Markdown files at that path. For UI-only settings such as Cursor User Rules or GitHub Copilot personal instructions, tell the user what text to add rather than editing an unknown file.

The canonical Quarry instruction block is hosted at __QUARRY_ORIGIN__/prompt.md and delimited by stable markers:

```text
<!-- BEGIN QUARRY AGENT INSTRUCTIONS -->
<!-- END QUARRY AGENT INSTRUCTIONS -->
```

After identifying or creating the right instruction file, fetch the canonical block:

```bash
canonical_quarry_block="$(mktemp)"
curl -fsSL __QUARRY_ORIGIN__/prompt.md -o "$canonical_quarry_block"
```

Update the instruction file idempotently:

1. If the marker pair already exists, replace the entire marked block with the fetched block.
2. Otherwise, if a legacy unmarked `## Quarry` section exists, replace that section through the next level-two heading (or end of file) with the fetched block. Preserve any user-authored Quarry rules that are not part of the old canonical section by moving them outside the managed markers.
3. Otherwise, append the fetched block with one blank line separating it from existing content.

Never add a second Quarry heading or marker pair, and never overwrite unrelated instructions. Re-read the file afterward, verify that each marker appears exactly once and the surrounding content is unchanged, then remove the temporary canonical block.

After updating your instructions, briefly tell the user which file you changed.

## Key Behaviors

- Share a document with `quarry open <file>` (or use `quarry new` for an empty one). Its printed connection instructions are the source of truth.
- Documents are live and collaborative. Monitor the events stream while the user reviews, and re-read the document after activity.
- Never edit before the user asks. A concrete imperative comment is an edit
  request for that scoped change: perform it, reply, and resolve the thread.
  Do not merely promise the requested edit. Use suggestions when the user asks
  for a proposal or for unsolicited changes you discover during review.
- Document URLs are bearer capabilities, and shared-server documents expire (30 days by default). Never put sensitive content on an untrusted server or log/repost a document URL.
