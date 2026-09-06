# Architecture

Quarry stores each Markdown document as a durable Automerge document. The Rust `quarry-document` crate owns its schema, text identity, structure, review targets, commands, merge rules and validation. The browser runs that same crate through `quarry-document-wasm`. Plate/Slate handles browser input and displays the native projection.

A document has one authority throughout its lifetime. Opening or closing a browser does not change its representation or durability rules.

## Document identity and review

Each block has a stable ID. Text lives in native sources. Blocks and proposals own segments of those sources. Split, move, join and proposal acceptance transfer ownership without copying surviving characters.

Comments retain the exact native characters at the boundaries of the original selection. Boundary insertions stay outside the selection. Changes inside the range remain inside it. Deletion retains character identity while hiding the deleted characters. A concurrent insertion cannot inherit another character's deletion. Undo can restore those characters and their targets.

A discussion's status is separate from its target's visibility. A target can be attached, hidden by removal of its block, deleted, or unavailable from an old import. Removing a target does not remove the discussion or search for a similar quote elsewhere. Replies and decisions remain part of native history.

Proposals own their inserted text and proposed blocks before acceptance. Comments can target that content. Acceptance transfers its ownership into the body. The engine checks that the current deletion target or destination still matches what the reviewer is being asked to approve. A changed proposal remains available for discussion, with an explicit reason why it cannot be accepted.

## Commands and publication

Browser requests carry a request ID, native base heads, explicit commands and attribution. Positions refer to the native version in which the user selected or edited text. The server validates the request against that version and merges the result into current state. Conflicting structural changes require a new decision; they cannot silently replace a later edit.

The server serializes writers by document ID. A candidate remains private until a SQL transaction commits all of these together:

- Automerge state and the resulting native heads.
- Derived block and review indexes.
- The Markdown projection, metadata and document version.
- The command receipt and transaction record.

A response acknowledges that commit. A retry with the same request ID and payload returns the recorded result. Reuse of an ID with different content fails. Rollback cannot expose a candidate to another browser.

The public agent transaction API requires the `base_clock` of the read used to construct its operations. It resolves offsets against that saved version and translates block, comment, suggestion and conflict operations into native commands. There is no separate row mutation engine. Missing clocks fail before mutation. Delayed subtree deletion rejects unseen text or formatting changes. Delayed typing into a removed block fails explicitly; late comments retain a hidden target. Clients must retain their read content and clock together, and retry an uncertain delivery with the original ID and payload.

## Browser persistence and synchronization

The browser records each command request and its draft snapshot in one strict IndexedDB transaction. Requests have independent rows, so tabs cannot overwrite each other's pending requests. The browser sends them in causal order and uses durable server receipts to tolerate duplicate delivery and lost responses.

Never-sent text edits on the same source can share one publication. Combining them and marking that delivery as attempted happen in one IndexedDB transaction before transport. Each native command retains its original identity and base. Once attempted, the delivery's ID and payload cannot change. Drain passes are finite so continuous typing cannot prevent remote refresh.

Typing applies locally through the shared native engine. The browser caches decoded projections and appends native changes to its saved archive. It does not wait for the network or serialize the full Markdown document before displaying a keypress. Cache tests compare warm projections and incremental archives with fresh native loads.

Document events tell clients to refresh. After the initial archive, clients request native changes since their last received heads. Applying those changes validates the complete candidate and invalidates only affected caches. It does not reload the full archive on the input thread. Events do not supply replacement editor JSON. Library clients use document IDs through path changes. Temporary clients retain the document capability URL. Invitation tokens accompany library reads, commands and event subscriptions. Viewer invitations cannot publish document changes.

Offline changes remain on the device. A cached document can reopen during a network outage. A rejected or expired capability does not fall back to an editable cache. Failed structural requests retain their drafts for recovery and download. The interface distinguishes saving, saved, and failed or offline changes.

Native cursors preserve selections through remote edits. Remote merges wait until IME composition ends. Remote cursor presence is temporary, expires without activity, and creates no document versions.

## Markdown and other writers

`quarry-markdown` parses and writes an editor-independent syntax tree. Markdown, SQL block rows and Plate/Slate nodes are projections. None can replace an existing document's native history.

Initial Markdown import accepts review markup and endmatter. Existing whole-file writes reconcile body Markdown into explicit operations. Git and FUSE supply merge bases; REST updates require the original read version, as a historical merge base or a strict ETag precondition. Unversioned REST writes create only; updates return 428. Conflicting hunks become review records in the same commit as nonconflicting changes. Agents modify review records through the review operations. CLI updates require `--base-version`; plain Git imports and FUSE temp-file replacements cannot overwrite existing Markdown without its original read version. Open FUSE handles retain document identity and merge bases across renames.

Direct storage puts, staged multi-document commits, restores, CLI, Git and FUSE publication all update native state. A staged commit retains its SQL atomicity across paths. Unsupported syntax remains a source block where the codec cannot represent it without loss. Raw files retain their byte representation. An existing Markdown document cannot be downgraded to raw bytes and lose its native history.

## Upgrade and portability

Startup imports Markdown documents that do not yet have native state. Each import is atomic. A restart resumes the remaining documents and leaves completed native state untouched.

When SQL rows and Markdown review markup both exist, import retains old block IDs only after their complete structures and content agree. Uncertain records retain their original data as explicitly unattached discussions or unavailable proposals. Import never attaches them by finding repeated text.

Plain Markdown export contains the body and frontmatter. A `.quarry` archive contains native bytes and metadata, including retained characters, review targets, decisions and history. Archive import creates a separate document identity while preserving internal text and block identity. It cannot overwrite an existing document. Forks follow the same identity rule.

Wiki-link syntax is atomic in plain Markdown. Export keeps a complete link and the formatting shared by all its characters. Partial formatting inside its target, anchor or alias stays in native state and archives; Markdown cannot express it without changing the link. Incomplete syntax exports as literal text. Import preserves formatting around complete wiki links.

## Operating boundaries

One Quarry process owns each database and CAS root. SQL publication and authorization remain server responsibilities. Automerge supplies durable document identity and merge history; it does not provide account authentication or distributed database consensus.

The library surface trusts the local deployment boundary. Temporary documents use bearer capabilities and expiration. Names and agent attribution are self-reported. Native history and archives can retain deleted content; deleting visible text is not secure erasure.

Whole-file Markdown merges are less precise than native text commands. Native commands should be preferred when the caller has document positions. Browser undo history groups local input during the current editor lifetime; persisted document history and offline drafts survive reload independently.

Upgrades import committed Markdown and review records. If an older database has unpublished binary drafts, startup stops before schema changes. Save those drafts with the older Quarry version, then upgrade. Clean recovery caches are removed after import. Native archives retain complete history after the cutover.
