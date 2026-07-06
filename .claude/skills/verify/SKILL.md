---
name: verify
description: Build, launch, and drive the Quarry server + embedded browser UI for end-to-end verification of a change.
---

# Verifying Quarry end-to-end

## Build + launch

```sh
cd ui && bun run build          # rebuild the browser bundle (server serves ui/dist)
cd .. && cargo run -q -p quarry -- init <scratch-dir>/.quarry
cargo run -q -p quarry -- serve --db <scratch-dir>/.quarry/quarry.db \
  --cas <scratch-dir>/.quarry/cas --addr 127.0.0.1:<port>
```

- Use a fresh scratch dir + fresh port. **Check `lsof -nP -iTCP:<port> -sTCP:LISTEN` first** — stale quarry servers from earlier sessions linger (5273 and 7831 are commonly taken) and will silently serve you their own state.
- Confirm you're talking to your build: `curl -s http://127.0.0.1:<port>/ | grep -o 'assets/index[^"]*'` and match the hash against `ui/dist/assets/`.

## Drive the UI (browser-use)

1. First load shows a "Welcome to Quarry" dialog — fill "Your name" and click "Get started" before anything else works.
2. Create a document: `⌘K` → "Create document". The URL becomes `/tmp/<secret>` — same shape as production tmp links.
3. To type into the editor you must **click into the document body first** (e.g. coordinates mid-page); keyboard input without that focus lands nowhere and fails silently.
4. `browser-use eval` does not await async IIFEs — kick off promises in one eval, read results in a second.

## Gotchas

- The production CSP (`crates/quarry-server/src/lib.rs`) is strict: no `data:` fonts, no third-party origins. `performance.getEntriesByType('resource')` filtered by `!u.startsWith(location.origin)` catches violations; `[...document.fonts].filter(f => f.status === 'error')` catches blocked fonts.
- Vite dev server (:5173) does not send the CSP — CSP bugs only reproduce against the real server.
