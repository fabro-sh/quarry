# Development Guide

## Workspace layout

- `quarry-document`: Automerge schema, identity, review targets and commands.
- `quarry-document-wasm`: the shared engine's browser boundary.
- `quarry-markdown`: Markdown parsing, export, review import and reconciliation.
- `quarry-core`: domain types, errors, metadata and paths.
- `quarry-cas`: content-addressed blobs and reachability GC.
- `quarry-storage`: atomic native publication, indexes, versions, receipts, archives and upgrades.
- `quarry-server`: HTTP, events, scope checks, command authority and agent operation translation.
- `quarry-git`, `quarry-cli`, `quarry-fuse`: external writers and adapters.
- `quarry`: the application binary.
- `ui/`: React workspace, Plate/Slate input adapter and durable browser outbox.

## Feature flags

The default build enables `tmp-documents` (scratch documents with capability-URL sharing plus the embedded browser workspace). The library document surface — path-addressed libraries, `put`/`get`/`list`/`move`/`delete`, explicit transactions, Git sync, FUSE mounts — is gated behind `lib-documents`:

```sh
cargo build --release -p quarry --features lib-documents
```

Library-surface quickstart (requires `lib-documents`):

```sh
quarry server --root .quarry init
printf 'hello\n' > /tmp/hello.md
quarry put notes notes/hello.md /tmp/hello.md
quarry get notes notes/hello.md
quarry server --root .quarry start --addr 127.0.0.1:7831
```

The server binds to `127.0.0.1` by default. Non-loopback binds print a warning because phase one intentionally has no auth.

### Trusted tmp-document creation addresses

Local servers do not trust forwarding headers and store no creation address by
default. A deployment behind CloudFront can opt into the CloudFront-generated
viewer address for anonymous tmp-document creation:

```sh
QUARRY_CLIENT_IP_SOURCE=cloudfront-viewer-address quarry server start
# equivalent: quarry server start --client-ip-source cloudfront-viewer-address
```

In this mode `POST /v1/tmp/documents` requires exactly one valid
`CloudFront-Viewer-Address` value in `IP:port` form. The server stores the
canonical IP without the source port and rejects creation if the trusted header
is missing or malformed. Do not enable this mode on a directly reachable server
or substitute `X-Forwarded-For`; the deployment must arrange for CloudFront to
generate the trusted header.

## Verification

Install the Rust target `wasm32-unknown-unknown` and the pinned
`wasm-bindgen-cli` version from `.github/actions/setup-document/action.yml`.
Use `bun install --frozen-lockfile` in `ui/`. The UI build scripts compile the
same Rust engine used by the server; generated WASM is not committed.

From the repository root:

```sh
cargo fmt --all --check
cargo test --locked --workspace --all-features
cargo test --locked -p quarry-server --no-default-features --features tmp-documents
cargo test --locked -p quarry-server --no-default-features --features lib-documents
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

From `ui/`:

```sh
bun run check:architecture
bun run typecheck
bun run test
bun run build
bunx playwright install chromium firefox webkit
bun run test:e2e:live
bun run test:performance
```

The native tests cover exact character targets, concurrent edits, Unicode,
review decisions, ownership transfer, undo, schema rejection, rollback, receipts
and restart. The Markdown corpus lives in `fixtures/markdown`. Storage and HTTP
tests exercise creation, upgrade, all writers, scope, permissions and archives.
Browser tests run against a real server. WASM editor tests also replay their
requests in a native Rust process and compare the resulting state.

Performance tests build the production UI, release server and release WASM. The Chrome gate
types 100 keys at the start, middle and end of a 114,814-byte document with
1,401 blocks. It requires p95
keydown-to-next-frame time at or below 20 ms, no frame gap above 50 ms, and
exact text after saving and reload. A second 100 KiB formatted fixture runs
in all three browser engines. Run these tests without competing builds or
benchmarks. They use Playwright's pinned browsers and the embedded production
UI; they do not measure a physical display or every Chrome version. Trace
snapshots are disabled in performance tests because their full DOM scans can
block frames during typing. Functional browser tests retain failure traces.
The concurrent Chrome cases type 400 characters at the start or end while an
agent edits and comments on the large document. They use the same input and
frame-gap gates.

Set `QUARRY_SYSTEM_CHROME=1` to run the Chromium project with the installed
Google Chrome instead of Playwright's pinned Chromium. No browser is installed
by that option. Record the browser version with the benchmark results.

Set `QUARRY_PRODUCTION_UI=1` when running the live browser suite to use the
embedded production UI. Build `ui/` first. Without this flag, the live suite
uses Vite and React development mode. Use that mode for native call profiling
with `QUARRY_PROFILE=1`; `QUARRY_CPU_PROFILE=/absolute/path/profile.json` can
capture a Chrome CPU profile in either mode. Set
`QUARRY_RENDER_PROFILE=/absolute/path/frames` to collect long animation frames
without CPU profiling. Profile filenames include the test ID and repetition
so a benchmark matrix keeps every recording. JSON attachments retain frame
gaps and key latency samples in their original order.

Set `CARGO_TARGET_DIR` to use an isolated build directory. The UI scripts honor
it. Run native feature builds sequentially when they share a target directory.
The API description is served at `/v1/openapi.json`; `bun run generate:api`
updates the browser's checked-in API description from a running server.

## Releases

Release versions, commits, and annotated tags are created by the private `quarry-dev` crate:

```sh
cargo dev release --dry-run --nightly
cargo dev release --bump patch
cargo dev release --bump minor
```

Stable releases must be cut locally from a clean, synchronized `main`. The nightly workflow runs the same command with a GitHub App token. Its `nightly` environment needs `FABRO_RELEASES_APP_CLIENT_ID` and `FABRO_RELEASES_APP_PRIVATE_KEY`, and the app installation needs repository Contents write access. Nightlies use the next stable line, for example `0.1.4-nightly.20260711` after `v0.1.3`; stable releases support bump selection only. Pushing the resulting `v*` tag starts `.github/workflows/release.yml`.

The release command runs the Rust and browser smoke before changing the repository. Use `--skip-tests` only after running those checks yourself. `--dry-run` does not fetch or mutate; fetch tags first when checking a local release plan. The final branch/tag push is atomic. If a failure leaves a local release commit and tag, inspect them and retry that atomic push rather than rerunning the release command. Never delete and reuse a release tag—cut the next version instead.
