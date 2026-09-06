# One Document Engine and Durable Review

Status: architecture proposal for review. No implementation authorized by this document.
Date: 2026-09-04

## Recommendation

Give Quarry one versioned document model, one mutation engine, and durable review records. Every accepted edit must account for the targets of existing comments and suggestions. The browser draws highlights from those records. Saving never reconstructs the records from highlights.

For Quarry's documented online-only, single-server requirements, prefer a central transaction authority. Prove a ProseMirror-based implementation in a bounded prototype before committing to the editor replacement. Use its existing collaboration and position-mapping code in both the browser and the authority. Do not write a new collaboration algorithm or independently port its transforms to Rust.

This is a larger change than repairing the current comment codec. It removes the recurring need to reconcile independently editable representations of the same document.

## Why the current model keeps failing

The current source changes write authority when a browser connects. Agent operations run against block rows. The live browser stores review anchors as formatting marks. Checkpoints reconstruct review rows from those marks. A missing mark is interpreted as deleted text.

The investigation reproduced three failures in tests against the production codec:

- An agent comment on a linked paragraph rebuilds its inline content and can destroy an in-flight browser comment anchor without changing the text.
- Comments on proposed replacement text are omitted from the stored projection.
- A paragraph with a wiki link can fall back to raw Markdown and lose anchors on unchanged neighboring text.

The gateway also deliberately kills a whole comment anchor when a deletion or replacement overlaps any part of its selected text.

These are violations of the document contract. Rendering, parsing, and synchronization currently have the power to change the meaning and existence of review records.

## One authority and one commit

The canonical document aggregate contains:

- The structured content tree and stable node IDs.
- Comment threads, their targets, and discussion state.
- Suggestions, their proposed content, targets, and decision state.
- A monotonically increasing revision.

SQL stores snapshots, accepted transactions, and retry receipts. These are parts of one durable history. Search indexes, Markdown exports, and browser decorations are derived views.

All writers submit transactions to the same authority:

```text
Browser edits     Agent commands     Markdown file imports
      \                 |                 /
       +-------- Document authority -----+
                         |
           Validate against a revision
                         |
         Apply content and review operations
         Map existing and newly created targets
                         |
             One atomic database commit
                         |
             Broadcast accepted transaction
```

Each accepted transaction records content operations, explicit target transitions, actor, revision, and idempotency receipt. Content changes and target changes commit together. There is no rows/session mode switch.

Browsers apply edits optimistically for responsive typing. The collaboration engine maps their unconfirmed edits over accepted remote edits. Only committed changes are acknowledged as saved or broadcast by the authority. A failed commit leaves the canonical state unchanged. Rejected local work stays available for recovery.

Comment creation belongs in the same ordered transaction stream as text editing. A comment created on locally typed text must depend on that insertion. It cannot race ahead through a separate metadata channel. A client transaction that contains both operations carries both together.

Use one retry identity per submitted logical transaction. A matching retry returns the stored outcome. Reusing a key for a different payload fails. This semantic receipt remains necessary even if a transport offers duplicate-safe updates.

## Review targets are document data

Use explicit target types:

| Target | Example |
| --- | --- |
| Text selection | One or more ranges in a specific content revision, including selections across blocks |
| Node | A heading, image, table, or opaque source block identified by stable ID |
| Suggestion content | A range within the proposed fragment of a specific suggestion |
| Document | A general review thread |

Text positions belong to the document model. DOM positions, serialized Markdown offsets, and editor leaf boundaries are adapter details. A target can refer to text inside a link without depending on the link's rendering wrapper.

Store the original quote, original revision, and surrounding context with each thread. They provide history and repair evidence. They are not permission to silently attach a comment to similar text elsewhere.

Keep discussion state separate from target state:

- Discussion: open or resolved.
- Target: attached, original content deleted, or needs reattachment.

A resolved thread can remain attached. An open thread can refer to deleted text. Neither condition means the thread is corrupt. The review panel always exposes these threads and their original context.

Treat an unexplained missing target as an invariant failure, not a normal orphan transition. The engine must either produce a valid target or record the operation that deleted or explicitly detached it. A renderer failure cannot produce that operation.

## Every edit carries a position mapping

The engine applies operations and produces a mapping from their input positions and identities to their output positions and identities. It uses that mapping for comments, suggestions, selections, and pending operations where their policies agree.

This mapping comes from the actual accepted operations. Comparing before-and-after Markdown cannot reveal whether text was moved, copied, deleted, or rewritten.

