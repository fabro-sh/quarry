# Quarry research and build plan

## Product shape and architectural thesis

Quarry should be treated as a lower-layer document substrate for future memory products, not as the memory product itself. The closest existing patterns are a mix of AgentFS’s SQLite-backed agent runtime, Obsidian’s plain-file vault model, and S3’s key-based object namespace. That combination matches your goals unusually well: libraries and documents as first-class durable records, filesystem-style paths, arbitrary metadata, binary blobs offloaded to CAS, and multiple access surfaces on top. It is much closer to the “plain files plus metadata plus projections” end of the spectrum than to a Notion-style block/rich-text system. citeturn24view4turn19search2turn14search1turn23search2turn23search5turn14search2

Turso Database is a strong fit for the core because Turso’s current guidance for new embedded projects points to the `turso` SDK and describes the engine as a SQLite-compatible rewrite with concurrent writes via MVCC, async I/O, and native Rust async/await support. The important caveat is maturity: Turso’s own comparison page still describes Turso Database as evolving in beta, while presenting libSQL as the more battle-tested option today. For Quarry phase one, that argues for a conservative core: a portable SQL schema, minimal dependence on engine-specific features, and a clean internal storage abstraction so you can fall back to libSQL or SQLite if needed without redesigning the product. citeturn28search18turn28search7turn30search2

I would seriously consider one physical database per Library, plus a tiny control-plane database for library registry, auth, mount bookkeeping, and daemon state. Turso’s own agent-database guidance explicitly documents isolated embedded databases and a database-per-agent pattern for isolation, cleanup, and security, while AgentFS packages an agent’s filesystem and state into a single SQLite file. A per-library file gives Quarry the same operational virtues: easy snapshotting, clean mount boundaries, straightforward Git sync state, and a portable unit you can copy, back up, or hand to another process. citeturn31view0turn24view4

## Data model and storage design

The right durable model is not “a mutable file row,” but “a versioned document head plus immutable versions plus immutable blobs.” S3 is useful here as a naming metaphor: object keys are unique identifiers, prefixes can be listed hierarchically, and the hierarchy is virtual rather than a true filesystem tree. In Quarry terms, each Library should own a normalized keyspace, with documents identified by `(library_id, normalized_path)` and backed by immutable committed versions. Directories do not need to exist as strong objects unless you want directory metadata later; for phase one they can be derived from path prefixes. citeturn23search2turn23search0turn23search5

The core entities I would introduce immediately are `libraries`, `documents`, `document_versions`, `transactions`, `transaction_changes`, `blobs`, `sync_peers`, `sync_state`, and `conflicts`. Make transactions first-class rows, not just implicit database behavior. Datomic is especially instructive here: it reifies transactions as entities that can carry provenance such as source, purpose, actor, and timestamp. Quarry should do the same, so every commit can store fields like `actor`, `message`, `source=rest|git|fuse`, `git_commit`, `peer_id`, and `import_metadata`. That gives you auditability now and a clean bridge to CLI, MCP, UI, and search later. citeturn25view0

For metadata, store the canonical payload as JSON and index only the hot paths you know you will query. Turso exposes SQLite’s built-in JSON support, and SQLite supports generated columns, expression indexes, and STRICT tables. That combination is ideal for Quarry: keep metadata flexible, but project a few fields into generated columns such as `mime_type`, `title`, `author`, or `tags_hash` if they become important for listing or filtering. Also reserve a place in the schema for future full-text and semantic search, because FTS5 is built in and Turso also exposes vector search support; you do not need those features in phase one, but you do want the core layout to make them additive later rather than disruptive. citeturn15search0turn16search2turn15search10turn16search21turn16search1turn1search3

