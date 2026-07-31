{
  "job_id": "threat:007-cas-store-69c3f1c1",
  "component": "cas-store",
  "entryPoints": [
    "crates/quarry-cas/src/lib.rs:73 — DiskCas::put(&self, bytes: &[u8]) writes fully caller-controlled byte content into the store (called from crates/quarry-storage/src/lib.rs:688 with document content)",
    "crates/quarry-cas/src/lib.rs:111 — DiskCas::read(&self, hash: &str) takes a caller-supplied hash string; hashes originate from database records (crates/quarry-storage/src/versions.rs:193, lib.rs:615, lib.rs:987, blocks.rs:1087)",
    "crates/quarry-cas/src/lib.rs:120 — DiskCas::exists(&self, hash: &str) caller-supplied hash string",
    "crates/quarry-cas/src/lib.rs:62 — DiskCas::object_path(&self, hash: &str) caller-supplied hash string mapped to a filesystem path",
    "crates/quarry-cas/src/lib.rs:125 — DiskCas::gc(reachable_hashes) consumes an iterator of hash strings produced by storage-layer reachability queries (crates/quarry-storage/src/lib.rs:235)",
    "crates/quarry-cas/src/lib.rs:48 — DiskCas::open(root) takes a caller/config-supplied root directory and creates objects/ under it"
  ],
  "sinks": [
    "crates/quarry-cas/src/lib.rs:146 — fs::remove_file(object.path()) deletes every file under objects/ whose reconstructed hash is not in the reachable set; an incomplete reachable set causes irreversible blob loss",
    "crates/quarry-cas/src/lib.rs:117 — fs::read(path) reads an entire blob into memory with no size cap",
    "crates/quarry-cas/src/lib.rs:91 — tmp.write_all(bytes) writes untrusted bytes to a tempfile, persisted at lib.rs:93 via tmp.persist(&path)",
    "crates/quarry-cas/src/lib.rs:66-71 — object_path_for_hash builds the on-disk path via string slicing (&hash[0..2], &hash[2..]); safety depends entirely on Blake3Hash validation",
    "crates/quarry-cas/src/lib.rs:50,88 — fs::create_dir_all on root/objects and per-shard parent directories",
    "crates/quarry-cas/src/lib.rs:58-60 — blake3::hash over untrusted content is the sole integrity primitive; read() (lib.rs:111-118) never re-verifies content against the requested hash",
    "crates/quarry-cas/src/lib.rs:77,114,122 — path.exists() existence checks used as lookup logic (TOCTOU between exists/read/gc traversal)"
  ],
  "assumptions": [
    "crates/quarry-cas/src/lib.rs:29-37 — assumes Blake3Hash::from_str is the only path-check gate: exactly 64 lowercase-able hex chars, so object_path_for_hash cannot be tricked into traversal (no '/' or '..'); any caller bypassing FromStr would break this",
    "crates/quarry-cas/src/lib.rs:125-129 — gc assumes the storage layer supplied the complete, correct reachability set; the CAS itself has no way to detect a missing hash before deleting",
    "crates/quarry-cas/src/lib.rs:111-118 — read assumes on-disk blobs are unmodified since put (content-addressed integrity is assumed, not enforced; no re-hash on read)",
    "crates/quarry-cas/src/lib.rs:77-83 — put assumes a pre-existing path means identical content (hash-collision/immutability assumption; returns early without comparing bytes)",
    "crates/quarry-cas/src/lib.rs:48-52 — assumes the root path from configuration is trusted and not attacker-controlled or shared with hostile writers (no permission/symlink checks on root or shard dirs)",
    "crates/quarry-cas/src/lib.rs:138-144 — gc assumes shard directory names are exactly 2 hex chars; it reconstructs hashes from arbitrary on-disk names and deletes non-matching files, assuming nothing else places files under objects/"
  ],
  "trustBoundaries": [
    "crates/quarry-cas/src/lib.rs:73-109 — untrusted caller content crosses from memory to the trusted on-disk store (write boundary)",
    "crates/quarry-cas/src/lib.rs:111-118 — on-disk bytes cross back into the process as trusted blob content returned to callers, without integrity re-verification",
    "crates/quarry-cas/src/lib.rs:29-37,62-71 — database/caller-supplied hash strings (less trusted) are validated and mapped to filesystem paths (more trusted) — the single validation boundary",
    "crates/quarry-cas/src/lib.rs:129-149 — reachability data from the SQLite layer (crates/quarry-storage/src/lib.rs:235) crosses into destructive filesystem operations"
  ],
  "hotFiles": [
    "crates/quarry-cas/src/lib.rs",
    "crates/quarry-storage/src/lib.rs — sole production caller driving put/read/gc reachability data flows",
    "crates/quarry-storage/src/versions.rs",
    "crates/quarry-storage/src/blocks.rs",
    "crates/quarry-storage/src/store.rs"
  ]
}