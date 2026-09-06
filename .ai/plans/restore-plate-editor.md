# Restore the Plate/Slate editor UX

Restore the editor that existed before the Automerge migration. Keep Automerge as the authority for document content, structure, review targets, history and durable saves.

## Work

1. Restore the original Plate components, floating toolbar, block menus, review cards, table controls, wiki-link chips, images, diagrams, and Contents navigation. Use the previous pinned dependency versions. Do not restore the retired synchronization or Markdown mirror paths.
2. Add a tested adapter from Slate operations to native document commands. Preserve native character identity through typing, formatting, split/join, moves, clipboard operations, undo and remote changes. Keep native history and review targets authoritative.
3. Connect the original editor UX to the existing native session, offline outbox, permissions, presence, review operations and archive support.
4. Restore relevant original UX tests and retain the native correctness tests. Test both the visible controls and their persistent effects. Cover concurrent agent edits, private comment drafts, Unicode, composition, offline restart, and retries.
5. Compare Chrome typing with the original build. Run the real-browser suite and the large-document/concurrent-edit performance gates. Update the running local QA instance when the restored editor is ready.

## Completion

The app uses the actual Plate/Slate editor, with its established interactions and presentation. Automerge remains the sole document authority. Every restored interaction has conformance coverage; the previous UI gaps are not accepted as migration tradeoffs. Save and reload preserve document and review identity. Chrome responsiveness remains within the measured performance gates.

## Unresolved questions

None. The user has authorized restoring Plate/Slate while retaining Automerge.