Separate document identity from content identity. Perkeep’s storage model is useful here: it treats everything as content-addressed blobs and models mutable state as historical, immutable mutation records. Restic makes the same core move in a different domain: blobs are written once and then referenced from snapshots; mutation is represented by new references, not by in-place overwrites. Quarry should borrow that lesson. A committed `document_version` should point at a blob ref and metadata snapshot; a transaction commit should atomically advance document heads; and CAS garbage collection should operate by reachability after commit, never by mutating a blob in place. citeturn32view1turn32view2turn32view0

One subtle but important design guardrail: if you ever use Turso Sync for Quarry library distribution, keep Quarry’s conflict semantics above the database layer. Turso Sync’s documented policy is last-push-wins, and `pull()` works by rollback-to-last-sync, apply remote changes, and replay local changes atomically. That is fine for database sync, but it is not the same as your stated Git policy of preserving both conflicting document copies. Quarry therefore needs its own document-version and conflict-record model regardless of what the underlying database engine does. citeturn5view2

A practical schema sketch for phase one would look like this:

```sql
libraries(
  id TEXT PRIMARY KEY,
  slug TEXT UNIQUE NOT NULL,
  created_at TEXT NOT NULL,
  settings_json TEXT NOT NULL
);

documents(
  id TEXT PRIMARY KEY,
  library_id TEXT NOT NULL,
  path TEXT NOT NULL,
  head_version_id TEXT,
  deleted_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  metadata_json TEXT NOT NULL,
  UNIQUE(library_id, path)
);

document_versions(
  id TEXT PRIMARY KEY,
  document_id TEXT NOT NULL,
  tx_id TEXT NOT NULL,
  content_ref TEXT,
  inline_text TEXT,
  metadata_json TEXT NOT NULL,
  media_type TEXT,
  byte_size INTEGER,
  created_at TEXT NOT NULL
);

transactions(
  id TEXT PRIMARY KEY,
  library_id TEXT NOT NULL,
  state TEXT NOT NULL,      -- draft|committed|aborted
  actor TEXT,
  source TEXT NOT NULL,     -- rest|git|fuse
  message TEXT,
  provenance_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  committed_at TEXT
);

transaction_changes(
  tx_id TEXT NOT NULL,
  path TEXT NOT NULL,
  change_type TEXT NOT NULL, -- put|delete|rename|metadata
  old_version_id TEXT,
  new_version_id TEXT
);

blobs(
  ref TEXT PRIMARY KEY,
  byte_size INTEGER NOT NULL,
  hash_alg TEXT NOT NULL,
  storage_backend TEXT NOT NULL,
  created_at TEXT NOT NULL
);

sync_peers(
  id TEXT PRIMARY KEY,
  library_id TEXT NOT NULL,
  kind TEXT NOT NULL,        -- git
  config_json TEXT NOT NULL
);

sync_state(
  peer_id TEXT NOT NULL,
  path TEXT NOT NULL,
  last_synced_doc_version_id TEXT,
  last_synced_git_oid TEXT,
  PRIMARY KEY(peer_id, path)
);

conflicts(
  id TEXT PRIMARY KEY,
  library_id TEXT NOT NULL,
  path TEXT NOT NULL,
  ours_version_id TEXT,
  theirs_version_id TEXT,
  discovered_at TEXT NOT NULL,
  resolved_at TEXT
);
```

## API, transactions, and Git sync semantics

The REST API should expose Quarry’s real write model instead of flattening everything into generic CRUD. SQLite-compatible systems already distinguish implicit autocommit transactions from explicit `BEGIN … COMMIT` units, and HTTP already gives you the primitives you need to make those writes safe: `PATCH` for partial modifications, `ETag` plus `If-Match` to prevent mid-air collisions, and `If-None-Match: *` for create-if-absent semantics. In practice, that means Quarry should have explicit transaction resources for multi-document work, while still supporting an auto-commit path for single operations. citeturn5view4turn18search2turn18search1turn18search7turn18search10

A clean phase-one API shape would be:

