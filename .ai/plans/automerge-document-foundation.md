# Automerge document foundation

Status: implementation authorized. This plan records the work and is open for review while implementation proceeds.

## Outcome

Use Automerge for the durable document. Use ProseMirror for editing. Put document and review rules in one Quarry command layer. Commit a command and its receipt before publishing its result.

The first requirement is to preserve text identity through moves, splits, comments, and edits that arrive later. The comparative spike showed that the default editor bindings do not meet this requirement.

## Document model

Store text in Automerge text objects. Divide those objects into segments with persistent boundary markers. Blocks refer to segments. Their placement and structure are separate from their text.

- A move changes block placement without copying characters.
- A split inserts a boundary and gives the following segments to a new block. Characters retain their identities.
- A join places existing segments in the same block without copying their text.
- A comment refers to native text ranges and has an independent discussion record. Its target can appear in more than one block after a split.
- Proposed text has its own text identity before acceptance. Acceptance places that text in the document without reconstructing it.
- Structural commands are serialized by the document authority. Concurrent text changes and comments retain their original references. The server validates imported changes before publishing them.

This is a design to prove, not an assumption that stable block IDs alone solve the problem. Boundary insertions, repeated text, Unicode, deletion, split/join cycles, and delayed changes must all have explicit results.

## Work

1. Add a Rust document crate with Automerge storage, typed commands, independent review records, and stable text references.
2. Prove moves, splits, joins, and proposal placement against changes made from older versions. Test both merge orders, persistence, and JavaScript interoperability.
3. Add atomic document persistence and request receipts. Failed commits must leave live state unchanged. Repeated requests must return their original result.
4. Route server document commands through the durable engine. Preserve the existing API's useful document and review behavior while removing reconstruction as the source of truth.
5. Add a ProseMirror adapter that translates explicit editing operations into the document model. Preserve the supported document schema and review controls. Avoid whole-document text diffs for structural edits.
6. Migrate stored documents and review targets once. Keep block rows and Markdown as projections. Validate stored state before changing the active document format.
7. Run domain, storage, server, editor, and end-to-end checks appropriate to each change. Retain counterexamples as regression tests.

## Completion criteria

- Moving or splitting a paragraph preserves edits and comments made from the prior document version.
- Repeated text cannot redirect a comment to another paragraph.
- Deleted target text has an explicit target state; deleting text does not delete its discussion.
- Accepting a proposal preserves comments on its proposed text and applies the proposal once.
- Save/reload retains native references and accepts valid delayed changes.
- Browser and agent commands share document semantics.
- Successful responses acknowledge durable state. Failed commits publish no candidate changes.
- Markdown and editor projections do not reconstruct the canonical document during normal operation.

## Unresolved questions

No user answer is required to begin. The main technical question is whether the segment model passes the combined move/split and boundary tests without additional identity rules. The proof will determine the final schema before migration.
