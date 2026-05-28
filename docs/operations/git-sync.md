# Git Import And Export

Git import reads ordinary files from a working tree into Quarry documents:

```sh
cargo run -p quarry -- git import notes /path/to/repo
```

Git export materializes Quarry documents into a working tree and creates a Git commit:

```sh
cargo run -p quarry -- git export notes /path/to/repo --branch main
```

Configured peers can be synchronized explicitly:

```sh
cargo run -p quarry -- git peer add notes /path/to/repo --branch main
cargo run -p quarry -- git peer list notes
cargo run -p quarry -- git pull notes <peer-id>
cargo run -p quarry -- git push notes <peer-id>
cargo run -p quarry -- git sync notes <peer-id>
```

Export writes `.quarry/marker.json` with the Library ID and slug. A later export refuses to write if the marker belongs to a different Library.

Markdown frontmatter is imported as metadata and can be exported back into Markdown. Non-Markdown sidecars use `path.ext.quarrymeta.yaml`.
Paths ending in `.quarrymeta.yaml` are reserved for Git metadata sidecars: import skips them as sidecars, and export refuses Quarry documents with that suffix so a document cannot be lost on a later import.

Peer sync stores a per-path baseline in `sync_state`. When both Quarry and Git change the same path from that baseline, Quarry keeps the canonical document as the local/Quarry winner, writes the Git side as a sibling `*.conflict-git-*` document, records a conflict row, and exports both files back to Git.

Peers may include a `remote` URL or local bare repository path plus a single `branch`. `pull` and `sync` fetch the configured remote before reading the working tree. `push` and `sync` push the committed export back to `refs/heads/{branch}` before advancing sync state.

Peer `pull`, `push`, and `sync` run under Quarry's global operation lock. Normal writes wait while the sync publishes its import/export results, and sync state advances only after the Git publication step succeeds.

The implemented sync path covers baseline push, both-unchanged no-op sync without an extra Git commit, one-sided import/export, both one-sided delete directions, both-deleted cleanup, both-created conflict preservation, both-changed-same-content convergence, change/delete conflicts, same-path content conflict preservation, large-delete safety aborts via `max_delete_percent`, remote fetch/push transport, and sync-state updates after successful publication.
