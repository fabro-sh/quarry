# Quarry as a collaborative storage substrate for agents and memory

## Bottom line

Quarry should be designed as a **canonical local-first storage core** with **immutable content objects**, **mutable refs**, and **projection layers** for Git, filesystem mounts, and APIs. The cleanest architecture is not “Git with extra features.” It is closer to **Git’s object model for durable history**, **Perkeep/IPFS-style content addressing for blobs**, and **Proof-style agent-native collaboration APIs**. Git’s native model is immutable objects plus refs, index, and reflogs, and it computes diffs from snapshots rather than storing collaborative intent. That makes Git excellent for export, reproducibility, and external interoperability, but a poor primary source of truth for presence, threaded comments, temporary redlines, and fine-grained collaborative editing. citeturn22view0turn22view2turn32view0turn32view1turn4view0

If you want to ship this with **PlateJS**, the pragmatic first implementation is **Yjs for live collaboration** and **Git as a bidirectional mirror**. Plate already supports Yjs collaboration, comments, and suggestions. Yjs provides shared types, awareness/presence, and subdocuments that explicitly fit folder-shaped document sets. If the backend or mount layer is implemented in Rust, **Yrs** gives you a compatible Rust port of the Yjs CRDT. citeturn4view2turn4view3turn4view4turn25search0turn25search3turn28view0

The most important operational constraint is this: **anything you expect agents to search with plain `rg` must exist as visible text somewhere**. `ripgrep` skips ignored files, hidden files, and binary files by default. That means a gitignored scratch directory and raw PDFs will often disappear from naïve agent search unless you mount them differently or force `rg -uuu`. This should shape the design of ephemeral workspaces and binary handling from the start. citeturn23view2turn23view3

## Recommended architecture

The core of Quarry should treat **paths as views, not identity**. Stable identity should come from content IDs and refs. Git’s data model stores immutable objects identified by hashes, and IPFS and Perkeep use the same basic idea of content-addressed storage. That pattern is a strong fit for Quarry because it makes renames, projections, and multi-surface synchronization much easier to reason about than path-first storage. citeturn22view0turn22view2turn32view0turn32view1

A concrete model would have four first-class record types. **Blobs** hold immutable bytes such as source files, PDFs, images, or extracted text. **Structured docs** hold live collaborative state for rich documents and review artifacts. **Annotations** hold comments, suggestions, critics, and provenance attached to a target object or ref. **Refs** represent mutable named pointers such as `published/main`, `draft/<doc>`, `ephemeral/<user>`, or `git/<branch>`. This mirrors the object/ref split in Git while still giving you room for richer semantics than Git can natively express. citeturn22view0turn30view2

The annotation layer should not be a hack inside the source file. It should be a first-class overlay. The W3C Web Annotation model is specifically designed for attaching bodies to particular segments of arbitrary resources, and it supports selectors for text, ranges, fragments, and SVG regions. That makes it a solid canonical format for comments on plain text, rich text, images, and PDFs. citeturn14view2turn14view3turn14view0turn14view1

For agent workflows, every important binary should also produce **searchable textual derivatives** and metadata. In practice, Quarry should materialize things like `report.pdf`, extracted text, page maps, and maybe a structured outline or chunk index. That is not mere convenience. It is necessary because default `ripgrep` will skip the binary itself. citeturn23view2

One useful design principle is to expose three official projections from the same core state. The **Git projection** is for interoperability and external branching history. The **filesystem projection** is for agents and legacy tools that want POSIX-like access. The **API projection** is for clients that want document, annotation, presence, and event semantics directly instead of pretending everything is a file. Proof’s public SDK routes are a good example of separating state, snapshot, edit, ops, presence, and bridge endpoints rather than exposing raw storage internals. citeturn4view0

## Collaboration engine and editor stack

**Yjs is the best v1 choice if Plate is the editor.** It is network-agnostic, shared-type based, and its document updates are compressed, commutative, associative, and idempotent, which makes them easy to store, retransmit, and replay. Yjs subdocuments are especially relevant because they explicitly support large folder-like hierarchies of documents with lazy loading. Awareness is a separate CRDT for transient presence and cursor state, so you do not contaminate the durable document with session-only metadata. The main caveat is that subdocument support varies by provider, so the provider decision matters more than it first appears. citeturn5view0turn25search4turn5view1turn25search0turn25search1

