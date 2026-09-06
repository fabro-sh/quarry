# List block conversion

Verified on 2026-09-06.

Changing a list item to a heading changed its type but retained `listStyleType` and related attributes. The native document correctly rejected the invalid heading. Choosing Text also failed to leave the list because both the list item and plain text use the native `p` kind.

The shared conversion action now removes list membership, indentation, checked state, and numbering properties together with the type change. Both the floating toolbar and block menu use this action. Suggesting mode creates one block-property proposal. List-to-list changes preserve their existing toggle behavior.

Tests also found that edits inside an existing block proposal retained properties that Slate had removed. The adapter now applies Slate's property-removal semantics before submitting that block update. The native schema remains strict.

## Evidence

- All 241 UI tests passed, including 67 tests that replay browser document requests through the native Rust engine and compare document views.
- New native cases cover bullet, numbered, and task items converted to plain text, all six headings, quotes, and code. They check block identity, text sources, comment targets, neighboring list items, unrelated alignment, and undo. Proposal cases check a single review decision, acceptance after a text edit, and conversion inside an existing proposed block.
- Production browser tests: 64 passed across Chromium, Firefox, and WebKit; two existing WebKit drag tests were skipped. All 42 new conversion cases passed. These use both menus and check H3 conversion, plain text, undo/redo, reload, formatting, comments, and acceptance after an HTTP agent edit.
- Installed Chrome 152.0.7977.77: all four focused H3 cases passed through both menus in Editing and Suggesting modes.
- TypeScript, the production UI build, the release server build, the architecture check, and `git diff --check` passed.

Logs and browser artifacts are in `target/list-conversion/`. The initial browser run exposed a test setup error: after reload, the test needed to open the Comments tab before looking for the suggestion card. The final run passed without application changes.

The QA server on port 7836 now serves this build. A closed-database backup was taken before restart. The document blocks and review data matched before and after restart. Eight installed Chrome tabs loaded with Saved status, one WebSocket each, and no application errors. That read-only check also preserved the document and review data.
