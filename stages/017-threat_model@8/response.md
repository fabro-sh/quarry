{
  "component": "core",
  "job_id": "threat:008-core-0d45f5fd",
  "summary": "quarry-core is a single-module crate (crates/quarry-core/src/lib.rs, 724 lines) defining shared serde types, identifier newtypes, the QuarryError model, path normalization, and the canonical YAML frontmatter renderer consumed by quarry-storage and quarry-git. It contains no I/O, no unsafe code, and no execution; its risk surface is (a) normalize_path being the sole path-validation gate relied on by higher-trust consumers, and (b) serde Deserialize impls that accept untrusted JSON into shared wire/storage types with no field-level validation.",
  "entryPoints": [
    "crates/quarry-core/src/lib.rs:606 normalize_path — accepts an arbitrary &str document path from callers (REST handlers, FUSE, git adapter) and returns a normalized path; the single validation gate for all document paths in the system",
    "crates/quarry-core/src/lib.rs:590 render_markdown_frontmatter — takes document metadata (attacker-influenced JSON stored per document) and serializes it as YAML frontmatter embedded into exported markdown/git content",
    "crates/quarry-core/src/lib.rs:21 string_newtype Deserialize (DocumentId, VersionId, Timestamp) — transparent serde newtypes accept any untrusted string with no validation; these IDs are later used in SQL queries, file paths, and git references by downstream crates",
    "crates/quarry-core/src/lib.rs:317 derived Deserialize on wire structs (DocumentVersion, Document, TransactionRecord, GitPeer.config, Library.settings) — admits arbitrary untrusted JSON including opaque JsonValue blobs and unbounded content vectors with no shape or length validation",
    "crates/quarry-core/src/lib.rs:197 FromStr impls (DocumentSource, TransactionState, ChangeType, ConflictStatus) — parse untrusted strings from storage rows or query params; attacker-controlled value is echoed into ParseEnumError messages"
  ],
  "sinks": [
    "crates/quarry-core/src/lib.rs:602 serde_yaml::to_string — serializes attacker-influenced metadata into YAML that flows into git working trees, block-document export, and diff3 bases; YAML injection or frontmatter confusion propagates into files and commits",
    "crates/quarry-core/src/lib.rs:603 format! frontmatter assembly — interpolates rendered YAML between '---' delimiters into a document body; newline/delimiter smuggling in keys or values would corrupt document structure parsed elsewhere",
    "crates/quarry-core/src/lib.rs:100 QuarryError Display messages — error variants embed untrusted strings (paths, enum values) that callers typically surface in HTTP/FUSE error responses, enabling error-message reflection",
    "crates/quarry-core/src/lib.rs:580 chrono::Utc::now / to_rfc3339_opts — timestamp generation used across crates; all integrity and audit records depend on this wall clock"
  ],
  "assumptions": [
    "crates/quarry-core/src/lib.rs:606 normalize_path assumes every caller routes all untrusted paths through it before use; it rejects '.quarry' only at the path root after trimming slashes and assumes no downstream consumer interprets trailing components like 'foo/.quarry' or case/Unicode variants differently",
    "crates/quarry-core/src/lib.rs:606 normalize_path rejects backslashes and '..' but not NUL bytes, control characters, or reserved device names; it assumes downstream filesystem/git consumers treat the result as an opaque relative path",
    "crates/quarry-core/src/lib.rs:21 DocumentId/VersionId/Timestamp newtypes provide type safety only; they assume another layer validates ID syntax before these strings reach SQL statements, git refs, or filesystem paths",
    "crates/quarry-core/src/lib.rs:319 DocumentVersion.byte_size and content_hash are trusted as-is; assumes the storage layer computes them rather than accepting client-supplied values",
    "crates/quarry-core/src/lib.rs:590 render_markdown_frontmatter assumes serde_yaml output cannot itself contain a '---' document boundary or break out of the frontmatter block, and that metadata keys YAML-escape correctly",
    "crates/quarry-core/src/lib.rs:372 Document.content is an unbounded Vec<u8> in a deserialized wire type; assumes transport layers enforce size limits (INLINE_CONTENT_THRESHOLD at line 11 is advisory only)"
  ],
  "trustBoundaries": [
    "crates/quarry-core/src/lib.rs:606 untrusted path string -> validated document path — normalize_path is the boundary where raw caller input becomes a path the rest of the system (storage, git, FUSE) trusts",
    "crates/quarry-core/src/lib.rs:590 stored metadata JSON -> exported file content — attacker-writable metadata crosses into byte-exact markdown/git content shared between quarry-storage export and quarry-git; divergence breaks diff3 bases and injects content into repositories",
    "crates/quarry-core/src/lib.rs:171 wire JSON -> Library.settings / GitPeer.config / transaction_provenance JsonValue — opaque JsonValue fields carry unvalidated attacker JSON from the API boundary into consumers that interpret them as configuration",
    "crates/quarry-core/src/lib.rs:332 storage BLOB -> API response (inline_content serde skip) — the boundary relies on every serialization path honoring #[serde(skip)] so raw content bytes never cross into wire JSON"
  ],
  "hotFiles": [
    "crates/quarry-core/src/lib.rs"
  ]
}