Plate is a strong fit because it already exposes the pieces you are talking about: **Yjs integration**, **comment marks**, **draft comments**, **suggestion marks**, **discussion integration**, **Markdown conversion**, and **DOCX import/export**. That means you can spend engineering time on Quarry’s storage semantics, Git bridge, and filesystem projection instead of rebuilding rich editor collaboration primitives from zero. citeturn4view2turn4view3turn4view4turn24view0turn24view1

**Automerge** is the strongest alternative and probably the best abstraction boundary to preserve. It is explicitly positioned as a local-first sync engine for multiplayer apps, it stores complete history in a compact binary format, supports rich text marks and block markers, includes byte arrays in its data model, and has an explicit ephemeral-data channel for session state. It is conceptually closer to “version control for your data” than Yjs, and it is a very good fit for structured collaborative objects that are not editor-centric. If Quarry eventually broadens beyond live documents into more general collaborative data structures, a `CollabDoc` interface that can host either Yjs or Automerge would be a worthwhile investment. citeturn5view2turn6search2turn26search0turn26search2turn27search0turn27search2turn27search8

For provenance and agent interaction design, **Proof** is worth studying closely. The open-source Proof SDK bundles a collaborative markdown editor with provenance tracking, comments, suggestions, rewrite operations, a realtime server, and an agent HTTP bridge. Its public routes already separate document creation, snapshots, edits, op streams, presence, comments, suggestions, and rewrite flows. Even if Quarry becomes a storage layer instead of an editor product, that route shape is a useful reference for how humans and agents can collaborate on the same artifact. citeturn4view0turn4view1

For “track changes” specifically, one more reference is useful: ProseMirror’s official change-tracking example treats changes as first-class values that can be committed, reverted, and blamed. That is not a drop-in Plate solution, but it is a helpful conceptual model if legal-style review and provenance become core requirements. citeturn3search2

## Git and filesystem projection

The right policy is **Git-compatible, but not Git-constrained**. Quarry should export stable refs to Git branches and import Git commits back into Quarry, but it should not force the entire collaboration model to live inside Git objects alone. Jujutsu is the most relevant precedent here: it uses Git for physical storage and remote interoperability while keeping higher-level metadata outside Git, and the resulting commits are still normal Git commits that can move through ordinary Git remotes. That is almost exactly the pattern Quarry wants. citeturn30view2

Not every representation will round-trip equally well. **Plain text, code, Markdown, and many config-like artifacts can round-trip losslessly through Git.** Rich documents with overlapping comments, inline suggestion marks, provenance rails, and anchored review threads should instead export to a Git-friendly representation such as Markdown plus sidecar metadata. Plate’s two-way Markdown conversion is helpful here because it creates a practical bridge between WYSIWYG editing and Git-readable export. Git’s own commit model remains snapshot-oriented, not operation-oriented. citeturn24view1turn22view0

**Git notes** are useful, but only for a narrow slice of the problem. They let you attach additional text to Git objects without changing the objects themselves, which makes them good for commit-level sync metadata, review summaries, or external references. They are not a good canonical model for text-range comments or binary-region annotations. Those need an object-level annotation system with selectors. citeturn11view4turn14view3turn14view1

For large binaries, the pragmatic export path is **Git LFS first**. Git LFS replaces large files in Git with text pointers and stores the actual content externally, which preserves normal Git workflows while keeping repository size manageable. If Quarry later needs more distributed, offline, multi-remote archival behavior, **git-annex** is the more conceptually relevant inspiration because it manages large files with Git without storing the file contents in Git and is explicitly oriented toward sync, backup, archive, and online/offline use. citeturn11view2turn11view3

For structured or binary-derived diffs, use **`.gitattributes` plus `textconv`**. Git explicitly supports converting binary-oriented data into human-readable text for diffs. That makes it realistic to export not only a raw artifact but also a normalized text projection for review. In Quarry, this can be applied to things like rich-doc canonical JSON, extracted PDF text, AST-normalized code, or other machine-generated review views. citeturn11view5

