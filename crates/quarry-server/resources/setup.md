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

`quarry open` creates the document on the server (https://quarry.lithos.computer by default; pass `--server` to target another), opens it in the user's browser, and prints connect instructions for you. Follow them exactly: send `X-Agent-Id` on every request, register presence, read the block tree, and reply "Connected in Quarry and ready."

Then seed the review so the user has something to react to:

1. Read __QUARRY_ORIGIN__/quarry.SKILL.md first for the exact `comment.add` and `suggestion.add` op shapes. Do not guess them.
2. Add one comment anchored to text in the "Try it yourself" section, inviting the user to reply.
3. Add one suggestion fixing the grammar mistake in the "A sentence that needs work" section, so the user can accept or reject a tracked change.

While the user explores, keep the document's events stream open (`GET .../events/stream`). When a `doc.changed` event arrives, re-read `.../blocks` and `.../review` and respond to the user's comments in the document.

## Update Your Persistent Instructions

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

If one or more files exist, choose the one for the current agent and merge in any missing Quarry guidance. If the current agent cannot determine which file it loads, use its built-in memory or settings command when available, such as Claude Code's `/memory`.

If no persistent instruction file exists and the user has not specified a tool, create a portable canonical file at `${XDG_CONFIG_HOME:-$HOME/.config}/agents/AGENTS.md`, then connect vendor-specific global files to it. Do not overwrite existing files.

Use imports or symlinks only when the target agent supports normal Markdown files at that path. For UI-only settings such as Cursor User Rules or GitHub Copilot personal instructions, tell the user what text to add rather than editing an unknown file.

The canonical Quarry instruction block is hosted at __QUARRY_ORIGIN__/prompt.md. After you identify or create the right instruction file, append or merge that exact block:

```bash
agent_instructions_file="/absolute/path/to/AGENTS.md"
printf "\n\n" >> "$agent_instructions_file"
curl -fsSL __QUARRY_ORIGIN__/prompt.md >> "$agent_instructions_file"
```

If the instruction file already has Quarry guidance, merge in any missing guidance from __QUARRY_ORIGIN__/prompt.md instead of duplicating the section.

After updating your instructions, briefly tell the user which file you changed.

## Key Behaviors

- One command shares a document: `quarry open <file>` (or `quarry new` for an empty one). The printed connect instructions are the source of truth for joining that document.
- Documents are collaborative and live. You and the user can work in the document at the same time; monitor the events stream rather than polling blindly.
- Propose, don't impose. While the user is reviewing, prefer `suggestion.add` (tracked changes) and comments over direct edits, and never edit before the user asks.
- Document URLs are bearer capabilities. Anyone with the URL can read and edit the document, and documents on shared servers expire (30 days by default). Keep sensitive content off servers the user does not control, and do not log or repost document URLs.
