{
  "job_id": "threat:009-core-shared-bcbb015d",
  "component": "core-shared",
  "paths": ["crates/quarry-core"],
  "language": "rust",
  "summary": "quarry-core is a pure shared-types crate: serde data structures (Document, DocumentVersion, transactions, search/graph DTOs), string newtype identifiers, error enums, path normalization, and YAML frontmatter rendering. It contains no I/O, execution, networking, or unsafe code, but its normalize_path validator and render_markdown_frontmatter serializer are the canonical safety routines other crates rely on, so weaknesses here propagate to filesystem and Git adapters.",
  "entryPoints": [
    "crates/quarry-core/src/lib.rs:606 normalize_path — untrusted document paths from REST/FUSE/Git/CLI callers enter here; the only path-safety validation in the system",
    "crates/quarry-core/src/lib.rs:590 render_markdown_frontmatter — attacker-controlled metadata JsonValue serialized into YAML frontmatter embedded in exported markdown (shared by quarry-storage and quarry-git, must stay byte-identical for diff3)",
    "crates/quarry-core/src/lib.rs:200 DocumentSource::from_str (also 234, 268, 299) — parses persisted/transport enum strings, rejecting unknown values",
    "crates/quarry-core/src/lib.rs:16 string_newtype macro — DocumentId/VersionId/Timestamp accept any string via new/From with no format validation; they flow into SQL keys, git refs, and filesystem paths downstream",
    "crates/quarry-core/src/lib.rs:311 WritePrecondition — deserialized precondition (IfMatch value) controlling optimistic-concurrency checks on writes"
  ],
  "sinks": [
    "crates/quarry-core/src/lib.rs:602 serde_yaml::to_string — serializes untrusted metadata into YAML emitted inside document content; frontmatter breakout via `---`/document markers must be judged at the serializer level",
    "crates/quarry-core/src/lib.rs:606 normalize_path — security-relevant validator feeding FUSE filesystem paths, git working-tree writes, and SQLite keys; rejects `..`, `\\`, `.quarry/` but not NUL/control bytes, unicode tricks, or Windows reserved names",
    "crates/quarry-core/src/lib.rs:332 DocumentVersion.inline_content (#[serde(skip)]) — raw content bytes held outside serialization; consumers cannot rely on its presence after a JSON round-trip"
  ],
  "assumptions": [
    "crates/quarry-core/src/lib.rs:606 — all callers route every untrusted path through normalize_path before touching the filesystem or git tree; nothing in this crate enforces that",
    "crates/quarry-core/src/lib.rs:611 — reserved-name check matches only exact lowercase `.quarry`; assumes case-sensitive filesystem semantics and pre-normalized unicode elsewhere",
    "crates/quarry-core/src/lib.rs:590 — metadata keys/values are assumed safe to embed as YAML; callers assume serde_yaml output cannot break out of the `---`-delimited frontmatter block",
    "crates/quarry-core/src/lib.rs:24 — DocumentId/VersionId/Timestamp newtypes assume upstream validation (format, length, charset) that does not exist in this crate",
    "crates/quarry-core/src/lib.rs:579 — now_timestamp assumes chrono RFC3339 output is what storage/parsers round-trip; no monotonicity or authenticity guarantee"
  ],
  "trustBoundaries": [
    "crates/quarry-core/src/lib.rs:606 — external path strings (HTTP/FUSE/CLI/git) to validated canonical storage path",
    "crates/quarry-core/src/lib.rs:590 — untrusted metadata JsonValue to bytes embedded in exported document files consumed by git/diff3 reconciliation",
    "crates/quarry-core/src/lib.rs:165 — serialized DTOs (Library, Document, TransactionRecord, CollabInviteToken) cross API wire format, SQLite storage, and in-memory domain types; #[serde(default)] Option fields (lines 323-330, 374, 392) silently accept missing fields from older/foreign producers",
    "crates/quarry-core/src/lib.rs:399 — CollabInviteToken carries role/revocation state across the wire; authorization semantics are defined by consumers, not here"
  ],
  "hotFiles": [
    "crates/quarry-core/src/lib.rs — entire 724-line crate: sole source of the path validator (606-628), frontmatter renderer (590-604), identifier newtypes (16-97), and every DTO whose serde attributes control cross-boundary behavior"
  ],
  "notes": "No unsafe code, no I/O, no process execution in this component. Primary risk concentration: normalize_path coverage gaps (NUL/control bytes, unicode, Windows reserved names, case-insensitive .QUARRY) and YAML frontmatter breakout via metadata keys/values. Consumers in quarry-storage, quarry-git, quarry-fuse, and the REST layer should be checked for paths that bypass normalize_path."
}