ProseMirror provides mappings for ordinary document steps. That is useful infrastructure, not a complete Quarry review policy. Quarry must add explicit mappings for semantic moves, replacement decisions, suggestion acceptance, and target state transitions.

| Operation | Required target behavior |
| --- | --- |
| Add, reply to, resolve, or edit a comment | Content is unchanged; other targets are unchanged |
| Add or remove bold, a link, or other formatting | Preserve targets on the underlying text |
| Insert before a selection | Move the selection by the inserted length |
| Insert inside a selection | Include the inserted text |
| Insert exactly at a selection boundary | Exclude it by default; use the same explicit boundary policy for all writers |
| Delete part of selected text | Retain the surviving selection; do not detach the whole thread |
| Replace text within a larger selection | Map the surrounding selection across the replacement |
| Delete or replace all selected content | Preserve the thread and original quote; record that its original content was deleted or replaced |
| Move a node | Preserve node identity and explicitly relocate its text targets |
| Split or join blocks | Map selections through the structural operation, including selections that now span blocks |
| Copy content | Allocate new node IDs; do not duplicate existing threads implicitly |
| Undo or redo | Use the operation history and corresponding mappings; restore original attachments when undo restores their targets |

When all selected content is replaced, automatic attachment to replacement text is allowed only when an explicit semantic operation establishes that correspondence. A broad paragraph rewrite does not establish which new sentence replaces an old commented sentence. Otherwise keep a historical target and offer explicit reattachment.

A collapsed position alone is insufficient evidence that content was deleted. Node targets and insertion suggestions legitimately have no text range. Inspect the actual operation and the target type.

## Suggestions own their proposed content

A suggestion is a proposed operation with its own stable ID, applicability conditions, and structured replacement fragment. Its proposed text is addressable before acceptance. The browser displays that fragment as a preview; saving does not mix it into accepted text and then remove it again.

Comments on proposed text target the suggestion fragment. Accepting the suggestion atomically:

1. Checks that the suggestion is still open and applicable to the current content.
2. Applies its proposed content operations.
3. Produces a mapping from the proposed fragment to the accepted content.
4. Transfers comments on that fragment through the mapping.
5. Records the decision and the resulting revision.

Rejecting a suggestion preserves its discussion and archived proposed text. The thread remains readable in its original context. It does not become an unexplained orphan.

Concurrent accept/reject operations have one serialized winner. Retry receipts make that outcome repeatable. Overlapping proposals are revalidated against current content before acceptance; a stale proposal cannot overwrite intervening work silently.

## Stale positions and external files

An agent target must name the revision at which its positions were measured. Prefer server-issued target handles over asking an agent to calculate offsets. Validate an expected quote against that revision when supplied.

The authority maps a stale target through retained accepted operations, or returns a typed conflict when the history or correspondence is unavailable. It must never apply old numeric offsets to current text and label that a successful rebase.

Markdown remains an import/export format. Whole-file writers keep a merge base and compile the result into the same transaction vocabulary. They cannot write canonical snapshots directly.

Plain Markdown does not encode document identity. An ambiguous rewrite cannot guarantee exact attachment preservation. Keep exact mappings where the merge base establishes them. Preserve ambiguous threads with their original context and request reattachment. Use quote matching only to suggest candidates for user confirmation.

The schema must represent every supported editor feature. Unknown content remains an opaque node with stable identity and preserved source. A display or export fallback must not flatten canonical content or discard its review records. Export failure can fail the export without corrupting the document.

## Collaboration engine choice

| Option | Assessment |
| --- | --- |
| Keep improving the rows/session bridge | Keeps two edit representations and repeated reconstruction of review state. Reject as the target architecture. |
| Central authority using one shared ProseMirror transaction engine | Preferred prototype for the documented online-only, single-server product. Reuses established step mapping and browser rebasing. Requires an editor change and a server runtime for the shared TypeScript engine. |
| One durable Yjs/Yrs document with independent review records | Coherent alternative if retaining Yjs/Rust integration or future offline work is a hard requirement. Requires durable collaboration identity and an editor binding that preserves it. |

The ProseMirror choice is provisional until the prototype passes. Its existing collaboration module handles browser text rebasing. Quarry still needs its own ordered review commands, suggestion model, structural mappings, persistence, and recovery. Use the same engine code on the server; a separate Rust implementation would recreate the current drift risk. Rust can retain storage and HTTP responsibilities, but the runtime boundary and deployment cost must be measured.

