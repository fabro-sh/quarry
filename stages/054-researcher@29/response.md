I've completed the review of the `cli-and-binary` component under the injection/input-handling lens. I read the hot files in full (`crates/quarry-cli/src/lib.rs`, `crates/quarry-cli/src/detect_agent.rs`, `crates/quarry/src/main.rs`) and traced every candidate sink backward:

- **`server restore` `remove_dir_all` (lib.rs:485)** — root comes only from operator argv/`QUARRY_ROOT`; deleting the resolved root is the command's documented function, and env control is same-trust-domain. No attacker source.
- **`copy_dir` symlink following (lib.rs:1028-1044)** — exploitation requires pre-existing write access to the operator's store root/backup; no privilege boundary crossed.
- **Server response → URL/stdout/browser (lib.rs:940-994)** — the server is operator-chosen over TLS; the interpolated `secret` can only redirect requests to the already-trusted host, and both stdout variants carry the same capability URL, so `detect_agent` env spoofing grants nothing extra.
- **`put`/`share`/`move`/`git` argv flows** — verified downstream guards hold: `validate_slug` rejects `/`, `\`, whitespace, `.`, `..` (libraries.rs:91-99), `normalize_collab_invite_role` and `normalize_path` sanitize share inputs (documents.rs:280-281), all SQL is parameterized, and git operations use git2 refspecs with no shell-out anywhere in the flow.
- **RUST_LOG/QUARRY_LOG_FORMAT** — filter is validated via `EnvFilter::try_new` with fallback (lib.rs:63-67).

No complete path from a real untrusted source to a dangerous operation without an effective defense was found.

```json
{
  "findings": []
}
```