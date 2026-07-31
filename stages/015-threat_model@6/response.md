{
  "component": "storage-layer",
  "job_id": "threat:006-storage-layer-6b547c78",
  "entryPoints": [
    "crates/quarry-storage/src/documents.rs:25 put_document(PutDocumentRequest): caller-supplied library slug, path, raw content bytes, metadata JSON, content_type, actor/message",
    "crates/quarry-storage/src/documents.rs:73 commit_document_without_events_with_transaction: untrusted content/metadata/content_type/transaction provenance",
    "crates/quarry-storage/src/documents.rs:189 get_document(library, path): caller path normalized then used in SQL",
    "crates/quarry-storage/src/documents.rs:367 list_documents(library, prefix, limit): caller prefix normalized then used in a LIKE pattern",
    "crates/quarry-storage/src/documents.rs:273 create_collab_invite_token(library, path, role, by_hint)",
    "crates/quarry-storage/src/documents.rs:503 move_document(from_path, to_path)",
    "crates/quarry-storage/src/tmp_documents.rs:58 put_tmp_document: tmp capability secret embedded in path; anonymous/unauthenticated write surface",
    "crates/quarry-storage/src/tmp_documents.rs:93 create_tmp_document_with_creation_ip: trusts server layer to supply real peer IP",
    "crates/quarry-storage/src/tmp_documents.rs:418 fork_tmp_document and promote_tmp_document (line 510): capability-secret path crossing tmp to library scope",
    "crates/quarry-storage/src/search.rs:6 search_documents(library, query, limit): reads every document body for substring search",
    "crates/quarry-storage/src/sync.rs:27 create_git_peer(library, config JsonValue): arbitrary JSON persisted as peer config",
    "crates/quarry-storage/src/sync.rs:113 upsert_sync_state(peer_id, path, git oids)",
    "crates/quarry-storage/src/blocks.rs:757 block review-item insert: caller block_id, offsets, body, replacement text",
    "crates/quarry-storage/src/blocks.rs:843 shadow-base upsert with caller surface/scope_key/base_markdown",
    "crates/quarry-storage/src/libraries.rs:7 create_library(slug)",
    "crates/quarry-storage/src/directories.rs:132 move_directory(from_path, to_path): caller paths drive subtree-wide LIKE renames"
  ],
  "sinks": [
    "crates/quarry-storage/src/lib.rs:476 format!-built SELECT interpolating scope_filter (internal constants only; values bound); same pattern at 799, 844, 941, 1413, 1510",
    "crates/quarry-storage/src/schema.rs:154 format! into PRAGMA table_info/index_info with hand-rolled quote_sql_string escaping (also 189, 201)",
    "crates/quarry-storage/src/documents.rs:397 SQL LIKE pattern format!(\"{prefix}%\") from caller prefix, no %/_ escaping, no ESCAPE clause",
    "crates/quarry-storage/src/lib.rs:1737 inodes LIKE '{from_path}/%' from caller move path, no wildcard escaping",
    "crates/quarry-storage/src/directories.rs:155 LIKE '{from_prefix}%' with unescaped caller path; also 177 and 285 (list_directories)",
    "crates/quarry-storage/src/lib.rs:987 CAS read keyed by content_hash stored in DB (document_from_row); same at versions.rs:193 and blocks.rs:1087",
    "crates/quarry-storage/src/row.rs:13 serde_json::from_str on DB-stored JSON (settings, metadata, provenance, config); also rows 35, 87, 96, 109 and sync.rs:72 peer config_json",
    "crates/quarry-storage/src/blocks.rs:1580 serde_json::from_str on stored blocks.marks/attrs (1586), metadata_json (1111, 1352), block_transactions.ops (2028)",
    "crates/quarry-storage/src/store.rs:110 acquire_lock: flock on lock file beside db_path; DB opened at configured path (line 112)",
    "crates/quarry-storage/src/store.rs:255 raw BEGIN IMMEDIATE / COMMIT / ROLLBACK execution with busy retry loop",
    "crates/quarry-storage/src/tmp_documents.rs:39 capability secret from Uuid::new_v4().simple(): 128-bit token is sole authorization for tmp documents",
    "crates/quarry-storage/src/search.rs:46 per-entry get_document inside search loop; one query triggers O(N bodies) reads; snippet slicing at 190-201",
    "crates/quarry-storage/src/documents.rs:298 INSERT collab_invite_tokens: role validated, token id is plain UUIDv4 returned to caller"
  ],
  "assumptions": [
    "crates/quarry-storage/src/documents.rs:397 assumes normalized paths contain no LIKE wildcards (%/_); normalize_path (quarry-core) not verified to reject them; affects lib.rs:1737, directories.rs:155/177/285",
    "crates/quarry-storage/src/tmp_documents.rs:99 assumes created_ip_address is a trusted edge-derived value supplied by the server layer, not the client",
    "crates/quarry-storage/src/documents.rs:25 assumes caller (server) already authorized the write; storage layer performs no authz beyond library existence",
    "crates/quarry-storage/src/documents.rs:110 assumes transaction.actor/message/provenance are trustworthy labels; stored verbatim and replayed in history",
    "crates/quarry-storage/src/row.rs:13 assumes JSON columns were written by this crate and are well-formed; parse failure is an error, not a tamper signal",
    "crates/quarry-storage/src/lib.rs:985 assumes inline_content/content_hash XOR invariant holds and content_hash resolves inside the CAS root (DiskCas traversal resistance not verified here)",
    "crates/quarry-storage/src/store.rs:112 assumes db_path/cas_path/lock_path come from trusted local configuration, not request data",
    "crates/quarry-storage/src/sync.rs:36 assumes GitPeer config JSON is sanitized before use by the git-sync consumer; stored unchecked",
    "crates/quarry-storage/src/blocks.rs:1601 assumes stored anchor offsets are consistent with document content; only u32 range is checked",
    "crates/quarry-storage/src/libraries.rs:91 assumes validate_slug is the only gate needed before a slug becomes a path component / SQL key elsewhere"
  ],
  "trustBoundaries": [
    "crates/quarry-storage/src/tmp_documents.rs:510 promote_tmp_document: anonymous tmp-scope data (secret-addressed) becomes trusted library-scope document",
    "crates/quarry-storage/src/tmp_documents.rs:438 fork_tmp_document: possession of a 32-hex-char path grants full read/copy; capability boundary with no other authz",
    "crates/quarry-storage/src/documents.rs:100 write_transaction boundary: all request data crosses into BEGIN IMMEDIATE write tx on the shared SQLite file",
    "crates/quarry-storage/src/lib.rs:987 DB-to-filesystem: stored content_hash crosses into DiskCas file read",
    "crates/quarry-storage/src/store.rs:144 subscribe_events: internal store events (paths, ids, origin metadata) broadcast to any subscriber",
    "crates/quarry-storage/src/sync.rs:18 git-sync events and sync_peers config carry git-remote-derived data into store and event bus"
  ],
  "hotFiles": [
    "crates/quarry-storage/src/lib.rs core query builders (format! SQL with scope fragments), document_from_row CAS read, inode LIKE moves, migration entry",
    "crates/quarry-storage/src/tmp_documents.rs anonymous capability-secret document lifecycle, IP recording, promote/fork boundary, size/UTF-8 validation",
    "crates/quarry-storage/src/documents.rs main document write/read/delete/move API, LIKE prefix listing, invite tokens",
    "crates/quarry-storage/src/directories.rs directory move/list LIKE patterns built from caller paths",
    "crates/quarry-storage/src/schema.rs migrations with execute_batch and hand-rolled SQL identifier quoting",
    "crates/quarry-storage/src/blocks.rs largest file: block state, review items, shadow bases, stored-JSON deserialization, anchor offset math",
    "crates/quarry-storage/src/store.rs locking, connection/transaction lifecycle, file I/O on configured paths",
    "crates/quarry-storage/src/row.rs central deserialization of every DB row into domain types"
  ]
}