If choosing Yjs/Yrs instead, commit the actual shared document state and review records together. SQL block rows become rebuildable read views. Do not recreate the shared document from Markdown or rows on each session. Relative positions help track text edits, but they do not survive deletion of their containing shared type. Node moves, splits, and accepted replacements still need identity-preserving operations or explicit target mappings. Persisting today's Slate-shaped document alone would preserve today's destructive behavior.

Both alternatives require the same review contract. Pick one collaboration engine. Do not combine a ProseMirror authority and a second Yjs authority.

## Relationship to the existing command-pipeline proposal

The July `durable-document-command-pipeline.md` proposal correctly calls for one command lane, atomic commits, scoped identity, and commit-before-publish. Retain those decisions.

Revise the part that accepts browser updates by projecting a Yjs candidate into rows, then reconciles server changes back into a live Yjs document. That still derives intent and targets from a conversion between editable representations. The new engine must receive actual edit operations and apply their target mappings directly.

This document proposes that revision. It does not directly edit or supersede the earlier review document.

## Proof before cutover

First write an executable review contract independent of the chosen engine. Then build one complete path: two browser clients, an agent client, durable storage, comments, one replacement suggestion, and restart recovery.

Required checks:

- A transaction containing only review operations leaves document content unchanged.
- Adding a comment does not remove or move any other comment target.
- Concurrent browser and agent comments survive on plain text, linked text, and proposed text.
- Partial deletion retains the surviving attachment.
- Full deletion preserves the thread, original quote, and an explicit deletion cause.
- Moves, splits, joins, and undo preserve or restore the specified target identity.
- Accepting a suggestion transfers comments on its proposal exactly once.
- Rejecting a suggestion preserves its discussion.
- Replaying a request does not duplicate content, comments, or decisions.
- Clients converge on content, review records, and target states after delivery delays, duplicates, and supported reordering.
- Restart restores the same accepted state. A failed commit is never broadcast as accepted.
- A stale agent request maps correctly or fails explicitly.
- Unsupported display/export does not mutate the canonical document.

Use generated operation sequences with an independent reference model for review semantics. Check after every accepted transaction, not only at final text convergence. Save failing seeds and shrink them into regression cases. Test browser adapters as well as engine functions; normalization and input handling are part of the correctness boundary.

Measure typing responsiveness, server cost, storage growth, and recovery on representative documents. Compaction must retain the maps needed by active clients and pending commands. Set a supported history window; clients older than that window enter explicit resync/recovery. Never guess positions after dropping the required history.

After the prototype passes:

1. Finalize the schema, transaction protocol, target rules, and engine choice.
2. Implement durable snapshots, accepted history, retry receipts, and browser acknowledgements.
3. Port browser editing and review actions to the shared engine.
4. Compile agent and whole-file writes to that engine.
5. Rehearse conversion of existing documents and review records, preserving uncertain threads as historical records.
6. Cut over in one release and delete the old session projection, review-mark reconstruction, and alternate mutation path.

Being a new app makes a coordinated breaking change reasonable. Existing user documents still need a verified backup and conversion check.

## Sources

- Current implementation: `crates/quarry-server/src/gateway.rs`, `crates/quarry-server/src/session.rs`, and `crates/quarry-collab-codec/src/session_doc.rs`.
- Existing requirements: `docs/superpowers/specs/2026-06-09-session-scoped-collab-design.md`.
- [ProseMirror collaboration guide](https://prosemirror.net/docs/guide/#collab): central authority, revisions, and rebasing of unconfirmed browser steps.
- [ProseMirror position-mapping implementation](https://github.com/ProseMirror/prosemirror-transform/blob/master/src/map.ts): step mappings and deletion information. Mappings are infrastructure; the review policy above is a Quarry design proposal.
- [ProseMirror collaboration example](https://prosemirror.net/examples/collab/): an example of annotations stored outside the document's formatting marks.
- [Yjs relative positions](https://docs.yjs.dev/api/relative-positions): text-relative positions and the inability to resolve a position when its containing type is deleted.
- [Yjs document updates](https://docs.yjs.dev/api/document-updates): persistence and synchronization of shared state, and update-merging limits.

## Unresolved questions

1. Are online-only editing and a single document authority still the intended constraints? Recommendation: keep them for this design unless offline editing is now a concrete requirement.
2. Is replacing Plate/Slate and running a shared TypeScript document engine on the server acceptable? Recommendation: evaluate it in the bounded prototype before selecting the engine.
3. Should a comment whose entire target is replaced automatically follow the replacement? Recommendation: require explicit correspondence from the operation; otherwise retain a visible historical target.
