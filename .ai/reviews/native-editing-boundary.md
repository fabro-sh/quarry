# Native editing boundary: implementation and verification

Date: 2026-09-06
Status: implemented, verified, and running on the local QA server.

## Root cause and change

The old adapter applied text immediately but reconstructed structure at the end of the input turn. A dependent join could therefore run against a tree that still contained an earlier removed or moved block. The previous split/join synchronization guard repaired that ordering for particular operations.

The adapter now executes canonical structural operations in order through the existing private native command builder. It checks the final Slate structure against native state; that check does not generate mutations. The split/join guard and final canonical tree reconstruction are removed. Structural edits inside an existing block proposal also use ordered native actions, without reconstructing that proposal from the final editor tree.

Rust/WASM now interprets selection replacement, text ownership transfers, container splits and joins, and insertion, movement, or removal inside an existing proposal. `replace_selection` joins surviving sibling text blocks and retains container boundaries for selections across containers. Fully removed middle blocks retain their text for hidden review targets and undo. `move_text` transfers source references in canonical or proposed blocks, without temporary blocks or copied characters.

New clipboard trees receive valid text owners before native identities are allocated. Plugin split properties and move destinations are applied in the same transaction as their structural change. The same request replays in the authority against its recorded document version. Unseen edits in a block targeted for whole-block deletion still cause an atomic conflict. Concurrent text survives ownership transfers.

Plate/Slate remains the input and presentation layer. Ordinary typing stays on the local WASM text path. Canonical Suggesting behavior and proposal continuation are preserved. IME composition can publish progressive, valid updates while retaining one undo group. Physical keypress replacement, deletion, and paste publish one native request.

The native-history cache invalidation fix from the preceding work remains in place. Display patches do not determine which source caches are invalidated.

## Verification

Artifacts are in `target/native-editing/`.

- `rust-verified.log`: 507 Rust tests passed across document, Markdown, storage, and server crates with all features.
- `unit-complete.log`: 284 UI tests passed across 22 files. The 110 Plate conformance cases replay the requests in native Rust.
- `browser-verified.log`: 347 production browser tests passed in Chromium, Firefox, and WebKit; four existing platform skips.
- `chrome.log`: five additional installed-Chrome tests passed, including delayed API edits, a second browser, selection replacement, and proposed table edits.
- `performance-normal.log`: six isolated, headed Chrome performance tests passed with the unchanged 20 ms p95 and 50 ms frame-gap limits.
- `clippy-verified.log`, `build-verified.log`, and `performance-types.log`: Clippy with warnings denied, production WASM/TypeScript/Vite build, and the performance fixture type check passed. Formatting, architecture inventory, and diff checks also passed.

New coverage includes selections in both directions, Unicode and formatting, source-preserving text movement, generated combinations of move/remove/join/split operations, edits inside proposals, native builder atomicity, late comments, concurrent endpoint typing, rejection of unseen middle-block changes, undo/redo, and comparison of incremental readers with fresh native archives. A selection ending in the private trailing input also survives repeated undo without reusing an identity.

A full UI run initially hit a timeout; its isolated rerun and the complete rerun passed. The first browser request-count check exposed that Playwright's `insertText` invokes IME composition in Firefox. The suite now tests physical keypresses and that input method separately, retains both cases, and checks their respective request and undo contracts.

## Chrome responsiveness

Installed Chrome: 152.0.7977.77. Document: 114,814 bytes, with 1,400 body paragraphs.

| Scenario | p95 key-to-frame |
| --- | ---: |
| First block | 14.1 ms |
| Middle block | 11.2 ms |
| Last block | 11.7 ms |
| Agent review during typing, first block | 9.3 ms |
| Agent review during typing, last block | 10.5 ms |
| Suggesting typing | 13.4 ms |
| Suggesting repeated deletion | 14.7 ms |

The largest frame gap across these checks was 24.7 ms.

The first latency run failed while Chrome's Energy Saver was enabled and the Mac reported 9% battery. A blank editable page measured 35.3 ms p95, the previous QA build measured 35.9 ms, and the new build measured 35.4 ms at the first block. Idle frames arrived approximately every 33 ms. These controls are retained in `frame-control.json`, `previous-performance.log`, and `performance.log`.

Chrome documents that Energy Saver reduces its capture rate on battery power. Its preference distinguishes disabled mode from activation below the battery threshold. [Chrome performance settings](https://support.google.com/chrome/answer/12929150?hl=en), [Chromium preference definitions](https://chromium.googlesource.com/chromium/src/+/HEAD/components/performance_manager/public/user_tuning/prefs.h).

The opt-in `QUARRY_PERFORMANCE_NORMAL_POWER=1` fixture disables Energy Saver only in a disposable benchmark profile and records its before/after setting. It leaves the user's Chrome settings unchanged. The blank-page control then returned to roughly 8 ms frames, and all six headed performance tests passed. The default benchmark behavior and latency limits are unchanged.

Reproduce the controlled run from `ui/`:

```sh
CARGO_TARGET_DIR=target/cutover \
QUARRY_LIVE_API_PORT=7841 QUARRY_PRODUCTION_UI=1 \
QUARRY_SYSTEM_CHROME=1 QUARRY_PERFORMANCE=1 \
QUARRY_PERFORMANCE_NORMAL_POWER=1 \
bunx --no-install playwright test -c playwright.live.config.ts \
  --project=chromium --grep Chrome --headed --workers=1 \
  --output=../target/native-editing/performance-normal-results
```

## QA and handoff

The server on port 7836 runs the verified release build. Restart snapshots confirm that the review sandbox's blocks and review data are unchanged. A closed-database backup precedes the update.

`qa-check.json` confirms that eight installed-Chrome tabs reach Saved, each uses one WebSocket, and no page errors or HTTP event streams occur. The manual QA document and review remain unchanged after those checks. Reload existing tabs to load the new editor bundle.

The implementation plan was completed in Quarry, its review stream remains connected, and its Markdown was synced back to `.ai/plans/native-editing-boundary.md`. Agent documentation and generated native API schemas include the new actions. Changes remain uncommitted.
