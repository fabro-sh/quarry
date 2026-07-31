{
  "rationale": "Reviewed crates/quarry-storage in full through the authentication/authorization lens, tracing every candidate sink back to its untrusted source. The tmp-document capability boundary is enforced consistently (all tmp queries filter document_scope='tmp', library_id IS NULL, expiry, and deletion; secrets are 122-bit UUIDv4), version_content_conn scopes version lookups by document_id so no tmp-version IDOR exists, all check-and-act sequences run inside a mutex-held BEGIN IMMEDIATE write transaction so no TOCTOU race exists, invite roles are restricted to viewer/editor, and normalize_path rejects traversal. One verified finding: promote_tmp_document treats possession of an anonymously self-mintable tmp secret as sufficient authorization to re-scope that document into any existing library, with no credential or permission bound to the target library; the promote endpoint is mounted on the tmp bearer-capability route family, so in the documented deployment where tmp URLs are shared beyond localhost while library APIs remain trusted-localhost, an anonymous attacker can inject attacker-controlled documents into any library at any unoccupied path. Confidence is MEDIUM because exploitability depends on that deployment posture; the missing authorization control itself is unconditional and fully traced.",
  "findings": [
    {
      "title": "Tmp document promotion into any library requires no library authorization (tmp bearer capability reused as a library-write credential)",
      "rationale": "promote_tmp_document is the single tmp-to-library trust-boundary crossing in the storage crate. It verifies only possession of the tmp secret — a capability anyone can self-mint anonymously via create_tmp_document — and an existence check on the target library, then executes an UPDATE that flips document_scope to 'library' with the caller-chosen library_id and path. No credential, permission, or identity is bound to the target library at any hop. The promote endpoint sits on the tmp bearer-capability route family (/v1/tmp/documents/{*path}) which the system's own discovery document advertises as reachable by anyone holding a tmp URL, while library REST APIs are documented as trusted-localhost; therefore, in the deployment the tmp model is designed for, an anonymous internet caller gains a library-write primitive that the trusted-localhost posture was meant to withhold. Guards checked and found ineffective: the compile-time feature flag (deployment switch, not authorization), the tmp secret parse (authorizes only the tmp document and is self-mintable), the caller-controlled tmp precondition (defaults to None), and the target-path conflict check (prevents overwrite, not unauthorized creation). Exploitability is deployment-dependent — a strict loopback-only deployment grants the same caller direct library writes anyway — so confidence is MEDIUM despite the fully traced path; severity is MEDIUM because the harm is create-only content injection with no read access and no overwrite.",
      "category": "authorization",
      "file": "crates/quarry-storage/src/tmp_documents.rs",
      "line": 543,
      "snippet": "                     document_scope = 'library',",
      "symbol": "promote_tmp_document",
      "ruleId": "improper-authorization.tmp-promote-scope-flip",
      "identity": {
        "anchor": "tmp-promote-library-scope-flip"
      },
      "severity": "MEDIUM",
      "difficulty": "MEDIUM",
      "confidence": "MEDIUM",
      "impact": "An anonymous attacker whose only credential is a tmp-document capability — which anyone can self-mint through the anonymous create endpoint — can permanently inject attacker-controlled Markdown documents (up to 1 MiB each, repeatable without limit) into any existing library at any unoccupied path. The injected content becomes a first-class library document: indexed, searchable, rendered to other users, and fed into agent prompt/review flows. The attacker gains no read access to the library and cannot overwrite existing paths, so the harm is integrity/content-injection into a trust domain that the system's own discovery document reserves for trusted-localhost callers.",
      "evidence": [
        "crates/quarry-server/src/tmp_document_handlers.rs:619-634 — boundary crossing out of the scanned component: the Promote arm of the tmp-document action handler parses caller-supplied JSON {library, path, if_match} and calls state.store.promote_tmp_document with no authentication and no authorization check on the target library; the only guard is the compile-time feature flag at line 620 (unlike sibling arms it does not even verify the tmp document first).",
        "crates/quarry-server/src/lib.rs:345-363 — the promote endpoint is mounted on the tmp route family /v1/tmp/documents/{*path}, which the project's discovery document (discovery.rs:256) advertises as reachable by 'anyone with /tmp/{secret}', while library REST APIs are documented as trusted-localhost; the router installs no auth middleware anywhere (lib.rs:215-217).",
        "crates/quarry-storage/src/tmp_documents.rs:119 — the tmp capability required by promote is self-minted: create_tmp_document_inner generates the secret via TmpDocumentSecret::generate() and returns it to the anonymous creator, so 'possession of a tmp secret' proves nothing about the caller.",
        "crates/quarry-storage/src/tmp_documents.rs:517-520 — promote_tmp_document validates only the tmp secret (TmpDocumentSecret::parse) and normalizes the caller-chosen target_path; the caller-supplied library slug is taken verbatim with no credential bound to it.",
        "crates/quarry-storage/src/tmp_documents.rs:523 — require_library_conn merely checks that the named library exists; it is an existence check, not an authorization decision.",
        "crates/quarry-storage/src/tmp_documents.rs:525-526 — the only precondition checked applies to the tmp document and is caller-controlled; the handler defaults it to WritePrecondition::None (tmp_document_handlers.rs:627-630).",
        "crates/quarry-storage/src/tmp_documents.rs:527-535 — the only target-side guard is an existence check that returns Conflict when target_path is occupied; it prevents overwrite but not unauthorized creation.",
        "crates/quarry-storage/src/tmp_documents.rs:540-556 — the sink: a single UPDATE sets library_id, document_scope = 'library', path, and clears expires_at on the anonymously-created row, re-scoping it into the target library as a normal document with no further check."
      ],
      "exploitScenarios": [
        "Attacker POSTs /v1/tmp/documents with a malicious Markdown body on a server whose tmp routes are reachable (non-loopback bind, permitted with only a log warning at crates/quarry-server/src/lib.rs:695, or an edge proxy sharing tmp URLs — the documented bearer-capability deployment); the response returns the 32-hex secret.",
        "Attacker identifies an existing library slug (predictable names such as a team/default slug, or any slug learned through shared links).",
        "Attacker POSTs /v1/tmp/documents/{secret}/promote with body {\"library\": \"<slug>\", \"path\": \"<unused path>\"}; no credentials beyond the self-minted secret are required.",
        "Storage executes the scope-flip UPDATE; the attacker's content is now a permanent library document that is indexed, searchable, and served to legitimate users and agents.",
        "Attacker repeats from step 1 to inject an unbounded number of documents (each fresh tmp document is independently promotable)."
      ],
      "preconditions": [
        "Server is built with both the tmp-documents and lib-documents features (otherwise the Promote arm returns NotFound).",
        "Attacker can reach the tmp route family — i.e. a deployment where tmp bearer URLs are shared beyond localhost (the documented intent of tmp documents) or a non-loopback bind.",
        "Attacker knows or guesses an existing library slug, since require_library_conn must succeed.",
        "The chosen target path is unoccupied in that library (existing documents cannot be overwritten).",
        "Exploitability is deployment-dependent: in a strict loopback-only deployment the same caller could already write the library directly, so the finding adds attack surface only where tmp routes are exposed more broadly than library routes (hence MEDIUM confidence despite the fully traced code path)."
      ],
      "recommendations": [
        "Root cause: require an independent authorization decision for the target library before re-scoping — promote must demand the same credential/permission as a direct library write (e.g. an authenticated library-write session or a per-library capability presented alongside the tmp secret), and promote_tmp_document should accept and verify that proof rather than treating the tmp secret as sufficient for a library-scope write.",
        "Hardening: move the promote operation off the anonymous tmp route family onto the library route family (e.g. POST /v1/libraries/{library}/documents/{path}:promote with the tmp secret in the request body) so that exposing tmp bearer URLs at an edge never exposes a library-write primitive; record the promoting identity on the transaction for audit.",
        "Regression test: an integration test asserting that a request carrying only a valid tmp secret (no library authorization) is rejected by the promote path, and that promote succeeds only when the target-library write credential is presented."
      ]
    }
  ]
}