```http
POST   /v1/libraries
GET    /v1/libraries/{library}
GET    /v1/libraries/{library}/documents?prefix=notes/&limit=...
GET    /v1/libraries/{library}/documents/{path}
PUT    /v1/libraries/{library}/documents/{path}          # auto-commit full replace
PATCH  /v1/libraries/{library}/documents/{path}/metadata # auto-commit metadata patch
DELETE /v1/libraries/{library}/documents/{path}

POST   /v1/libraries/{library}/transactions
PUT    /v1/libraries/{library}/transactions/{tx}/documents/{path}
PATCH  /v1/libraries/{library}/transactions/{tx}/documents/{path}/metadata
DELETE /v1/libraries/{library}/transactions/{tx}/documents/{path}
POST   /v1/libraries/{library}/transactions/{tx}/commit
POST   /v1/libraries/{library}/transactions/{tx}/rollback

POST   /v1/libraries/{library}/git/peers
POST   /v1/libraries/{library}/git/peers/{peer}/pull
POST   /v1/libraries/{library}/git/peers/{peer}/push
GET    /v1/libraries/{library}/conflicts
POST   /v1/libraries/{library}/conflicts/{conflict}/resolve
```

For Git, I would keep the working tree human-first: ordinary files at their natural paths, plus a reserved `.quarry/` subtree for metadata sidecars, tombstones, and sync bookkeeping. That design preserves the ergonomics that make plain-file systems like Obsidian inspectable, while avoiding the trap of pretending Git natively models arbitrary per-document metadata. The critical detail is state tracking: for each sync peer and path, store the last synced Quarry document version and the last synced Git blob/tree identity, so Quarry can do real three-way reasoning instead of naïve timestamp mirroring. citeturn14search1turn5view1

When both Git and Quarry changed the same path since the common base, create an explicit Quarry conflict and preserve both versions rather than trying to force Git’s merge semantics onto the product. CouchDB is the best conceptual reference here: during replication, divergent document revisions can both exist, and the system preserves enough state for a later, explicit resolution step. That matches your stated requirement much more closely than either Turso Sync’s last-push-wins behavior or Git’s default low-level merge behavior. In practice, I would preserve the current visible document at the canonical path and materialize the alternate copy under a deterministic sibling or reserved conflict namespace, while also recording a durable conflict object in Quarry’s API. citeturn26search0turn26search3turn26search9turn5view2

Do not rely on Git’s built-in binary merge handling to implement Quarry conflict preservation. Git’s built-in text merge driver writes conflict markers into text files, while the built-in binary driver keeps the current branch’s version in the work tree and leaves the path conflicted for the user to resolve. If Quarry wants “store both copies,” that has to be Quarry logic. On the implementation side, `git2-rs` backed by libgit2 is the safest path today because libgit2 explicitly exposes merge-base calculation, tree/commit merge APIs that return an index with conflicts, and index conflict entry APIs. `gitoxide` is increasingly attractive if pure Rust is a strategic goal; its own feature inventory now lists clone, fetch, push, status, merge, worktree checkout, and reset, which makes it plausible as a later all-Rust implementation target. citeturn5view0turn22view0turn22view1turn22view2turn22view3turn21view1

Large binary files are the one place where Git sync can become painful fast. Git LFS stores pointer files in Git and data on an LFS server; git-annex moves file contents into a key-value store and versions symlinks; DataLad builds on Git plus git-annex to handle arbitrarily large datasets. Quarry should borrow the lesson without blocking phase one on full LFS protocol support: define a size threshold, define what happens when binaries exceed that threshold, and document that very large Git-exported binaries are a separate concern. That matters even more if your users sync to GitHub, because GitHub blocks files larger than 100 MiB and recommends Git LFS for that case. citeturn24view7turn24view6turn17search4turn11search3

## FUSE design and operating system constraints

