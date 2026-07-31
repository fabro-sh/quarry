{
  "job_id": "research:001-http-server-71604076:injection-and-input:1",
  "findings": [
    {
      "category": "path-traversal",
      "title": "Unauthenticated git import reads arbitrary local files into a library",
      "file": "crates/quarry-server/src/git_handlers.rs",
      "line": 104,
      "symbol": "git_import",
      "ruleId": "path-traversal.git-repo-path",
      "identity": {
        "anchor": "git-repo-path-confinement",
        "instance": "import"
      },
      "severity": "HIGH",
      "difficulty": "MEDIUM",
      "confidence": "HIGH",
      "impact": "An unauthenticated HTTP caller makes the server recursively read every regular file under an arbitrary caller-chosen filesystem path (e.g. /etc, $HOME, ~/.ssh, cloud-credential directories) and commit the contents as documents into a named library, then reads those contents back through the unauthenticated document API. Any file readable by the server process is exfiltrated.",
      "rationale": "The handler passes request.repo verbatim into quarry-git's import path; the only check is that the directory exists. The recursive fs::read, the library commit, and the unauthenticated read-back endpoint were each verified in source, so the source-to-sink path is complete with no effective defense. Confidence is high from static tracing; per the read-only rule the exploit was not executed. Severity is HIGH because any file the server user can read crosses a trust boundary into an unauthenticated API; difficulty is MEDIUM because exploitation requires the non-default lib-documents build and reaching the listener (local process, non-loopback bind, or DNS rebinding). The repo's own threat-model doc records this endpoint family as a known accepted risk of the lib-documents build, but no code-level defense exists.",
      "snippet": "        import_worktree(&state.store, &library, std::path::Path::new(&request.repo)).await?,",
      "evidence": [
        "crates/quarry-server/src/git_handlers.rs:104 — git_import converts the attacker-controlled JSON field `request.repo` (GitImportRequest, git_handlers.rs:86-89) into a filesystem path with `std::path::Path::new` and hands it to import_worktree with no validation, allowlist, or canonicalization in the handler.",
        "crates/quarry-server/src/lib.rs:460-463 — the route POST /v1/libraries/{library}/git/import is installed with no authentication or authorization middleware; the only exposure control in the entire server is warn_if_non_loopback (lib.rs:692-706), which prints a warning and serves anyway.",
        "crates/quarry-git/src/lib.rs:830 and 853-870 (data flow crosses from quarry-server into quarry-git here) — the only check on the path is ensure_worktree_exists, which verifies the directory exists; there is no confinement to any root.",
        "crates/quarry-git/src/lib.rs:881-903 — scan_worktree_import_files walks repo_dir recursively with WalkDir (skipping only .git/.quarry entries and sidecars) and executes `let bytes = fs::read(entry.path())?;` on every regular file under the attacker-chosen path.",
        "crates/quarry-git/src/lib.rs:842-844 — the read files are passed to import_worktree_transaction, committing each file's bytes as a document in the caller-named library (the library itself can be created unauthenticated via POST /v1/libraries, library_handlers.rs:20-28).",
        "crates/quarry-server/src/document_handlers.rs:308 and 457 — get_document serves the imported document contents back over unauthenticated GET /v1/libraries/{library}/documents/{*path}, completing the exfiltration loop. Guard checked: normalize_path is applied only to the relative path inside the worktree (quarry-git lib.rs:899), never to repo_dir itself, so it is ineffective against the traversal."
      ],
      "exploitScenarios": [
        "Attacker identifies a quarry server built with the lib-documents feature whose HTTP port they can reach (local process on the loopback default 127.0.0.1:7831, a non-loopback bind, or a drive-by DNS-rebinding page in the operator's browser — no Host or Origin check exists).",
        "POST /v1/libraries with {\"slug\":\"loot\"} to create a library (or reuse any existing one).",
        "POST /v1/libraries/loot/git/import with body {\"repo\":\"/home/victim/.ssh\"} (repeat for /etc, $HOME, cloud-credential paths).",
        "The server recursively reads every file under that path and commits each as a library document; the response lists the imported paths.",
        "GET /v1/libraries/loot/documents/{path} for each imported path to retrieve the file contents (private keys, credentials, configs)."
      ],
      "preconditions": [
        "The server binary is built with the non-default `lib-documents` cargo feature (the default tmp-documents build and the shipped Docker image omit the git routes).",
        "The attacker can reach the HTTP listener: any local process under the loopback default, any network peer when bound to a non-loopback address (warn-only), or a remote web attacker via DNS rebinding since the server performs no Host/Origin validation and no authentication.",
        "Target files are readable by the user running the quarry process."
      ],
      "recommendations": [
        "Root cause: stop taking filesystem paths from the request. Resolve import sources server-side (e.g. only from a configured, fixed worktree root) and canonicalize+verify any path stays under that root before any filesystem access; reject absolute paths and `..` from request input.",
        "Hardening: put authentication/authorization on the whole /v1/libraries/** namespace and fail closed (refuse to bind) on non-loopback addresses without auth instead of only warning; validate Host/Origin to blunt DNS-rebinding drive-bys.",
        "Regression test: POST /v1/libraries/{lib}/git/import with repo values /etc, $HOME, and ../../ escape attempts must return a 4xx and perform no filesystem reads; add an equivalent test proving imports only succeed under the configured root."
      ]
    },
    {
      "category": "path-traversal",
      "title": "Unauthenticated git export wipes and writes arbitrary directories",
      "file": "crates/quarry-server/src/git_handlers.rs",
      "line": 132,
      "symbol": "git_export",
      "ruleId": "path-traversal.git-repo-path",
      "identity": {
        "anchor": "git-repo-path-confinement",
        "instance": "export"
      },
      "severity": "HIGH",
      "difficulty": "MEDIUM",
      "confidence": "HIGH",
      "impact": "An unauthenticated HTTP caller points export at an arbitrary directory; the server recursively deletes everything in it except .git, then writes attacker-controlled file contents into it and git-inits/commits. Pointed at a home directory this is mass data destruction; combined with the unauthenticated document-write API it plants controlled files (e.g. ~/.ssh/authorized_keys), yielding host-level code execution as the server user.",
      "rationale": "The handler passes request.repo verbatim into quarry-git's export path, which creates the directory, deletes every entry except .git, and writes library document contents (themselves attacker-controlled via the unauthenticated document PUT). The sole guard, verify_or_write_marker, fires only when the directory was previously exported from a different library, so it is not an effective defense. Confidence is high from static tracing; per the read-only rule the exploit was not executed. Severity is HIGH because arbitrary directory deletion plus controlled file placement escalates to host compromise; difficulty is MEDIUM because exploitation requires the non-default lib-documents build and reaching the listener. The repo's own threat-model doc records this endpoint family as a known accepted risk of the lib-documents build, but no code-level defense exists.",
      "snippet": "            std::path::Path::new(&request.repo),",
      "evidence": [
        "crates/quarry-server/src/git_handlers.rs:123-137 — git_export converts the attacker-controlled JSON field `request.repo` (GitExportRequest, git_handlers.rs:108-114) into a filesystem path at line 132 and passes it to export_worktree with no validation, allowlist, or canonicalization in the handler.",
        "crates/quarry-server/src/lib.rs:464-467 — POST /v1/libraries/{library}/git/export is installed with no authentication or authorization middleware; warn_if_non_loopback (lib.rs:692-706) only logs when the listener is non-loopback.",
        "crates/quarry-git/src/lib.rs:1030 (data flow crosses from quarry-server into quarry-git here) — execute_worktree_export runs `fs::create_dir_all(&plan.repo_dir)?`, creating the attacker-chosen directory tree if absent.",
        "crates/quarry-git/src/lib.rs:1031 and 1195-1209 — the only guard, verify_or_write_marker, is ineffective: it errors only when a .quarry/marker.json already present names a different library; for any other directory (the common case) it silently writes the marker and proceeds.",
        "crates/quarry-git/src/lib.rs:1032 and 1254-1267 — clean_worktree then iterates the directory and deletes every entry except .git (`fs::remove_dir_all(path)?` for subdirectories, `fs::remove_file(path)?` for files), wiping the attacker-chosen directory's contents.",
        "crates/quarry-git/src/lib.rs:1036-1046 — the server then writes each library document to `plan.repo_dir.join(&file.path)` via write_atomic; document paths and contents are attacker-controlled through the unauthenticated PUT /v1/libraries/{library}/documents/{*path} (document_handlers.rs:509-576), and commit_all (lib.rs:1271-1274) even initializes a git repo at the path."
      ],
      "exploitScenarios": [
        "Attacker reaches the unauthenticated HTTP port of a lib-documents build (local process, non-loopback bind, or DNS-rebinding drive-by; no Host/Origin check exists).",
        "For destruction: POST /v1/libraries/{lib}/git/export with {\"repo\":\"/home/victim\"} — the server recursively deletes every file and subdirectory (except .git) in that directory, then writes library documents there.",
        "For code execution: first PUT /v1/libraries/{lib}/documents/authorized_keys with an attacker public key as the body (unauthenticated write).",
        "Then POST /v1/libraries/{lib}/git/export with {\"repo\":\"/home/victim/.ssh\"} — clean_worktree wipes .ssh and write_atomic plants the attacker-controlled authorized_keys.",
        "The attacker logs into the host over SSH as the user running quarry."
      ],
      "preconditions": [
        "The server binary is built with the non-default `lib-documents` cargo feature (default tmp-documents builds and the shipped image omit the git routes).",
        "The attacker can reach the HTTP listener (loopback local process, non-loopback bind, or DNS rebinding; no authentication or Host/Origin validation exists).",
        "Target directories are writable/deletable by the user running the quarry process.",
        "The chosen target directory does not already contain a .quarry/marker.json registered to a different library (the only guard, and it is absent in the common case)."
      ],
      "recommendations": [
        "Root cause: stop taking filesystem paths from the request. Export only to server-side-managed destinations under a fixed configured root; canonicalize and verify containment before any filesystem operation, and refuse absolute paths and `..` from request input.",
        "Hardening: require authentication/authorization on /v1/libraries/** and refuse non-loopback binds without auth (fail closed instead of warn-only); additionally require export targets to be empty or already marker-registered to the same library before running clean_worktree, and never delete as a side effect of export.",
        "Regression test: POST /v1/libraries/{lib}/git/export with repo values outside the configured root (absolute paths, .. escapes) must return a 4xx with zero filesystem effects; a test must prove clean_worktree never runs on a directory that was not explicitly registered."
      ]
    }
  ]
}