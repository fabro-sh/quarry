# Multi-block deletion and concurrent projection correctness

Date: 2026-09-06

## Reproduced failures

Deleting a selection across three paragraphs failed with `The target changed: Join requires adjacent sibling blocks`. Typing or pasting over the selection and moving a block immediately before joining it produced the same error.

Plate had already removed or moved blocks. The adapter deferred those structural commands until the end of the input turn, but issued the dependent join immediately. The native document therefore still contained the intervening block.

A delayed deletion test with an agent and two browsers exposed a second failure. The server and editing browser showed the correct text. The observing browser retained deleted text from one source. Native Rust and WASM tests reproduced the discrepancy with identical document heads.

Automerge 0.11.0's `PatchLog::mark` combines consecutive mark events without checking their object IDs. Quarry used display patches to invalidate its source caches. A multi-source deletion could therefore leave a stale cache for one source. The underlying native history retained the correct deletion.

## Changes

- Before a block split or join, the adapter applies earlier pending structural changes inside the same native transaction. The native join continues to require adjacent siblings. Surviving characters retain their original sources and comments.
- Incremental native merges derive cache invalidation from the operations that entered document history. Object ancestry identifies the affected sources. Source-map changes and block records also invalidate the relevant caches. This no longer depends on display patches to identify every changed object.
- The API schemas and stored document format are unchanged.

## Regression coverage

- Delete, type, and paste over paragraphs, lists, code lines, and existing block proposals.
- Forward and backward selections over two, three, and five blocks, with Unicode and bold text.
- Comments on surviving and removed text, native undo/redo, proposal acceptance, and replay of browser requests through Rust.
- Delayed API edits during browser deletion, a second observing browser, late comments from an earlier read, and undo that preserves the agent's insertion.
- Native incremental delivery with warm caches, multiple marked or deleted sources, duplicate delivery, undo, and comparison against a fresh archive.

## Evidence

Artifacts are in `target/block-join/`.

- `reproduction.log`: four tests fail with the original adjacent-sibling error before the adapter fix.
- `remote-reproduction.log` and `native-remote-reproduction.log`: WASM and Rust reproduce stale text in the second browser before the cache fix.
- `rust.log`: 500 Rust tests passed.
- `unit.log`: 271 UI tests passed across 22 files.
- `chrome-final.log`: three installed-Chrome tests passed, including both delayed browser/API scenarios.
- `browser-final.log`: 335 production browser checks passed across Chromium, Firefox, and WebKit. Four existing platform-specific tests were skipped.
- `performance.log`: all six isolated, headed Chrome benchmarks passed. The worst p95 key-to-frame time was 14.5 ms, and the largest frame gap was 24.8 ms. The existing limits are 20 ms and 50 ms. The document benchmark contains 114,814 Markdown bytes and tests the beginning, middle, and end, concurrent agent review, and Suggesting input.
- `clippy.log`, `build.log`, `server-build.log`, and `architecture.log`: static checks and production builds passed.
- `qa-restart.log`, `qa-check.json`, and `qa-browser.log`: the local QA server was updated. Document blocks and review data matched before and after restart. Eight installed-Chrome tabs showed Saved, one WebSocket each, and no application errors.
