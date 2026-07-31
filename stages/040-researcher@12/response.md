I've completed the memory/unsafe-lens review of the `web-ui` component. Summary of what I traced:

- **Binary deserialization** (`rust-ws-provider.ts:142`): the only custom binary path — `decoding.readVarUint8Array` on checkpoint frames — uses lib0's bounds-checked decoder (truncated/oversized length prefixes throw, they don't read out of bounds), and the resulting bytes flow only into `Y.decodeSnapshot` inside a `try/catch` (`save-state.ts:32`). No memory-unsafe operation; a forged frame could at worst spoof a save-state label, which is an integrity concern outside this lens and unverified (depends on the server relaying arbitrary message types).
- **Numeric handling**: all `slice`/index arithmetic (`table.ts`, `markdown-codec.ts`, `App.tsx`) derives bounds from actual array lengths; `Array.from({ length: width })` in `table.ts:166` uses a width computed from real row lengths, not attacker-supplied numbers. The `>>> 0` hash in `PlateMarkdownEditor.tsx:1809` is deliberate uint32 wraparound for a local key hash.
- **Allocation/DoS-adjacent**: no `new Array(n)`, `.repeat(n)`, or typed-array sizing driven by untrusted input anywhere in `ui/src`.
- **FFI/WASM**: the entire dependency tree (React, PlateJS, Yjs, lib0, mermaid, comlink) is pure JavaScript — no WASM, no native bindings, no `unsafe` blocks exist in TypeScript. The worker (`mirror-serializer.worker.ts`) runs structured-clone data through Comlink, no shared memory.
- **Other hot files** (`workspace-event-stream.ts`, `markdown-codec.ts`, `image.ts`, `RemoteCursorOverlay.tsx`) operate on JSON/strings via memory-safe JS APIs.

There is no complete source-to-sink path for any memory-safety violation in this pure-TypeScript component.

```json
{
  "job_id": "research:002-web-ui-40ce0b0c:memory-and-unsafe:1",
  "component": "web-ui",
  "lens": "memory and unsafe operations",
  "findings": [],
  "notes": "Component is pure TypeScript/React with no WASM, FFI, or native bindings. All binary decoding (lib0 readVarUint8Array on collab checkpoint frames, rust-ws-provider.ts:142) uses bounds-checked pure-JS decoders and the decoded snapshot is consumed inside try/catch (save-state.ts:32). All index/length arithmetic derives from actual array bounds; no attacker-controlled allocation sizes, no manual buffer manipulation, no SharedArrayBuffer. No memory-safety attack surface exists to exploit."
}
```