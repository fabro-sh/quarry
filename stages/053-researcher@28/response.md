The sweep is complete. Summary of what I verified across `crates/quarry-storage` (all 15 files read in full, blocks.rs/links.rs via a read-only explorer, guards traced into quarry-core and quarry-cas):

- **SQL injection**: Every query uses bound parameters. The six `format!`-built SELECTs in `lib.rs` interpolate only internal constant `scope_filter` fragments with all values bound. `schema.rs:154/189` PRAGMAs quote via `quote_sql_string`, and their inputs are internal constants or index names read from the database itself — not request data.
- **LIKE wildcard injection** (`documents.rs:397`, `directories.rs:150/172/285`, `lib.rs:1737`): confirmed `normalize_path` (quarry-core:606–628) does not reject `%`/`_` and no `ESCAPE` clause exists. However, every one of these queries is pinned to a single `library_id`, and the server layer's stated trust model is trusted-localhost with no path/prefix-level authorization (`discovery.rs:273`: library REST "trust[s] localhost and do[es] not currently enforce bearer-token auth"; collab tokens are per-document, not per-prefix). A caller who can supply a wildcard prefix already holds full read/move rights over that library — no privilege boundary is crossed, so this is not a reportable finding.
- **Path traversal**: CAS reads go through `Blake3Hash` parse (exactly 64 hex chars) before any path join, and the hash is computed by the crate, never caller-supplied; store/db/lock paths come from local config.
- **Insecure deserialization**: `serde_json::from_str` sites parse JSON written by this crate; parsed values are re-rendered or returned, never fed to paths, SQL, or commands. Memory-safe.
- **ReDoS**: search uses `str::contains`/`find`, no regex; snippet slicing is char-boundary guarded.
- **Tmp capability surface**: secret parse requires exactly 32 lowercase hex chars; promote/fork strictly require the capability, which is the designed bearer-token authorization.
- **Prompt injection**: no prompt construction exists in this component; the document-content → agent-prompt sink lives in `quarry-server`, outside this component, and no unguarded storage-side sink feeds it.

No complete source-to-sink path from an untrusted source to a dangerous operation without an effective defense was found.

```json
{
  "findings": []
}
```