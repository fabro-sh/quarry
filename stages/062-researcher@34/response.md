## Review summary

I read `crates/quarry-cas/src/lib.rs` in full (the entire crate — 183 lines) and traced every caller across `quarry-storage`, `quarry-server`, and `quarry-cli` through the injection/input-handling lens. Each candidate sink resolved as follows:

- **Path traversal via `read`/`exists`/`object_path` (lib.rs:111/120/62):** Every hash string passes `Blake3Hash::from_str` (lib.rs:29-37), which rejects anything that is not exactly 64 ASCII hex chars and lowercases the result — no `/`, `..`, or NUL can survive into `object_path_for_hash` (lib.rs:66-71). Additionally, all production hash strings reaching `cas.read` come from `document_versions.content_hash`, which is written only from `cas.put`'s own server-computed BLAKE3 output (`insert_version_conn`, quarry-storage/src/lib.rs:688-697), so no attacker-controlled hash reaches the parse at all.
- **`put` (lib.rs:73-109):** Writes attacker bytes, but to a path derived from the server-computed hash of those exact bytes — the attacker controls content, not location. `NamedTempFile::new_in(parent)` + `persist` (rename) does not follow a planted symlink at the target. The line-77 dedup trust ("exists ⇒ correct content") is exploitable only with pre-existing write access to the CAS directory, which is outside any remote attack path.
- **`gc` (lib.rs:125-156):** The reachable set is derived from DB `content_hash` values (server-generated). `file_type()` does not follow symlinks, so only real dirs/files under `objects/` are touched. `to_string_lossy` name reconstruction can only mis-retain a foreign file (collision keeps it), not delete a legitimate blob — legitimate files reconstruct their exact hash. Foreign-file deletion presupposes a shared CAS root, contradicting the deployment invariant and not attacker-reachable.
- **Log/format injection:** The only attacker-reflecting string (`invalid BLAKE3 hash {value}`) is unreachable with attacker input, since hashes come from the DB.
- No SQL, shell, regex, template, XXE, or untrusted deserialization exists in this component (`BlobInfo` is never deserialized from external input).

No complete attacker-controlled source-to-sink path exists in this component under this lens.

```json
{
  "findings": []
}
```