# Complete the native editing boundary

Status: implemented and verified. Date: 2026-09-06

## Outcome

Keep Plate/Slate's editing experience and local WASM response. Give each completed user action one native interpretation, including structural changes, comments, proposal ownership, and undo.

## Implementation

1. Extend the existing Rust edit interpreter with identity-preserving selection replacement, text movement, and structural edit sequences. Keep strict validation at the transaction boundary and retain exact delayed-write checks.
2. Translate Plate plugin operations into an ordered sequence addressed by native block identities and the request's document version. Apply text and structural steps in that order. Remove final-tree reconstruction as the source of canonical structural edits and remove operation-specific synchronization guards.
3. Use the same native actions for direct edits and edits inside existing proposals. Preserve Suggesting decisions, proposal continuation, input selection, composition, and native undo. Route selection replacement and text movement through the native interpreter.
4. Regenerate API schemas and update agent instructions for the completed native actions.
5. Add native and browser conformance tests for generated edit sequences, delayed API edits, concurrent browser observations, comments, undo/redo, and reload. Compare incremental state against fresh native archives throughout.
6. Run the full relevant Rust, UI, and production browser suites. Run isolated headed Chrome responsiveness benchmarks. Update the existing local QA server after validation and preserve its document and review data.

## Contracts

- Every native read during execution observes the effects of the preceding edit steps.
- New content receives new identity. Moving or joining existing content retains its text sources.
- One editor action publishes a complete, valid native transaction. Composition can publish progressive updates with one undo group. Intermediate normalization states remain private.
- A delayed operation addresses its read version. Competing structural changes fail atomically when they invalidate its intent.
- Native history determines cache invalidation. Rendering projections cannot supply document identity or mutation history.
- Ordinary typing stays on the local text path. Existing Chrome limits remain acceptance checks: p95 key-to-frame at most 20 ms and maximum frame gap at most 50 ms.

## Unresolved questions

None requiring user input. The user authorized the boundary change and the existing editor behavior and tests define the compatibility target.
