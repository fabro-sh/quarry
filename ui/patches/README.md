# Slate chunk rendering

`slate-react@0.120.0.patch` backports the upstream [Strict Mode fix, #5988](https://github.com/ianstormtaylor/slate/pull/5988), commit `bfa5055f75fcf64ee71c2aee8f1ca43dc50222f4`.

The affected release clears pending chunk updates during render. React Strict Mode renders twice, so the second render can skip an edited chunk. The fix clears pending updates after React commits. Both module entry points receive the same fix.

Plate 52 pins Slate React 0.120.0. Later Slate React releases also require changes to Slate and Slate DOM. This small backport retains Plate's compatible dependency set. Remove it when upgrading Plate to a version that includes the upstream fix.

`tests/live-plate-chunks.spec.ts` checks typing and remote highlights in distant chunks, including save and reload. `tests/live-performance.spec.ts` enforces the Chrome typing latency limits with chunking enabled.

The same patch reads the current DOM selection before keyboard and clipboard handlers run. This part is a Quarry fix. Slate throttles selection updates for 100 ms. A browser can also deliver an input event before `selectionchange`. A fast click followed by Tab or paste could then use an old selection. The patch calls Slate's existing selection handler and flushes it before invoking plugins. It retains the handler's composition and pending-render checks. `tests/live-editor-input.spec.ts` checks immediate table navigation. `tests/live-plate-upload.spec.ts` checks image placement when paste follows selection and an agent moves the destination during upload.

# Exact selections across tables

`@platejs%2Ftable@52.0.11.patch` adds the `disableSelectionExpansion` option to Plate's table selection handler. This is a Quarry patch. The default Plate behavior expands a selection that crosses a table boundary to include the whole table. Quarry uses native text commands that can retain the unselected parts of each cell, so it disables that expansion. Other table selection behavior remains upstream.

The native conformance test replaces a selection from a paragraph into the middle of a cell. It checks the exact surviving characters, comment identity, unchanged neighboring cells, undo and Rust replay. Keep this option when upgrading Plate, or remove the patch if upstream adds equivalent support.

# React selection capture

`react-dom@19.2.1.patch` stops React's contenteditable selection scan after it finds both endpoints. The released code continues through the full document on every commit, even when the caret is in the first heading. This is a Quarry optimization. It preserves the existing offset calculation and fallback when an endpoint is outside the focused element. All four client and profiling builds receive the same change.

`src/features/editor/react-selection.test.ts` exercises React's actual commit and selection restoration in development and production. It checks collapsed selections, both directions, element boundaries, nested text and UTF-16 offsets. It also checks that capturing a selection near the start does not read distant text. Browser input tests cover the integration with Slate. The Chrome performance tests retain the existing latency limits. Remove this patch when the installed React version stops scanning after both selection endpoints are found.
