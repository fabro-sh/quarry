## Summary

I audited the `web-ui` component (`ui/src`, `ui/public`) through the memory-and-unsafe-operations lens. The component is pure TypeScript/React running in the browser sandbox:

- **No WASM, FFI, `unsafe` blocks, SharedArrayBuffer, Atomics, or manual memory management** exist anywhere in the component (grep across `ui/` confirms; the single `unsafe` hit is a comment in `remark-inline-marks.ts` about remark rules).
- **The only binary decoding of remote bytes** is `decoding.readVarUint8Array(decoder)` at `ui/src/features/collab/rust-ws-provider.ts:142` on server-sent checkpoint frames, and `Y.decodeSnapshot` at `ui/src/features/collab/save-state.ts:32` — both lib0/Yjs are memory-safe pure-JS decoders; malformed input throws (the snapshot path is already wrapped in try/catch), so there is no out-of-bounds or corruption path, only a local exception.
- **Typed-array/Buffer-style arithmetic** appears only in `ui/src/features/editor/image.ts:29-30` (SHA-256 digest hex encoding of a local File — fixed 32-byte output, no attacker-controlled indexing or length math).
- **JSON type-confusion candidates** were checked and are guarded: `workspace-event-stream.ts:98-99` validates `payload.type` is a string before use, and all `localStorage` JSON.parse sites in `App.tsx` (`loadRecentLibraries` :4139, `loadTreeOpenState` :4163, right-pane tab :4184, theme :276) validate shape with `Array.isArray`/`typeof` filters before trusting parsed values. None of these reach a memory-unsafe operation regardless — JS has no raw memory to confuse.

No complete path from an untrusted source to a memory-corruption, out-of-bounds, use-after-free, integer-overflow, type-confusion, or unsafe-FFI operation exists in this component. The dangerous rendering sinks noted in the threat model (mermaid `dangerouslySetInnerHTML`, `window.open` on link hrefs) belong to the injection/XSS lens, not this one.

```json
{
  "findings": []
}
```