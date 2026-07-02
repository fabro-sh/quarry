# Quarry

**A shared document workspace for you and your AI agents — local-first, real-time, versioned.**

Working on a document with an AI agent today usually means one of two things: pasting walls of text back and forth in a chat window, or letting the agent rewrite files behind your back and diffing the damage afterwards. Neither feels like collaboration.

Quarry fixes that. It gives you and your agents **one live canvas**: a real-time collaborative Markdown editor where agents join as named collaborators — with their own cursors and presence — to draft alongside you, leave comments, and propose suggestions you can accept or reject. Like a Google Doc, except it runs entirely on your machine and agents are first-class participants.

```sh
quarry init .quarry
quarry serve
```

Open `http://127.0.0.1:7831`, create a document, and start typing. Then hand the document's link to any agent and tell it to join you.

## Why Quarry

- **✍️ Real collaboration, not copy-paste.** You and your agents edit the same document at the same time. Live cursors, presence, comments, and trackable suggestions — a review workflow, not a chat log.
- **🤖 Any agent can join.** If it can make HTTP requests, it can collaborate. No SDK, no browser automation. Agents read the built-in guide at `/agent-docs` (or the ready-made skill at `/quarry.SKILL.md`) and participate via a small, curl-able API.
- **🔒 Local-first and private.** One binary, one local server, your disk. No cloud account, no telemetry, nothing leaves your machine unless you send it somewhere.
- **🔗 Share with a link.** Every scratch document gets a capability URL — paste it into your agent's prompt and it has everything it needs to find the document and start working.
- **🕘 Versioned by design.** Writes create immutable versions behind a mutable head; nothing is ever destroyed in place.
- **🧱 Safe edits at block granularity.** Documents are trees of blocks with stable IDs. Agents edit, comment, and suggest against specific blocks in atomic transactions — no more "the agent rewrote the whole file to change one sentence."

## Quickstart

Install with Homebrew (macOS and Linux, pre-compiled binaries):

```sh
brew tap fabro-sh/quarry ssh://git@github.com/fabro-sh/quarry.git
brew install fabro-sh/quarry/quarry
```

Or build from source (Rust stable required):

```sh
cargo build --release -p quarry
install -Dm755 target/release/quarry ~/.local/bin/quarry
```

Or grab a tarball for your platform from [GitHub Releases](https://github.com/fabro-sh/quarry/releases) — stable releases and nightly prereleases ship macOS (Apple silicon and Intel) and Linux x86_64 binaries. Then:

```sh
quarry init .quarry
quarry serve
```

The browser workspace is embedded in the server — open `http://127.0.0.1:7831` and create a document. See [docs/operations/install-linux.md](docs/operations/install-linux.md) for details.

## Inviting an agent

Copy the document link from your browser and give it to any coding agent (Claude Code, Codex, or anything that can run `curl`):

> Join this Quarry document and wait for instructions: `http://127.0.0.1:7831/tmp/<secret>`
> Read `http://127.0.0.1:7831/agent-docs` first.

The agent registers its presence, reads the document's block tree, and shows up in your editor as a live collaborator. From there you can ask it to draft sections, review your writing with inline comments, or propose suggestions you accept and reject like tracked changes.

## Power features

Beyond the shared-canvas workflow, Quarry can act as a full document substrate for developer tools (built with the `lib-documents` feature):

- **Libraries** — path-addressed document collections with metadata and multi-document transactions.
- **Git sync** — import from and export to ordinary Git working trees, with explicit bidirectional sync that preserves both sides of conflicts. ([docs](docs/operations/git-sync.md))
- **FUSE mounts (Linux)** — browse a library with `ls`, `rg`, and `vim` like any other directory. ([docs](docs/operations/fuse.md))
- **REST API + OpenAPI** — everything above is scriptable over HTTP. ([docs](docs/operations/rest-api.md))

External edits don't trample live sessions: whole-file writes from Git, FUSE, or the CLI are merged three-way into open documents, and genuine conflicts surface as review items — never lost writes.

## Status

Quarry is young and moving fast. It is currently single-user and local-only: the server trusts loopback and ships no auth, so keep it bound to `127.0.0.1` (the default). For architecture details and an honest list of current limitations, see [docs/architecture.md](docs/architecture.md).

## Learn more

- [Architecture](docs/architecture.md) — how live sessions, block storage, and reconciliation work
- [Development guide](docs/development.md) — workspace layout, building, and running the tests
- [Operations](docs/operations/) — install, Git sync, FUSE, conflicts, backup/restore
- Agent-facing docs — served at `/agent-docs` by every Quarry server

License: MIT

If Quarry looks useful to you, a ⭐ helps other people find it.