The FUSE filesystem should be a projection layer over a Library, not the canonical source of truth. The safest sequence is simple: start with read-only mounts over committed state; add read-write autocommit mounts where each close/rename/delete operation becomes a tiny Quarry transaction; then, only if the UX proves worth it, add an explicit-transaction mount mode that exposes a control namespace such as `.quarry/txn/commit` and `.quarry/txn/rollback`. That sequencing keeps the first filesystem experience useful for agents immediately—`ls`, `find`, `grep`, `ripgrep`, editors, shell tools—without forcing you to solve the hardest semantics on day one. AgentFS’s own design is a good signal here: filesystem projection is powerful, but the product value comes from pairing that projection with durable state, isolation, and auditability underneath. citeturn24view4turn19search2turn19search6turn19search16

FUSE’s low-level semantics matter enough that Quarry should design to them explicitly. libfuse documents that permission checks can be delegated to the kernel with `default_permissions`; `readdir` can either stream entire directories or page through offsets; `rename` may be asked to do `RENAME_NOREPLACE` or atomic `RENAME_EXCHANGE`; extended attributes are part of the interface; and writeback caching changes behavior in ways that surprise many implementers, including kernel-originated reads on `O_WRONLY` files and kernel-handled `O_APPEND` semantics. In other words, the filesystem layer is not just a dumb path adapter over SQL rows. If you support writable mounts, the Quarry daemon needs a real inode/path cache, consistent rename behavior, and explicit decisions about caching and append safety. citeturn8view3turn8view1turn8view0turn6search1turn6search4turn6search5

For the Rust implementation, the lowest-risk recommendation is Linux-first. `fuser` is an active Rust rewrite of the low-level FUSE userspace library and has ongoing Linux/BSD/macOS test coverage work; `fuser-async` adds async syscall handlers on top of the same model; `fuse3` is attractive because it advertises async direct I/O and `readdirplus`, but its own docs still say macOS support is not there. On macOS, the ecosystem is moving—macFUSE now talks about newer user-space backends and broad platform support—but that is still extra operational surface area you do not need for Quarry v1. So the pragmatic plan is Linux read-only first, Linux read-write second, and cross-platform polish later. citeturn27search10turn27search16turn27search21turn9search5turn27search4turn27search2turn27search12

## Similar systems and what Quarry should borrow

The most directly relevant “memory layer” products are Supermemory, Mem0, TrueMemory, and QMD, but they sit mostly above the storage substrate you are building. Supermemory positions itself as context infrastructure for agents with persistent memory, retrieval, connectors, and a memory graph; Mem0 presents itself as a universal self-improving memory layer; TrueMemory emphasizes a local, single-SQLite-file memory system with layered retrieval; and QMD is a local search engine plus MCP server that stores indexed content in SQLite with FTS5, vectors, and reranking. Quarry should not try to clone those products in phase one. Instead, it should make them easy to build on top of: durable documents, version history, filesystem projection, Git interop, and a future event stream for indexing. citeturn24view1turn24view2turn24view3turn29view0turn29view1turn29view3

AgentFS is the closest cousin to Quarry on the filesystem side. Its core idea is that agents benefit from a POSIX-like filesystem, a key-value store, and a tool-call audit trail backed by one SQLite database file, with copy-on-write isolation and optional MCP/NFS surfaces on top. The key lesson is not that Quarry should become AgentFS; it is that agent-facing filesystem projection is valuable precisely because the underlying state is queryable, portable, and durable. Quarry can borrow that lesson while remaining more general-purpose and document-centric. citeturn24view4turn19search2turn19search0turn19search4

Perkeep is the clearest analogue for CAS plus mutable-over-immutable design. It stores content as content-addressed blobs, uses JSON schema blobs for higher-level structure, and represents mutable state as timestamped mutation records over immutable content. Quarry does not need Perkeep’s full schema system, but it can absolutely borrow the architectural pattern: immutable content objects, explicit mutations, rebuildable indexes, and a clean separation between storage and higher-level interpretation. citeturn32view1turn32view2turn24view5

