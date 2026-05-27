# Quarry: Planning Research Report

## TL;DR

- **Build Quarry on `rusqlite` (or `libsql`) for now, not `turso` directly** — Turso's own README still flags it as BETA and its Rust `Transaction` type in 0.6.1 is literally an empty stub (`pub struct Transaction {}`); the `turso` crate also lacks incremental BLOB I/O, lists "no multi-process access" as a limitation, and has had multithreading regression bugs filed as recently as January–May 2026 (issues #4877, #5688, #6676). Design a thin `Storage` trait so you can swap to `turso` once it stabilizes without rewriting consumers.
- **The two hardest design risks are Git↔Library two-way reconciliation and FUSE-on-the-same-process-as-Turso** — neither is documented by Turso, and prior art (rclone bisync, Syncthing, JuiceFS, gcsfuse, Perkeep) all show that the right pattern is: (a) snapshot-based three-way diff against a "last-known-synced" state, (b) store-both-copies conflict markers (rclone's `..path1`/`..path2` suffix pattern), and (c) one `turso::Database` shared across REST/FUSE tasks with a fresh `Connection` per task plus explicit `Error::Busy` retry. Spike these in Phase 0 before committing to architecture.
- **Phase one should ship in 6 increments**: (1) core storage + CAS, (2) transactions + REST API, (3) Git import/export (one-way each direction), (4) bidirectional Git sync with conflict capture, (5) FUSE read-only, (6) FUSE read-write + packaging. Keep web UI, CRDT, and full-text search out of scope but reserve a stable internal `Storage` and `GitSync` trait so they're cheap to bolt on later.

---

## Key Findings

- **Use `rusqlite` or `libsql` behind a `Storage` trait for Phase 1; reserve a `turso` impl for later.** Turso's own quickstart recommends `turso` for new local-first projects, but the same docs concede *"At this point, libSQL is production ready, Turso Database is not — although it is evolving rapidly."* For a project where atomic multi-document transactions are foundational, an empty `Transaction` struct in the latest `turso 0.6.1` is disqualifying.
- **Use `fuser` + `fuser-async` for the FUSE layer.** `fuse3` and `polyfuse` are Linux-only; Quarry's laptop-tool positioning requires macOS support via FUSE-T or macFUSE.
- **Use `git2` (libgit2) for Phase 1 Git operations.** gitoxide is impressive but still lacks push, full merge, and reset. Migrate hot read paths to `gix` opportunistically later.
- **Use BLAKE3 + a hand-rolled disk CAS for Phase 1**, structured like `iroh-blobs`'s on-disk layout. Don't pull in the full `iroh-blobs` (and QUIC) dependency unless you need P2P.
- **Use `axum` + `utoipa-axum` for REST + OpenAPI.** This is the unambiguous 2026 community default for new async Rust API services.
- **Bidirectional sync is a three-way diff problem, not a two-way one.** rclone bisync's algorithm (persistent prior listing, conflict-loser-suffix, `--max-delete` safety check, `RCLONE_TEST` sentinel) is the right pattern to copy.
- **Closest architectural sibling is JuiceFS** — same shape (metadata in SQL, data in object store, FUSE on top). Their schema and the May 2025 1.3 release notes are required reading.

---

## 1. Similar Systems and Prior Art

### 1.1 Git-backed Knowledge Stores / PKM Tools

**Obsidian (with Git plugin).** Closed-source Electron app; vault is a directory of Markdown files plus a `.obsidian/` config dir. The Git plugin is community-maintained and uses isomorphic-git — single-user only, no true bidirectional reconciliation. **Lesson:** "files on disk + Git" is the *de facto* PKM lingua franca; Quarry's FUSE mount should make a Library look like an Obsidian vault from outside. **What to avoid:** Obsidian's plugin model assumes a single editor; it has no abstraction for "another writer" (an agent) operating concurrently. Quarry's transaction layer is the bit Obsidian conspicuously lacks.

**Logseq** (AGPL-3.0; Clojure/ClojureScript; active). Two storage modes: file graphs (Markdown/Org-mode files on disk) and the newer **DB graphs** (SQLite-backed). The DB version added an RTC (Real-Time Collaboration) sync feature in alpha. **Lesson:** Logseq is the closest sibling to Quarry in the "should the canonical store be files or a DB?" debate — they kept files as the user-facing format but added a SQLite index for queries and RTC. Quarry is essentially the inverse: SQLite as canonical, files-on-disk as a projection via Git/FUSE. **What to avoid:** Logseq's dual-storage migration has been bumpy; data-loss reports during DB-graph migration are common. Keep a single source of truth.

**Anytype** (open-source; active, large team). Object-relational model — notes are typed objects, not Markdown files. Local-first, end-to-end encrypted, P2P sync. **Lesson:** typed metadata on documents is genuinely useful; Quarry's "arbitrary metadata per document" should at least sketch a path to typed objects later. **What to avoid:** Anytype's data format is opaque and not portable as plain files; this is exactly the lock-in Quarry should reject.

**Foam** (MIT; TypeScript; VS Code extension; semi-active). Markdown + wikilinks living inside a VS Code workspace. **Lesson:** the simplest possible model — a directory of Markdown files — is a viable PKM if your editor is good enough. **What to avoid:** Foam has no schema, no transactions, no concurrency story.

**Dendron** (MIT; TypeScript; in maintenance — the founder stepped away and the project's velocity has dropped sharply since 2023). Hierarchical dot-notation namespaces (`project.notes.todo.md`) over Markdown files. **Lesson:** hierarchical keys with `.` separators are isomorphic to S3-style `/` keys; the FUSE mount makes either feel natural. **What to avoid:** Dendron's decline is a reminder that PKM tools that bet on a single editor (VS Code in their case) tend to wither when that editor changes — Quarry's REST API + FUSE strategy hedges against this.

**Silverbullet** (MIT; TypeScript; active). Markdown + Lua scripting in a self-hosted PWA. **Lesson:** plain-Markdown-plus-scripting is a viable middle ground. **What to avoid:** Silverbullet is single-user web app, not designed for multi-actor.

**Trilium / TriliumNext** (AGPL-3.0; TypeScript/JS; the original project's repo was handed to the community as `TriliumNext/Trilium` after the original maintainer Zadam declared the upstream in maintenance mode; the community fork is now active and the user-facing migration is described as "no special migration steps … simply install TriliumNext/Trilium as usual and it will use your existing database"). Stores notes in a SQLite DB, not files. **Lesson:** *exact* prior art for "SQLite as canonical, opaque blob storage for attachments, REST-ish sync." **What to avoid:** Trilium has no Git export at all; users repeatedly request it. Quarry's bidirectional Git is the differentiator.

**Joplin** (AGPL-3.0; TypeScript; active). End-to-end encrypted notes, sync via WebDAV/Nextcloud/Dropbox. **Lesson:** "treat the sync target as a dumb blob store with versioned files" is much simpler than CRDT — and Joplin's experience shows it works for single-user across devices. **What to borrow:** Joplin's conflict handling is "create a copy in the Conflicts folder" — *this is exactly the naive model Quarry wants for Phase 1;* steal it directly.

**Quartz** (MIT; TypeScript; active). Static-site generator for Markdown vaults, used to publish Obsidian/Logseq notes as websites. **Lesson:** if Quarry exposes Markdown via FUSE, Quartz becomes a downstream consumer for free.

**Athens Research** (MIT; Clojure; abandoned since 2022). Datalog-backed Roam-clone. **Lesson:** Datalog over blocks is powerful for graph queries; if Quarry adds typed metadata later, consider Datalog as a query language. **What to avoid:** Athens died because the team tried to do everything at once. Resist the urge to build CRDT in Phase 1.

### 1.2 Agent Memory Frameworks

**Mem0** (Apache-2.0; Python; ~47K+ GitHub stars per the Atlan 2026 framework comparison, ~48K per the TokenMix benchmark roundup — both put it as the largest open-source agent-memory framework by community). User-preference-focused memory with hybrid vector+graph+KV store. **Lesson:** the storage layer Mem0 actually uses underneath (Qdrant + Neo4j + PG) is roughly Quarry-shaped — keyed documents with metadata. Mem0 is essentially "Quarry with embeddings and a fact-extraction LLM layer on top." Position Quarry as the layer beneath, not a competitor.

**Letta (formerly MemGPT)** (Apache-2.0; Python; active). OS-inspired tiered memory (in-context, external paged storage); a full stateful agent runtime. **Lesson:** Letta's recent "Is a Filesystem All You Need?" benchmarks demonstrate that filesystem-style storage (which is exactly Quarry's model via FUSE) is competitive with vector-only memory systems for long-horizon coherence. This is a powerful endorsement of Quarry's design hypothesis. **What to borrow:** Letta's distinction between "core memory" (always in context) and "archival memory" (paged) maps cleanly to Quarry metadata flags (`pinned: true`).

**Zep** (Apache-2.0 for Graphiti core; SaaS Zep Cloud is proprietary; active). Temporal knowledge graph for entity-relationship-over-time queries. AgentMarketCap's April 2026 comparison reports *"On Deep Memory Retrieval, Zep scores 94.8% accuracy versus MemGPT's 93.4%."* **Lesson:** if Quarry exposes its CAS-keyed history correctly, "what did the agent know at time T" queries become free — Zep's killer feature without Zep's complexity.

**Cognee** (Apache-2.0; Python; active). Pipeline for converting raw docs → graph → vector memory. **Lesson:** Cognee's transform pipeline treats raw docs as input, exactly what Quarry stores. Build Quarry as the *source* substrate Cognee or similar can pull from.

**Supermemory / Truememory (Memori)** (proprietary SaaS / various open-source forks). Position themselves as "personal memory as a service." Local-first alternatives (per Omegamax's 2026 comparison) post higher LongMemEval scores than the cloud-managed offerings (95.4% vs Zep's 71.2%). **Lesson:** Quarry's local-first stance is timely; the market is moving local. **What to borrow:** their MCP-server packaging — Supermemory ships an MCP server, which is Quarry's planned future surface.

**OpenAI Memory** (proprietary, opaque). Closed implementation; user reports indicate a flat fact list, no temporal model. **Lesson:** if OpenAI's commercial offering is a flat fact list, Quarry's structured Library/key-path model is already a richer substrate.

**mem-agent** (open-source). Agent with explicit memory tools. **Lesson:** the tool surface to expose to agents is small — `read(key)`, `write(key, content, metadata)`, `list(prefix)`, `search(query)`. Plan the MCP server around these.

**A-MEM (Agentic Memory)** (open-source research; paper 2024). Self-organizing memory with Zettelkasten-inspired links. **Lesson:** structured links between documents (metadata key `links: [...]`) is a low-cost feature that pays off later for graph queries.

### 1.3 Local-first / Sync-engine Systems

**Automerge** (MIT; Rust core + JS/WASM bindings; active). Per the official Automerge blog (`automerge.org/blog/automerge-3/`), **Automerge 3.0 shipped in July 2025**, cutting memory usage ~10× and yielding a striking load-time win the team highlighted verbatim: *"we recently had an example of a document which hadn't loaded after 17 hours loading in 9 seconds!"* Memory for a Moby Dick-sized document fell from 700 MB to 1.3 MB in the same release. **Lesson:** if Quarry ever adds CRDT collab (out-of-scope item B), Automerge is the right starting point — Rust core means no FFI pain. **What to design in now:** the `Document` content type should be `enum { Bytes(Vec<u8>), AutomergeDoc(Vec<u8>) }` so adding it later is non-breaking.

**Yjs** (MIT; JavaScript; active). Higher-performance CRDT with binary encoding and GC. Used by JupyterLab and many real-time editors. Rust ports exist (`yrs`/`y-crdt`). **Lesson:** Yjs is the JS-ecosystem standard; if a web-UI consumer wants CRDT collab, it'll probably want Yjs. **What to avoid:** Yjs and Automerge are not interoperable; pick one.

**ElectricSQL** (Apache-2.0; TypeScript + Elixir; active). Postgres logical-replication-based sync to local SQLite/PGlite. **Lesson:** server-authoritative replication is simpler than CRDT for most apps. **What to avoid:** Quarry is single-machine, so ElectricSQL's distributed Postgres story is overkill — but the "shape" subscription concept (subscribe to a query, get a continuously-synced subset) maps to Quarry's REST API as `GET /library/{id}/watch?prefix=...` later.

**Replicache / Reflect** (commercial; TypeScript). Mutator-based sync. **Lesson:** their "push/pull endpoint" protocol is small and clean; if Quarry needs HTTP sync between two laptops later, copy the protocol.

**Triplit** (MIT; TypeScript; active). Triple-store + sync engine. **Lesson:** triples are a clean way to model arbitrary metadata; if Quarry's metadata system gets complex, consider EAV (entity-attribute-value) tables. **What to avoid:** triples are over-engineering for Phase 1 — start with JSON blobs.

**Dexie Cloud** (commercial; TypeScript). IndexedDB sync. Less relevant for Rust/desktop.

**Jazz** (MIT; TypeScript; active). "CoValues" — typed CRDT objects with auth/perms baked in. **Lesson:** the "every entity has explicit permissions" model is interesting for multi-actor (human + AI). YAGNI for Phase 1, but reserve an `actor_id` column.

**Evolu** (MIT; TypeScript; active). SQLite + CRDT (vector-clock per row). **Lesson:** if Quarry ever needs multi-device sync, the "SQLite row + vector clock" pattern is well-trodden.

**Logoot / RGA approaches** (academic CRDT papers; various implementations). **Lesson:** sequence CRDTs (Logoot, RGA, Yjs's YATA, Automerge's RGA-Split) are the bedrock of collaborative text. Don't implement one yourself; pick Automerge or Yrs if/when needed.

### 1.4 Git-as-Database Systems

**Dolt** (Apache-2.0; Go; active; "Git for Data"). Stores rows in prolly trees (content-addressed B-trees), exposes Git CLI (`clone`, `branch`, `merge`, `commit`) over SQL tables. Dolt explicitly markets itself for agent memory: *"It's the best database for agent memory, especially as you move up the ladder to multi-agent and multi-machine workflows"* (project README). **Lesson:** prolly trees are the right structure for content-addressed structured data; the Noms design paper is foundational reading. **What to avoid:** Dolt's full Git-merge-conflict semantics over rows is enormously complex — Quarry's "store both copies" naivete is the right Phase-1 stance.

**TerminusDB** (Apache-2.0; Prolog/Rust; active). Graph (RDF triples) DB with branch/merge/diff. Per The New Stack's interview with Dolt's Tim Sehn, *"Dolt and TerminusDB are both versioned databases you can branch, merge, and diff. The main difference is that Dolt is a standard relational database and TerminusDB is a graph database."* **Lesson:** structural sharing across versions via Merkle DAGs is cheap and gives you free history. **What to avoid:** custom query language (WOQL) is adoption friction.

**irmin** (ISC; OCaml; active, maintained by Tarides). Git-shaped distributed database, used in MirageOS and Tezos. **Lesson:** the irmin model — "the working tree is just a view over content-addressed objects" — is the cleanest mental model for Quarry's relationship between TursoDB rows and the CAS.

**Pijul** (GPL-2.0; Rust; active but small community). Patch-based VCS using the theory of patches; conflicts are first-class. **Lesson:** patch commutation as a model of "concurrent edits that don't conflict" is mathematically clean. **What to avoid:** Pijul has no ecosystem; pin Git as Quarry's interop target.

**Jujutsu (jj)** (Apache-2.0; Rust; active, developed at Google as Piper successor). Git-compatible, but conflicts are first-class data, not error states. **Lesson:** "conflicts are stored data, not failures" is exactly Quarry's Phase-1 stance for Git sync. Read jj's docs on its conflict representation — and consider using jj for Quarry's own development.

**Fossil** (BSD-2-Clause; C; active, by SQLite author). DVCS storing artifacts in SQLite. Per Fossil's "Thoughts on the Design" document: *"The underlying database that Fossil implements has nothing to do with SQLite, or SQL, or even relational database theory. The underlying database is very simple: it is an unordered collection of 'artifacts' … The current implementation of Fossil uses SQLite as a local store … But the use of SQLite in this role is an implementation detail and is not fundamental to the design."* **Lesson:** *direct* prior art for "SQLite as the content-addressed store backing a VCS." Quarry should adopt the same stance — TursoDB is an implementation detail of the canonical data model.

**gitoxide (gix)** (Apache-2.0/MIT dual; Rust). Per Sebastian Thiel's official 2025 retrospective (GitoxideLabs/gitoxide Discussion #2323, "2025 - the Retrospective"): *"as of 2025-12-31, we are counting 211,983 SLOC … On GitHub there are 10695 stars (up by 1,416)"* and ~2,084 days of his solo work as of year-end. Current status: clone, fetch, blame, status, diff, commit-graph, worktree checkout, full read/write of objects/refs/index/config all work. **Missing as of late 2025: push (in progress), full merge workflows (commit-level), rebase, reset** (the retrospective explicitly says reset *"is definitely planned for 2026"*), commit hooks. The GitButler team's public stance: *"the next iteration will be the inverse with git2 only being used where gitoxide is lacking a feature."* **Recommendation:** start with `git2`, migrate hot read paths to gix incrementally.

### 1.5 FUSE Filesystems for Document/Object Stores

**gcsfuse** (Apache-2.0; Go; active, maintained by Google). Mounts a GCS bucket as a directory. **Lesson:** "directories are derived from object keys with `/` separators." Quarry should do the same. **What to avoid:** gcsfuse's POSIX compliance is weak (no cross-directory rename without copy, no hard links); this is fine.

**s3fs-fuse** (GPL-2.0; C++; active). S3 as FUSE. **Lesson:** "list-by-prefix to fake directories" is standard; for Quarry this is cheap because Turso/SQLite can do range queries on a `key` column.

**rclone mount** (MIT; Go; active). Universal cloud-storage FUSE. **Lesson:** rclone's `mount` handles dozens of backends via a unified VFS layer. **What to borrow:** rclone's *caching* strategy — write-through with a configurable RAM/disk cache — is exactly what Quarry needs for "FUSE-write becomes Turso INSERT" without blocking on every fsync.

**JuiceFS** (Apache-2.0; Go; active). FUSE filesystem with metadata in a DB (Redis/Postgres/SQLite/TiKV) and data in object storage. **This is the closest architectural sibling to Quarry.** Per the [JuiceFS metadata schema docs](https://juicefs.com/en/blog/engineering/design-metadata-data-storage), keys are formatted: `A{inode}I` (attr), `A{inode}D{name}` (dentry), `A{inode}C{block}` (chunks), `A{inode}X{name}` (xattr). Per the [JuiceFS 1.3 Beta release-notes blog](https://juicefs.com/en/blog/release-notes/juicefs-1-3-support-sql-database): *"With this optimization, JuiceFS 1.3 achieves over 10x improvement in single-directory concurrency compared to previous versions."* The mechanism: replace client-side directory-level locking with SQL's atomic `UPDATE ... SET NLINK = NLINK + 1 WHERE ...` round-trips. (Cross-network real-world bench gains were closer to 5×; 10× is the in-DB ceiling.) **Lesson:** use SQL atomic primitives, not pessimistic locks, for filesystem metadata operations. **What to borrow:** the entire JuiceFS metadata schema; it's been hardened over years.

**SeaweedFS-FUSE** (Apache-2.0; Go). Distributed object store with FUSE. **Lesson:** less relevant for single-laptop, but their chunked-file design (default 8 MB chunks) is the right approach to slicing large files into the CAS.

**git-fs / gitfs** (various, mostly abandoned). FUSE filesystems over a Git working tree. **Lesson:** these projects withered because real Git has too many edge cases (worktrees, submodules, LFS, sparse-checkout). **What to avoid:** don't try to expose Git history via FUSE — only the current working state.

**sshfs** (GPL-2.0; C; mostly maintained). FUSE over SFTP. **Lesson:** sshfs's per-operation latency makes it painful; Quarry's in-process design moots this.

### 1.6 Content-Addressed Storage Systems

**IPFS / Kubo** (MIT/Apache; Go; active). DHT-distributed CAS. **Lesson:** Quarry doesn't need DHT, but IPFS's `UnixFS` schema (representing a filesystem on top of content-addressed chunks with Merkle DAGs + rabin-fingerprint chunking) is well-tested.

**Perkeep (formerly Camlistore)** (Apache-2.0; Go; semi-active, Brad Fitzpatrick). GPG-signed claims on top of content-addressed blobs, with the **permanode** concept. Direct quote from Perkeep's spec: *"A permanode is simply a signed schema blob with no data inside that would be interesting to mutate … a permanent reference to a mutable object then is simply the blobref of the permanode."* **Lesson:** this is *the* reference design for "mutable document with content-addressed history" — Quarry should at least be permanode-shaped internally. **What to borrow:** the indexer-vs-blob-store separation. **What to avoid:** Perkeep's claims schema is heavyweight; Quarry can start with a SQL row pointing to a CAS hash.

**Git object store** (GPL-2.0; C). SHA-1 (transitioning to SHA-256) Merkle tree of `blob`/`tree`/`commit` objects. **Lesson:** the canonical CAS design; understand it cold. **What to borrow:** the loose-object-then-pack-file storage strategy.

**restic** (BSD-2-Clause; Go; active). Encrypted backup with content-defined chunking via rabin fingerprints. Per Onidel's 2025 benchmark: *"Restic implements content-defined chunking with variable-length segments, typically achieving 60-80% deduplication ratios on typical server workloads."* **Lesson:** for files that change incrementally, *content-defined chunking* gives massive dedup. The [scy gist comparison](https://gist.github.com/scy/de5176aef9209cb07e5f8c7b365cfbf1) shows restic handling inserted-bytes scenarios better than borg. **What to borrow:** CDC for Quarry's large-blob chunking in Phase 3+.

**borg** (BSD-3-Clause; Python; active). Similar to restic; per the same 2025 comparisons, borg's zstd compression typically saves an additional 30–50% beyond dedup.

**bup** (LGPL-2.1; Python; semi-active). Uses Git pack files with rolling-hash chunking. **Lesson:** if Quarry ever wants its CAS to *be* Git pack files (so Git can clone the CAS directly), bup proves it's feasible.

**Tahoe-LAFS** (GPL-2.0; Python; active). Distributed CAS with cryptographic capabilities. **Lesson:** the capability-URL model is elegant; over-engineered for single-user Quarry but worth knowing.

**S3 ETag-based dedup** is naive (whole-object hash, no chunking). **Lesson:** don't rely on file-level hashing for dedup; use chunk-level.

**`iroh-blobs`** (Apache-2.0/MIT; Rust; very active; n0-computer). Uses BLAKE3 verified streaming — BLAKE3's default 1 KiB chunks combined into chunk groups (the [iroh-blobs protocol docs](https://docs.rs/iroh-blobs/latest/iroh_blobs/protocol/) say *"It is possible to request entire blobs or ranges of blobs, where the minimum granularity is a chunk group of 16KiB or 16 blake3 chunks"*), with **bao**-encoded outboard trees enabling range requests and progressive verification. Their [blob store design challenges blog post](https://www.iroh.computer/blog/blob-store-design-challenges) is required reading. Storage layout: `<BLAKE3 HASH HEX>.data` + `<BLAKE3 HASH HEX>.obao4` (outboard, chunk-group-size 24, i.e., 16 KiB) + bitfield for partial blobs. The companion `blake3` crate's stable `HasherExt` hazmat API enables subtree hashing. **Recommendation:** Quarry should borrow `iroh-blobs`'s on-disk layout rather than depend on the full crate (which brings in QUIC and the iroh networking stack).

### 1.7 Document/Content Databases

**CouchDB** (Apache-2.0; Erlang; active). The original "JSON documents + MVCC + HTTP sync" database. **Lesson:** CouchDB's revision-tree model (every document has a hash-chained revision history) is what Quarry gets for free from CAS-keyed history.

**Couchbase Mobile** (Apache-2.0 client; commercial server; active). Couch sync protocol on mobile. **Lesson:** their `_changes` feed API is the right shape for "subscribe to changes since revision X." Reserve `GET /library/{id}/changes?since=...` in Quarry's API.

**RxDB** (Apache-2.0; TypeScript; active). Browser-side reactive DB with CouchDB-shape replication. **Lesson:** "queries-as-observables" is great for UIs; out of scope for Phase 1 but cheap to add via SSE later.

**PouchDB** (Apache-2.0; JavaScript; active). JS port of CouchDB. **Lesson:** if Quarry later exposes a Couch-compatible `/changes` and `_bulk_docs` endpoint, PouchDB clients work for free. YAGNI now; just don't preclude it.

**Fossil-SCM** — discussed in §1.4.

**Datasette** (Apache-2.0; Python; active, Simon Willison). SQLite + auto-generated read-only REST/JSON/CSV API. **Lesson:** *exact* model for "wrap a SQLite DB in a REST API." Quarry's REST API should look as clean as Datasette's URLs (`/library/{id}/docs/{key-path}`).

### 1.8 Static-Site / CMS Systems with Similar Shape

**Decap CMS (formerly Netlify CMS)** (MIT; React/JS; ~18K stars, semi-slow). Git-backed, YAML config, commits Markdown to a repo. **Lesson:** "edits are commits" is a clean UX; Quarry's auto-commit transactions can map 1:1 to Git commits.

**TinaCMS** (Apache-2.0; React/JS; active; SaaS-backed). Visual editor on top of Git-stored Markdown. **Lesson:** typed schemas defined in TS (`tina/config.ts`) with auto-generated GraphQL — schema as source code. Quarry should similarly let users *define* metadata schemas, eventually.

**Keystatic** (MIT; TypeScript; active, Thinkmill). Git-backed CMS with strong TS-first schemas, generates Astro/Next integrations. **Lesson:** clean Astro/Next integration is a force multiplier; ship a `quarry-js` client SDK eventually.

**Sanity / Contentful** (commercial SaaS). Database-backed headless CMS. **Lesson:** Sanity's GROQ query language is interesting prior art for "query documents by metadata"; reserve `GET /library/{id}/query?groq=...` as a future possibility but don't build it now.

---

## 2. Key Technical Resources

### 2.1 TursoDB / libSQL in async Rust

**Current state (mid-2026).** Latest stable: **`turso 0.6.1`** on crates.io (with 0.6.0-pre.* previews; ~286K total downloads at time of writing). License: MIT. Owner: Pekka Enberg (penberg). The README still carries a **BETA warning** at the top: *"This software is in BETA. It may still contain bugs and unexpected behavior."* The Turso team continues to offer bounties (up to $1,000) for data-corruption bugs.

**Critical limitation #1 — empty `Transaction` type.** In `turso 0.6.1`, the public `Transaction` struct in `src/lib.rs` is *literally an empty stub*: `pub struct Transaction {}`. There is no `conn.transaction()` RAII guard like `rusqlite`/`libsql` provide. Explicit transactions must be driven by raw SQL (`conn.execute("BEGIN")` / `COMMIT` / `ROLLBACK`). **This is a major issue for Quarry's transaction-centric design.**

**Critical limitation #2 — one active transaction per connection.** From `docs/manual.md`: *"Each connection can have exactly one active transaction at a time … When you need concurrency (including `BEGIN CONCURRENT`), you need to use different connections, not parallel statements within the same connection."*

**Critical limitation #3 — no incremental BLOB I/O.** SQLite's `sqlite3_blob_open` / `_read` / `_write` API is not exposed. Blobs must be passed whole as `Value::Blob(Vec<u8>)`. **Storing large files inline is a non-starter; CAS off-loading is mandatory for any file >~few MB.**

**Critical limitation #4 — "no multi-process access"** is listed as an explicit limitation. An `experimental_multiprocess_wal` builder flag exists in 0.6 (`.tshm` sidecar) but is experimental. **Implication:** Quarry must be a single-process daemon; FUSE mount and REST API run in the same process.

**Other documented limitations** (from `docs/manual.md`): no savepoints in WAL mode (MVCC mode has them), no views (experimental flag only), no vacuum (experimental flag), UTF-8 only.

**MVCC / BEGIN CONCURRENT status.** From the official [Turso v0.5.0 blog post (turso.tech/blog/turso-0.5.0)](https://turso.tech/blog/turso-0.5.0): *"In 0.5, concurrent writes move from tech preview to beta. You enable MVCC with `PRAGMA journal_mode = 'mvcc'`, and the feature is now available for limited production use."* But MVCC has serious caveats per `docs/manual.md`: *"Indexes cannot be created and databases with indexes cannot be used,"* data is eagerly loaded into RAM, no AUTOINCREMENT, and outstanding correctness bugs (#4877 "mvcc doesn't find valid version after checkpoint+restart followed with update," #5688 AUTOINCREMENT contention). **Don't use MVCC mode for Quarry yet.**

**Async / concurrency pattern.** The crate's own integration test (`test_parallel_writes_and_wal_size`) demonstrates the recommended pattern: clone the `Database` (cheap, internally `Arc<TursoDatabase>`) into each tokio task, call `db.connect()` per task, retry on `Error::Busy(_)` with a small backoff:

```rust
let db = Builder::new_local(path).build().await?;
for _ in 0..N {
    let db = db.clone();
    tokio::spawn(async move {
        let conn = db.connect().unwrap();
        loop {
            match conn.execute("INSERT ...", params).await {
                Ok(_) => break,
                Err(Error::Busy(_)) => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    continue;
                }
                Err(e) => panic!("{e:?}"),
            }
        }
    });
}
```

`Database` is `Clone`; `Connection` is *not* `Clone` but is intended one-per-task.

**Vendor recommendation.** Per [docs.turso.tech/sdk/rust/quickstart](https://docs.turso.tech/sdk/rust/quickstart): *"`turso` is the recommended crate for running a local database, including synchronizing it to and from Turso Cloud."* But the README FAQ states bluntly: *"At this point, libSQL is production ready, Turso Database is not — although it is evolving rapidly."* (Newer README phrasing softens this to *"Turso powers production apps today … However, it is still under active development and for mission-critical applications, caution is advised."*)

**Quarry recommendation.** Use **`rusqlite`** (mature, sync API + `spawn_blocking`) or **`libsql`** (async-native, Turso's "production-ready" choice) for Phase 1, behind a `Storage` trait. Migrate to `turso` once: (a) it ships a real `Transaction` API, (b) it exposes incremental BLOB I/O, and (c) the BETA flag is removed.

Links:
- Repo: https://github.com/tursodatabase/turso
- crate: https://crates.io/crates/turso
- docs: https://docs.turso.tech/sdk/rust/reference
- libsql crate: https://crates.io/crates/libsql

### 2.2 FUSE in Rust

| Crate | License | Async story | Platform | Status |
|---|---|---|---|---|
| `fuser` | MIT | Sync trait; pair with `fuser-async` | Linux + macOS (via FUSE-T or macFUSE) + FreeBSD | Active, mature; de-facto Rust FUSE library |
| `fuse3` | MIT | Native async (tokio or async-std) | **Linux only** (FUSE ABI ≥ 7.23) | Active (v0.9.0) |
| `polyfuse` | MIT | Native async (tokio) | **Linux only** | Less active; v0.1.x; macOS/FreeBSD listed as "future work" |

**Quarry recommendation: `fuser` + `fuser-async`.** Reasoning:
- **macOS support is critical** for a laptop tool; `fuse3` and `polyfuse` are Linux-only.
- `fuser-async` (~189K downloads) wraps `fuser` with async traits, including an S3-backed example demonstrating the exact pattern Quarry needs (FUSE syscalls dispatching to async tokio code).
- `fuser` is well-maintained and is what most production Rust FUSE projects use.

**macOS gotcha.** macOS dropped kernel FUSE support; users need **macFUSE** (kext, paid past v4) or the newer **FUSE-T** (NFS-based, no kext, free). Document FUSE-T as the recommended option and test against it.

**Reference architectures:**
- `fuser-async`'s S3 example (crates.io/crates/fuser-async).
- JuiceFS's FUSE layer (Go, but the schema is the contribution — read `pkg/meta/sql.go`).

### 2.3 Git libraries in Rust

**`git2`** (Apache-2.0/MIT; bindings to libgit2). Mature, supports everything (push, full merge, rebase, reset, hooks). **Use for writes.**

**`gitoxide` (`gix`)** — see §1.4 status quote. Read-side mature; push/merge/rebase/reset still in progress.

**Quarry recommendation for Phase 1:**
- Use `git2` for all Git operations initially.
- Add `gix` opportunistically for hot read paths (status, diff, log) in Phase 4+ once basic two-way sync is working.
- Wrap behind a `GitSync` trait so the switch is mechanical.

**Two-way sync design (critical).** Neither library does bidirectional sync — they're libraries, not sync engines. You build the reconciliation. Per rclone bisync (§2.6): three-way diff against a "last-known-synced" snapshot, store-both-copies on conflict, persistent state file.

### 2.4 REST API frameworks in async Rust

**Recommendation: `axum` 0.8+** (MIT; from the tokio org). Unambiguous community default in 2025–2026.

- `actix-web` mature but its actor model is unusual.
- `rocket` has slipped on async ergonomics.

**OpenAPI generation:** use **`utoipa`** + **`utoipa-axum`** (MIT/Apache; very active). `utoipa-axum`'s `OpenApiRouter` fuses routing and spec generation so they can't drift. Pair with `utoipa-swagger-ui` or `utoipa-scalar` for serving docs. The official utoipa todo-axum example is the canonical project layout to copy.

`aide` is the other axum-OpenAPI option but `utoipa-axum` has more momentum.

**Recommended layout:** see §3 workspace.

### 2.5 Content-addressed storage in Rust

**Hash:** `blake3` (CC0/Apache-2.0; >3 GB/s on modern CPUs; tree-structured so streaming verification is free). The `HasherExt` hazmat API enables subtree hashing — see Iroh's "[The new BLAKE3 hazmat API](https://www.iroh.computer/blog/blake3-hazmat-api)" post.

**Store crate options:**
1. **`iroh-blobs`** (Apache-2.0/MIT; n0-computer; very active). Solid local `FsStore`, but pulls in QUIC and the iroh networking stack. Their [blob store design challenges](https://www.iroh.computer/blog/blob-store-design-challenges) is mandatory reading — they document every wart (partial writes, validity bitfields, "files are hard").
2. **`dennwc/cas`** (Go; reference CAS spec).
3. **Roll your own.** For Quarry Phase 1, this is the right call. ~200 lines:

```rust
pub struct CasStore { root: PathBuf }
impl CasStore {
    pub async fn put(&self, bytes: &[u8]) -> io::Result<Hash> {
        let hash = blake3::hash(bytes);
        // atomic write: tempfile + fsync + rename to cas/objects/ab/cdef...
    }
    pub async fn get(&self, hash: &Hash) -> io::Result<Vec<u8>> { ... }
    pub async fn exists(&self, hash: &Hash) -> bool { ... }
    pub async fn delete(&self, hash: &Hash) -> io::Result<()> { ... }
}
```

On-disk layout (steal from Git): `cas/objects/ab/cdef0123...`.

**Garbage collection.** Perkeep/Git approach: walk the database for every `cas_hash` reference, mark each, delete anything in `cas/objects/` not marked. Phase 1: `POST /admin/gc`, run only when no transactions in flight. Phase 3+: incremental ref-counting via triggers or app-level wrappers.

**Inline vs CAS threshold.** Hybrid: documents ≤ **64 KiB** stored inline in the `documents.content` BLOB column; >64 KiB go to CAS with only the BLAKE3 hash in the row. Configurable per Library.

### 2.6 Watcher/sync mechanics

**rclone bisync** is the most directly applicable prior art. Per the [rclone bisync docs](https://rclone.org/bisync/), the algorithm:

1. **Persistent state.** On each successful sync, rclone writes a "prior listing" of both sides to `~/.cache/rclone/bisync/`. The next run computes deltas against this prior listing — a **three-way diff**.
2. **Conflict-loser suffix.** Per the rclone bisync CLI docs: `--conflict-suffix string` *"Suffix to use when renaming a --conflict-loser. Can be either one string or two comma-separated strings to assign different suffixes to Path1/Path2. (default: 'conflict')"*. Default conflict loser action is `num` (rename with a numbered suffix); other options are `pathname` and `delete`. **This is exactly Quarry's "store both copies" rule.** Use suffixes like `.conflict-quarry-<timestamp>` and `.conflict-git-<timestamp>`.
3. **Safety limits.** `--max-delete 50%` — aborts if more than 50% of files would be deleted. Adopt this.
4. **`--check-access` sentinel.** rclone places a `RCLONE_TEST` file on both sides; if missing, abort. Quarry should write a `.quarry/marker.json` containing the Library ID.

**Syncthing's** approach is similar but for many-to-many; relevant idea is **version vectors per file**.

**Unison** (OCaml; the classic two-way file-sync tool) pioneered three-way diff sync; its "Reconciliation" section is foundational reading.

### 2.7 Virtual filesystem over a metadata DB

**JuiceFS is the reference.** SQL schema in `pkg/meta/sql.go`:
- `node` (inode, type, mode, uid/gid, atime/mtime/ctime, length, nlink, parent)
- `edge` (parent_inode, name, child_inode, type) — directory entries
- `chunk` (inode, indx, slices) — file-data → object-store mapping
- `xattr` (inode, name, value)

Per their JuiceFS 1.3 release blog: *"With this optimization, JuiceFS 1.3 achieves over 10x improvement in single-directory concurrency compared to previous versions"* — by replacing client-side directory locking with SQL atomic updates like `UPDATE node SET nlink = nlink + 1 WHERE inode = ?`. **Lesson:** use SQL's atomic primitives, not client-side locks, for filesystem metadata operations.

**Quarry's adaptation.** Since the canonical store is *keys* (S3-style `/`-separated) rather than inodes:

**Option A (recommended for Phase 1) — derive directories from key prefixes (S3-style).** No directory records. `ls /foo/` becomes a prefix query. Pros: simple, no metadata to keep consistent. Cons: empty directories don't exist; some POSIX directory operations have nowhere to store the result.

**Option B — real directory rows.** Adds a `type` column (file/dir) and parent-child relations. JuiceFS-style.

**Recommendation: Option A + a small `dir_metadata` table** keyed by prefix path that holds *only* the metadata POSIX requires for `getattr` on a directory (mode, mtime). Empty directories get a row in `dir_metadata` only. Forward-compatible with Option B.

**Inode allocation.** FUSE needs stable u64 inodes. Two patterns:
- **Hash-based:** `inode = hash(key) >> 1 | 1` — stateless but tiny collision risk.
- **Allocated:** an `inodes(inode INTEGER PRIMARY KEY AUTOINCREMENT, key TEXT UNIQUE)` table, never reused. **Recommended.**

### 2.8 MCP server resources

**Spec:** https://modelcontextprotocol.io. Wire format is JSON-RPC 2.0 over stdio, SSE, or Streamable HTTP. Protocol versions 2024-11-05, 2025-03-26, and 2025-06-18 are current.

**Rust SDK: `rmcp`** (MIT; official SDK at https://github.com/modelcontextprotocol/rust-sdk). Per the project's GitHub releases page, **`rmcp-v0.12.0` was the latest tag on December 18, 2025**; the live README references higher versions (0.16.0 series), reflecting the rapid early-2026 release cadence. Features:
- `#[tool]` and `#[tool_router]` proc macros for ergonomic tool definitions with JSON-schema generation via `schemars`.
- Transports: stdio (Tokio child process), Streamable HTTP, SSE.
- Server and client roles.

Example MCP servers exposing document/memory stores (prior art):
- `rustfs-mcp` — S3-compatible storage as MCP.
- Logseq MCP servers (`joelhooks/logseq-mcp-tools`, `ergut/mcp-logseq`).
- Supermemory's MCP server (proprietary).
- The tursodb CLI ships with an `--mcp` flag exposing nine tools (`schema_change`, `list_tables`, `insert_data`, `execute_query`, etc.).

**Quarry MCP server (Phase 2+):** natural tool surface:
- `quarry_read(library, key)` → document content + metadata
- `quarry_write(library, key, content, metadata)`
- `quarry_list(library, prefix)`
- `quarry_search(library, query)` — placeholder for eventual FTS
- `quarry_begin_transaction(library)`, `quarry_commit_transaction(tx_id)`

Design the REST API as the canonical surface; the MCP server is a thin shim over HTTP.

---

## 3. Phased Build Plan

### Workspace layout (Cargo workspace, ~9 crates)

```
quarry/
├── Cargo.toml                       # workspace manifest
├── crates/
│   ├── quarry-core/                 # domain types, no I/O
│   │   └── src/
│   │       ├── library.rs           # Library, LibraryId
│   │       ├── document.rs          # Document, Key, Metadata
│   │       ├── transaction.rs       # Transaction, Draft
│   │       └── error.rs
│   ├── quarry-storage/              # Storage trait + impls
│   │   └── src/
│   │       ├── lib.rs               # Storage trait
│   │       ├── rusqlite_impl.rs     # Phase 1 default
│   │       ├── libsql_impl.rs       # optional Phase 1
│   │       └── turso_impl.rs        # Phase 5+ once stable
│   ├── quarry-cas/                  # content-addressed storage
│   │   └── src/{store.rs, hash.rs, gc.rs}
│   ├── quarry-git/                  # GitSync trait + git2 impl
│   │   └── src/{import.rs, export.rs, bisync.rs, conflict.rs}
│   ├── quarry-fuse/                 # FUSE filesystem (fuser-async)
│   │   └── src/{fs.rs, inode.rs, cache.rs}
│   ├── quarry-server/               # axum REST API + utoipa OpenAPI
│   │   └── src/{app.rs, api/, state.rs}
│   ├── quarry-mcp/                  # FUTURE: rmcp shim (skeleton in Phase 6)
│   ├── quarry-cli/                  # FUTURE: clap-based CLI client
│   └── quarry/                      # binary crate: bootstraps everything
│       └── src/main.rs
└── examples/
    └── basic.rs
```

### Phase 0 — Spikes (week 1, 3–5 days)

**Goal: de-risk the unknowns before committing to architecture.** Throwaway code OK.

1. **Turso interactive transaction spike.** Open `turso 0.6.x`, issue raw `BEGIN`/`INSERT`/`COMMIT` against multiple connections concurrently, confirm ACID semantics under `Error::Busy` retry. **Decision criterion:** if Turso can't reliably handle 4 concurrent writers with retry over a 1-hour soak, fall back to `rusqlite`.
2. **FUSE-on-macOS spike.** Mount a trivial `fuser-async` filesystem on macOS via FUSE-T; confirm read, write, mkdir, rename, getattr. Test with `ls`, `cat`, `vim`, `ripgrep`. **Decision criterion:** if FUSE-T has fatal regressions on current macOS, document FUSE-T as a prerequisite and consider WebDAV mount as a fallback.
3. **git2 bidirectional spike.** Make two clones of an empty repo, edit both, write a Rust program that does the reconciliation. **Decision criterion:** confirm correctness on the 4-quadrant test matrix (added/added, modified/modified, modified/deleted, deleted/deleted).
4. **CAS write throughput spike.** With BLAKE3 + tokio + iroh-blobs-style layout, measure single-thread throughput. Should be >500 MB/s easily.

### Phase 1 — Storage layer (week 2–3)

Ship `quarry-core`, `quarry-storage`, `quarry-cas`. No API, no Git, no FUSE.

- Define `Storage` trait (CRUD, list-by-prefix, transactions).
- Implement against `rusqlite` (sync API wrapped in `spawn_blocking` where helpful). Production impl for Phase 1.
- Implement `CasStore` with put/get/exists/delete on disk.
- Implement hybrid inline-vs-CAS with configurable threshold (default 64 KiB).
- Schema (v1):
  ```sql
  CREATE TABLE libraries (id TEXT PRIMARY KEY, name TEXT, created_at INTEGER, last_synced_oid TEXT);
  CREATE TABLE documents (
    library_id TEXT NOT NULL REFERENCES libraries(id),
    key TEXT NOT NULL,
    content_inline BLOB,
    content_hash TEXT,
    size INTEGER NOT NULL,
    metadata TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    PRIMARY KEY (library_id, key)
  );
  CREATE INDEX idx_docs_prefix ON documents(library_id, key);
  ```

**Exit criterion:** integration test creates a Library, writes 1000 docs (mix of small + large), reads/lists/deletes; explicit GC cleans CAS files correctly.

### Phase 2 — Transactions + REST API (week 4–5)

Ship `quarry-server` (axum + utoipa).

- Transaction layer: `Transaction` wraps a SQL transaction + a list of staged document writes. Auto-commit ≡ begin+commit in one call; explicit transactions return `tx_id` for follow-up writes.
- REST endpoints (OpenAPI-generated):
  - `POST /libraries`
  - `GET /libraries/{id}/docs/*key` (read; HEAD for metadata)
  - `PUT /libraries/{id}/docs/*key` (auto-commit)
  - `DELETE /libraries/{id}/docs/*key`
  - `GET /libraries/{id}/docs?prefix=...`
  - `POST /libraries/{id}/tx` → `{tx_id}`
  - `PUT /libraries/{id}/tx/{tx_id}/docs/*key`
  - `POST /libraries/{id}/tx/{tx_id}/commit` / `/abort`
- Auth: skip for Phase 1; bind to `127.0.0.1` only.
- Concurrency: one shared `Arc<AppState>`; per-request connections; storage `Busy` → HTTP 503 with `Retry-After`.

**Exit criterion:** `curl` round-trip works; OpenAPI spec at `/api-docs/openapi.json` validates with `openapi-cli`.

### Phase 3 — Git import/export (one-way) (week 6)

Ship `quarry-git`'s one-way primitives.

- `POST /libraries/{id}/git/export` — materialize Library as files into a Git working dir, commit, optionally push. Document at key `foo/bar.md` → file `<repo>/foo/bar.md`. Metadata as YAML frontmatter (configurable) for Markdown, sidecar `foo/bar.md.quarrymeta.yaml` for binary.
- `POST /libraries/{id}/git/import` — read working dir, ingest.
- Record `last_synced_oid` after each success.

**Exit criterion:** import-export round-trip is byte-lossless.

### Phase 4 — Bidirectional Git sync (week 7–8) — **highest-risk phase**

- `POST /libraries/{id}/git/sync` — the two-way operation.
- Algorithm (rclone-bisync pattern):
  1. `quarry_snapshot` := walk current Library documents.
  2. `git_snapshot` := walk working tree (after `git fetch && git merge --ff-only origin/main` if a remote configured; non-FF cases captured separately).
  3. `last_snapshot` := materialize `last_synced_oid`'s Git tree.
  4. Three-way diff: per key, classify into 8 quadrants (BothUnchanged, OnlyQuarryChanged, OnlyGitChanged, BothChanged, BothDeleted, QuarryDeleted/GitChanged, QuarryChanged/GitDeleted, BothCreated).
  5. Apply: trivially propagate one-sided changes; for `BothChanged` with different content, write *both* — Quarry-side keeps original key; Git-side becomes `<key>.conflict-git-<iso8601>`; in inverse direction, write `<key>.conflict-quarry-<iso8601>` to Git. Mark a `conflicts` row in DB.
  6. Commit Git state: `quarry: sync at <timestamp>`.
  7. Update `last_synced_oid`.
- Safety: write `.quarry/marker.json` with library ID on first export; refuse sync if missing/mismatched. Implement `--max-changes` percentage abort like rclone's `--max-delete`.

**Exit criterion:** 4-quadrant test matrix passes; manual end-to-end test ("edit on laptop, edit via Working Copy on phone, sync both") produces correct conflict files.

### Phase 5 — FUSE filesystem (week 9–10)

Ship `quarry-fuse` (read-only first, then read-write).

- Inode allocation table in Storage (explicit `inodes` table — JuiceFS style).
- Read ops: `getattr`, `readdir`, `lookup`, `open`, `read`. Mount via `fuser::spawn_mount2`.
- POSIX↔transaction mapping (see §4.4): every FUSE write becomes a tiny auto-commit transaction. There is no way to batch writes into one transaction across syscalls without an out-of-band signal — *don't try.* Optionally expose a `.quarry/begin-tx` magic file pattern as a Phase 6+ stretch goal.
- Write ops: `write`, `create`, `mkdir`, `rename`, `unlink`, `rmdir`, `setattr` (mode/mtime).
- Write coalescing: small writes to the same file within ~500ms get coalesced in a write-back cache before hitting the DB. **Essential** — without it, `vim` saves (many small writes) overwhelm storage.

**Exit criterion:** `ripgrep`, `find`, `vim`, `cp -r`, `git status` all work against a mounted Library on Linux and macOS.

### Phase 6 — Packaging + polish (week 11–12)

- `quarry` binary: `quarry init`, `quarry serve --library ~/quarry`, `quarry mount /mnt/quarry`.
- Single static binary; Homebrew formula; `.deb` package; GitHub releases.
- MCP server skeleton (`quarry-mcp`) — exposes `quarry_read`/`quarry_write`/`quarry_list` as MCP tools, shim over REST API.
- CLI client (`quarry-cli`) skeleton — `quarry get`/`put`/etc.
- Backup/restore CLI.
- Docs site.

### Specific Risks / Unknowns to Spike Early

1. **Turso async transaction semantics.** Spike in Phase 0. Mitigation: fall back to `rusqlite`.
2. **FUSE on macOS.** Test under FUSE-T explicitly. Mitigation: document FUSE-T as prerequisite; consider WebDAV mount fallback.
3. **Git two-way sync edge cases.** Specifically: non-fast-forward fetches, reserved Windows/macOS filenames (`con`, `aux`), case-folding clashes on case-insensitive filesystems (`Foo.md` vs `foo.md`), BOM/line-ending normalization. The 8-quadrant test matrix must cover these.
4. **FUSE↔transaction interaction.** POSIX has no commit. Don't expose explicit transactions through FUSE. Defer the magic-file pattern.

### Forward-compat hooks for out-of-scope features

- **Web UI (A):** everything goes through REST API. Add `text/html` content negotiation later. Reserve `/ui/*` paths.
- **CRDT collaborative editor (B):** add `content_type` field on documents now (default `binary` / `text/plain` / `text/markdown`); reserve `application/automerge`. Phase 5 FUSE treats unknown types as opaque bytes.
- **Full-text search (C):** Turso has experimental Tantivy-based FTS — don't enable. Stub `POST /libraries/{id}/search` → 501. Slot FTS in behind the same endpoint later.

---

## 4. Design Questions / Open Issues to Flag

### 4.1 Document content storage: inline vs CAS vs hybrid

**Recommendation: hybrid with a configurable size threshold (default 64 KiB).**
- Inline ≤ 64 KiB.
- CAS > 64 KiB; row stores BLAKE3 hash + size only.
- `(content_inline BLOB NULL, content_hash TEXT NULL)` — exactly one non-null.

**Open question:** rewrite inline → CAS when a doc grows? Yes; on each write decide by current size. Old CAS object becomes unreferenced and is GC'd. Don't move lazily.

### 4.2 Metadata schema: typed columns vs JSON blob vs key-value table

**Recommendation: JSON blob (Turso/SQLite's `json1` functions) for Phase 1, with two extracted "well-known" columns: `content_type` and `created_at`.**
- JSON is flexible; query with `json_extract(metadata, '$.tags[0]')`.
- Two indexed columns for common queries.
- Future migration to typed columns or EAV is straightforward.

Avoid EAV in Phase 1 — premature for single-user.

### 4.3 Directory semantics

**Recommendation: prefix-derived (S3-style) with a small `dir_metadata` sidecar table for POSIX directory attributes only (mode, mtime), populated lazily by FUSE getattr.**
- Keyspace stays S3-like.
- POSIX-required directory metadata exists where needed.
- Empty directories: `dir_metadata` row with no matching documents.
- Directory rename: `UPDATE documents SET key = REPLACE(key, 'old/', 'new/') WHERE key LIKE 'old/%'` in a transaction.

### 4.4 Transactions vs FUSE mount

**Resolution: FUSE writes are always auto-commit.** No filesystem-transaction abstraction. Multi-file atomicity via REST API. Stretch goal: `.quarry/begin-tx` magic-file pattern in Phase 6+.

**Implication:** FUSE inode cache must invalidate when REST API writes happen, and vice versa. Use a single in-process `tokio::sync::broadcast`; cache and open-file readers subscribe.

### 4.5 Transactions vs Git sync

**Resolution: one Git commit per Quarry commit (REST transaction commit), batched if N pending writes have accumulated within a debounce window.**
- Explicit transactions: one Git commit with transaction ID in message.
- Auto-commit transactions: one commit each (debounce ~5s).
- The Git sync operation acquires a DB-level write lock that blocks new transactions for the sync duration. Naive but correct; optimize later.

**Open question:** opportunistic (on every commit) or scheduled (manual/cron)? **Recommendation: scheduled in Phase 4** (manual `POST /sync` + optional `--sync-interval`). Reduces failure-mode complexity.

### 4.6 Conflict markers format

**Recommendation: rclone-style suffix on the key, with metadata pointer to the "winner."**
- For key `foo/bar.md` (Quarry version A, Git version B):
  - Quarry: `foo/bar.md` keeps A; new document at `foo/bar.md.conflict-git-2026-05-27T14:30:00Z` holds B. Both rows have `metadata.conflict_with: "foo/bar.md"`.
  - Git: same — synced commit contains both `foo/bar.md` (A) and `foo/bar.md.conflict-git-...` (B).
- For Markdown, support optional inline `<<<<<<< ======= >>>>>>>` markers (per-library config: `conflict_strategy = "side-by-side" | "inline" | "both"`).

**What rclone does that Quarry should copy:** preserve conflict files until explicitly resolved; deleting a conflict file = "this is resolved, drop the metadata pointer."

### 4.7 Concurrency model

**Resolution: single-process daemon. One `Arc<QuarryCore>` shared across:**
- The axum REST server (per-task DB connection from a small pool).
- The FUSE handler (tokio task per syscall via `fuser-async`).
- Background Git sync workers.

**Pattern:** `QuarryCore` owns a `Storage` impl with internal connection pool (`deadpool`/`bb8` style). Each op borrows a connection. A single `tokio::sync::RwLock<()>` guards global operations (Git sync, GC, schema migrations) — write-mode during, read-mode (cheap) during normal ops.

**Multi-process: explicitly disallowed in Phase 1.** Don't try to share a Library directory between two `quarry` daemons. (Turso doesn't support multi-process; SQLite locking over FUSE on macOS is broken; complexity isn't worth it.)

### 4.8 Other open questions

- **Schema migrations.** `refinery` crate for `rusqlite`, or roll your own with `PRAGMA user_version` if going Turso-direct.
- **Backup.** SQLite's online backup API + `tar` of `cas/`. Document.
- **BLAKE3 chunk size?** 1 MiB for content-defined chunking; defer CDC to Phase 3+. Phase 1: whole-file BLAKE3.
- **Binary documents in Git?** Opt-in `git-lfs` integration in Phase 4 for files >5 MiB. Out of scope Phase 1.
- **Case sensitivity.** Keys case-sensitive in storage; document that FUSE on case-insensitive FS (default macOS) will collapse case-conflicting keys — log warning at mount.
- **Symlinks in FUSE?** Phase 1 returns ENOTSUP. Add if needed.
- **`.quarry/` directory in mounted FS?** Reserve `.quarry/` prefix for internal sentinels (markers, future magic files). Refuse client writes to it.

---

## Recommendations (concrete next actions)

1. **This week:** Execute the four Phase 0 spikes. **Threshold to proceed with `turso`:** the transaction spike passes a 1-hour concurrent-writer soak with no panics and no deadlocks. Otherwise ship Phase 1 on `rusqlite` behind the `Storage` trait. **Threshold to proceed with `fuser` on macOS:** all standard editors and `ripgrep` work against a trivial mount under FUSE-T. Otherwise document FUSE-T installation as a prerequisite and add WebDAV fallback to the roadmap.
2. **Week 2:** Lock the schema (§4.1–§4.3) and ship Phase 1 storage layer.
3. **Week 4 decision point:** if the REST API gels quickly and OpenAPI generation is clean (utoipa-axum), continue. If utoipa-axum proves limiting, fall back to handwritten OpenAPI YAML — a few hundred lines.
4. **Week 7 decision point (the biggest one):** before writing bidirectional Git sync, write the *test matrix* first — the 8-quadrant table from §3 Phase 4. If you can articulate the expected behavior for each cell, the implementation is mechanical. If you can't, stop and rethink the model.
5. **Stretch goal benchmark for "successful Phase 1":** a real human + a Claude Code agent can both edit notes in the same Library via FUSE for one workweek without losing data.

---

## Caveats

- **Turso is moving fast.** Anything claimed about `turso 0.6.1` may be obsolete in 3–6 months. Re-evaluate before committing. The crate genuinely is the long-term right answer for async-first embedded SQLite; just not in May 2026 for production.
- **macOS FUSE is a moving target.** Apple has been hostile to kernel extensions for years; FUSE-T may or may not exist in its current form in 2027. The architecture should make swapping the FUSE crate trivial.
- **gitoxide push support** has been "soon" for years per Sebastian Thiel's annual retrospectives (the 2025 retrospective explicitly defers reset to 2026). Don't plan around it.
- **No public end-to-end production reference for "TursoDB + FUSE in one process"** exists that I could find. This is novel territory; expect surprises. The closest analog is JuiceFS (DB + FUSE), but they're separate processes (FUSE client talks to a remote metadata service).
- **MCP protocol versions** churn quarterly. The Rust SDK `rmcp` has had ten-plus minor releases in six months (per the rust-sdk releases page). Treat the MCP server as a thin shim, not a deep integration.
- **Single-user, single-machine** is the design center. As soon as you contemplate multi-device sync, the conflict model becomes a CRDT or Dolt-style merge problem, and Phase 1's "store both copies" naivete is insufficient. That's a v2 conversation.