`git worktree` is useful for Quarry’s publish and preview flows. Linked worktrees share the same repository objects while keeping per-worktree `HEAD`, `index`, and related files separate. Detached worktrees are also explicitly useful for throwaway experimental changes. That fits preview branches, draft exports, and generated Git projections very well. citeturn22view1

For the filesystem layer, **FUSE should be treated as a materialized view, not the one true database**. FUSE is valuable because it provides a userspace filesystem interface and supports secure non-privileged mounts, but userspace filesystems still incur kernel/userspace crossing overhead and semantic rough edges that make them a better projection surface than a canonical write path. Samba then adds practical policy knobs such as `read only`, `write list`, `create mask`, `directory mask`, and configurable VFS modules. That is a good fit for agent mounts, team shares, or controlled read/write exposure. citeturn1search10turn20search0turn10view0turn10view1turn10view2turn10view3

The trap is your proposed gitignored ephemeral directory. If agents are expected to use plain `rg`, then a gitignored, hidden, or binary-only location is the wrong default surface. Quarry should either mount a dedicated **agent view** that deliberately exposes ephemeral artifacts in searchable form, or standardize on search commands that disable ripgrep’s automatic filtering. Otherwise the “filesystem for agents” promise will look true in demos and then quietly fail in real use. citeturn23view2turn23view3turn11view1

## Binary assets, drafts, and transient work

PDFs and other binaries should remain **immutable source blobs** in Quarry, with collaboration happening in overlays. PDF.js is a strong browser rendering layer and explicitly supports rendering annotations and adding a subset of annotation types. Its viewer-side `annotationStorage` can be persisted through `saveDocument()`. But Quarry should still keep the original file and the review layer separate, only baking annotations into an export artifact when someone explicitly needs that. citeturn2search7turn13search1turn13search5

For robust anchoring, store **both semantic and positional selectors**. The W3C model’s **TextQuoteSelector** captures the exact selected text plus prefix and suffix, which is resilient across small edits. A **TextPositionSelector** or **RangeSelector** provides fallback coordinates in the document stream. The spec explicitly lists PDFs as a medium where fragment, text quote, and text position selectors are relevant. For page-region highlights, a page number plus rectangle or SVG selector is a sensible implementation detail layered on top. citeturn14view3turn14view0turn14view1

Your `.draft` idea is directionally right, but it should be a **ref/state concept**, not just a filename suffix. Plate’s comment system already has draft-comment behavior and state tracking, and its suggestion plugin already models inline and block-level suggestions with undo/redo and discussion integration. Quarry should make “save draft” advance a draft ref and “publish” create a stable snapshot, optionally mirrored to Git. A useful precedent here is Jujutsu’s “working copy is automatically committed” model: autosaved state is treated as a first-class object instead of a dirty buffer. Quarry can adopt that spirit without copying Jujutsu literally. citeturn4view3turn4view4turn30view2

You also want two different kinds of temporary state, and they should not be conflated. **Presence and live review cues** belong in transient channels such as **Yjs Awareness** or **Automerge ephemeral data**, because those are designed for non-persistent session information. **Scratch documents** that should persist locally for a user but not publish should live in a local-only namespace and be excluded from shared Git export by policy. Git already distinguishes shared `.gitignore` rules from repository-local exclude rules for this reason. citeturn25search0turn25search1turn27search0turn11view1

## Delivery plan

**Foundation first.** Start by implementing immutable blobs, refs, directory trees, extraction pipelines, and Git export/import for ordinary text and code. That gives Quarry a real storage identity before you add collaborative editor complexity. The object/ref split is battle-tested, and searchable derivatives are required if agent search is a first-class interface. citeturn22view0turn23view2

**Collaborative text second.** Add Plate with Yjs or Yrs next. Use one collaborative doc per rich artifact, with annotation threads and suggestions as first-class overlay objects. Borrow Proof’s public API shape for state, snapshots, edits, ops, presence, and agent actions. That gets you to a usable “human + agent + reviewer” workflow quickly. citeturn4view2turn4view3turn4view4turn4view0turn28view0

