# Conflict Resolution

Quarry never writes inline Git conflict markers into canonical documents. When sync detects a true conflict, it preserves the Quarry side at the original path, writes the Git side as a sibling document named like `notes/plan.md.conflict-git-2026-05-27T14-30-00Z`, and records an open conflict row.

List conflicts for a Library:

```sh
cargo run -p quarry -- conflicts list notes
```

Inspect the canonical document and any conflict sibling through the CLI, REST API, FUSE mount, or an exported Git worktree:

```sh
cargo run -p quarry -- get notes notes/plan.md
cargo run -p quarry -- get notes notes/plan.md.conflict-git-2026-05-27T14-30-00Z
```

Resolve by choosing the final canonical content, writing it back to the original path, and then marking the conflict record resolved:

```sh
cargo run -p quarry -- put notes notes/plan.md /tmp/resolved-plan.md
cargo run -p quarry -- conflicts resolve notes <conflict-id>
```

If the correct resolution is deletion, delete the canonical document instead of writing replacement content:

```sh
cargo run -p quarry -- delete notes notes/plan.md
cargo run -p quarry -- conflicts resolve notes <conflict-id>
```

Resolving a conflict updates only the conflict record status and `resolved_at` timestamp. It does not merge content, delete conflict sibling documents, or remove canonical documents. Delete any `*.conflict-git-*` document after you no longer need it.

Conflict records are scoped to one Library. REST and CLI resolution reject a conflict ID from a different Library.