For Git and large-content handling, the “family resemblance” is strongest with git-annex, Git LFS, and DataLad. git-annex moves file contents into a key-value store and versions symlinks, Git LFS leaves pointer files in Git and stores real content elsewhere, and DataLad layers dataset/versioning workflows on top of Git plus git-annex. Quarry should borrow their core idea that Git is a great coordination surface but not always a great bulk-blob store. That is exactly why preserving a first-class Quarry CAS, separate from Git transport concerns, is the right call. citeturn24view6turn24view7turn17search4turn17search20

For conflict and version-control semantics, lakeFS and CouchDB are both useful reference points. lakeFS shows how Git-like semantics—branching, committing, merging, rollback—can sit above object storage as a metadata/control layer rather than as the underlying storage itself. CouchDB shows a replication model where conflicting versions are preserved and later resolved, instead of being silently discarded. Quarry is narrower than lakeFS and lower-level than CouchDB’s document API, but those are the two best precedents for the semantics you appear to want. citeturn24view8turn26search0turn26search6turn24view9

## Delivery plan and open questions

The best phase-one delivery order is to lock the storage contract before you build the edges. First, build the per-library database layout, immutable version model, CAS tables, transaction tables, and conflict tables. Keep all writes going through the same commit path whether they originate from REST, Git, or FUSE. That is the one decision that will pay off everywhere else. It also keeps you aligned with the “embedded local-first database” pattern Turso recommends for agent workflows. citeturn31view0turn28search18

Next, ship the REST API with both auto-commit and explicit transactions. The big win here is not convenience; it is semantic clarity. If Quarry says “begin → edit drafts → commit,” then the API should literally expose that flow, with conditional write protection using `ETag` and `If-Match` where appropriate. Once that exists, it becomes the canonical behavioral spec for every other surface. citeturn5view4turn18search1turn18search2turn18search10

Then implement Git sync as a peer adapter, not as a second storage engine. Start with one remote, one branch, explicit `pull` and `push`, no background daemonism, and a narrow, documented conflict policy that always preserves both copies. Build the sync algorithm around stored ancestry (`last_synced_git_commit`, `last_synced_doc_version`) so you are implementing a deterministic merge, not file mirroring. Only after that is stable should you add niceties like polling, watch mode, richer bidirectional metadata mapping, or LFS-aware behavior. citeturn26search0turn5view0turn22view0turn24view7

After Git, add the FUSE projection in the least risky order: read-only mount, then writeable autocommit mount, then optional explicit transaction control files if you still want them. That order aligns with libfuse’s complexity profile and with the reality that most of the value for agents arrives as soon as standard tools can read and navigate the library. It also gives you usable mounts without prematurely entangling filesystem buffering semantics with your transaction UX. citeturn8view1turn8view3turn27search10turn27search21

Finally, harden the operational layer: garbage collection based on blob reachability, encrypted local databases if sensitive memory is in scope, observability around sync and mount operations, and invariant testing for crash safety. Turso Database already supports encrypted databases, and systems like restic show why reachability-based storage order and atomic writes matter when blobs, indexes, and snapshots are all interrelated. Quarry does not need restic’s full repository design, but it does need the same discipline around atomic publication and eventual cleanup. citeturn30search6turn32view0

The biggest open questions that will materially affect the design are these:

- Should a Library map to one physical database file, or do you want many Libraries inside one database for operational reasons?
- Do you want Git sync to target only explicit sync commands, or should it eventually support ambient/watch-based sync?
- For writable FUSE mounts, do you want edits to auto-commit on close by default, or do you want a visible explicit transaction control namespace?
- Are very large binary documents allowed to sync into Git in phase one, or is it acceptable to set a threshold and defer Git LFS-style behavior?
- Is Quarry single-tenant first, or do you expect multi-user auth, ACLs, and shareable hosted libraries very early?
- Do you want delete history and tombstones to be permanent by default, or do you need a true purge/excision story for sensitive documents?