**Binary review third.** Once the text path is stable, add PDF.js rendering, extracted-text derivation, and W3C-style annotation selectors. That lets PDFs become first-class reviewed assets without forcing binary quirks into the core model too early. citeturn2search7turn13search1turn14view1turn14view3

**Projection surfaces fourth.** After the canonical model is stable, add the read-mostly FUSE mount, the Samba share, and the public REST/WebSocket surface. Instrument these layers for opens, misses, search skips, selector resolution failures, publish events, and Git import conflicts. Those metrics will matter more than generic storage metrics because Quarry’s whole value proposition is “agents and humans can actually use the same substrate.” citeturn1search10turn10view2turn4view0

**Memory products last.** Only after Quarry is stable as a substrate should you build memory systems on top. Systems like Supermemory, Mem0, and Zep sit above ingestion, extraction, connectors, graphs, and retrieval. Quarry should provide the durable document, annotation, and event substrate they query. It should not try to become the memory product and the low-level storage layer at the same time. citeturn31view0turn31view1turn31view2

## Related systems and resources

**Closest product and interaction references**

- **Proof / Proof SDK** for agent-native collaborative documents with provenance tracking, comments, suggestions, rewrite operations, a realtime server, and an agent bridge. citeturn4view0turn4view1
- **PlateJS** for the editor shell if you want Yjs, comments, suggestions, Markdown conversion, and DOCX import/export without building those primitives yourself. citeturn4view2turn4view3turn4view4turn24view0turn24view1
- **Yjs plus Yrs** for the CRDT layer if rich collaborative documents are the first shipping surface. citeturn25search4turn25search0turn25search3turn28view0
- **Automerge** for a more storage-centric local-first sync engine with history, rich text, byte arrays, and ephemeral data. citeturn5view2turn26search0turn26search2turn27search0
- **Liveblocks Comments** for a clean model of threads, mentions, text annotations, and other collaborative review UX. citeturn17view4

**Storage and versioning references**

- **Jujutsu** as the best precedent for “Git as physical storage and compatibility layer, richer metadata outside Git.” citeturn30view2
- **Radicle** for peer-to-peer Git collaboration and storing social artifacts in Git-backed workflows. citeturn17view2
- **Git LFS** for mainstream binary export, and **git-annex** for distributed large-file sync, backup, and archival behavior. citeturn11view2turn11view3
- **Perkeep** for long-lived, content-addressed storage of files and objects, with search, sync, and FUSE access. citeturn32view0
- **lakeFS** for Git-like branching and atomic merges over object storage. citeturn17view1
- **Dolt** for “Git for data” thinking when a structured query layer matters. citeturn32view2
- **IPFS** for content-addressed, verifiable object storage and transfer ideas. citeturn32view1

**Memory layers that could live above Quarry**

- **Supermemory** for long-term and short-term memory, connectors/sync, extraction, files, and graph-based context assembly. citeturn31view0
- **Mem0** for a universal, self-improving memory layer with both managed and open-source paths. citeturn31view1
- **Zep** for graph RAG, agent memory, and context assembly over user and business data. citeturn31view2

## Questions that will change the design

- **Is Git an interoperability surface, or do you want it to be an equally trusted primary write path?** If it is only a compatibility layer, Quarry can keep a richer canonical model. If not, you will need much stricter round-trip guarantees and more lossy compromises.

- **Is offline-first collaboration a hard requirement from the first release?** If yes, that strengthens the case for a genuinely local-first core and for keeping presence, drafts, and sync semantics out of server-only assumptions.

- **Should the canonical rich-document format be Markdown-first or editor-structure-first?** Markdown-first improves Git ergonomics. Structure-first improves comments, overlapping marks, and future WYSIWYG fidelity.

- **Must agent scratch space be searchable with vanilla `rg`, or can agents rely on a special API or search flags?** This answer changes whether your “ephemeral” area belongs in ignored paths, mounted visible paths, or neither.

- **Do annotations need to round-trip back into original binaries, or is an overlay-plus-export model enough?** If overlay-plus-export is enough, the implementation gets much simpler and more robust.