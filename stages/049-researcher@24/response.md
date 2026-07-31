All matches are in `#[cfg(test)]` code. I've now read the full production surface of the component (`crates/quarry-cli/src/lib.rs`, `crates/quarry-cli/src/detect_agent.rs`, `crates/quarry/src/main.rs`) under the memory/unsafe lens:

- **No `unsafe` blocks, no FFI, no raw pointers, no transmute** anywhere in the component — confirmed by grep over both crates.
- **No integer arithmetic on untrusted values**: argv/env inputs (paths, library names, URLs, version ids) are used only as strings/paths; no size calculations, casts, or offset math feed allocations or indexing.
- **No unchecked indexing on untrusted data**: `created["document"]["path"]` (lib.rs:948) is `serde_json::Value` indexing, which returns `Value::Null` on a missing key rather than panicking, and the `.as_str()` + `ok_or_else` guard handles a malicious server response safely.
- **`copy_dir` recursion (lib.rs:1028–1044)**: recursion depth is bounded by the kernel's `PATH_MAX` (~1365 levels with minimal component names), far short of the 8 MiB main-thread stack; `DirEntry::file_type()` does not follow symlinks, so symlink cycles cannot cause unbounded recursion either. The source path is also operator-supplied, not attacker-controlled. (Its symlink-following copy behavior is a path-handling concern for a different lens, not a memory-safety issue.)
- **Whole-file reads** (`fs::read`/`read_to_string`, lib.rs:560, 711) read operator-chosen local files; an OOM there is self-inflicted, not an attacker action.

No complete attacker-controlled source-to-sink path exists in this component under this lens.

{
  "findings": []
}