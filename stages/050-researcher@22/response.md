I've completed a full read of the hot-path files and traced every auth-relevant flow. Here's my assessment:

**Verified clear through the auth lens:**

- **Tmp capability boundary** (`tmp_documents.rs`): every tmp entry point (get/head/history/version/diff/delete/set_ttl/fork/promote/import/block-mutation) routes through `TmpDocumentSecret::parse` and expiry-filtered, scope-filtered queries (`lib.rs:468-474`, `836-842`, `933-939`; `blocks.rs:1811-1824`). No path acts on a tmp document without the 122-bit secret.
- **Library scoping/IDOR**: all document, link, and version queries filter `library_id` + `document_scope`; `version_content_conn` (`versions.rs:180`) pins `v.document_id = ?1 AND v.id = ?2`, so a version ID from another document yields NotFound.
- **Promote scope flip** (`tmp_documents.rs:510-565`): requires the tmp secret, checks target-path conflict, and the deployment model (verified in `quarry-server` — no auth middleware anywhere) makes library writes equally open, so promotion grants no capability beyond the baseline `put_document`.
- **Race conditions in access decisions**: all check-then-act sequences (precondition check → write, conflict check → promote, fork clone) execute inside one `BEGIN IMMEDIATE` transaction under the in-process write mutex (`store.rs:206-216`), with the flock guarding cross-process exclusivity.
- **Invite tokens**: roles restricted to `viewer`/`editor` (`lib.rs:1929-1937`); redemption is outside storage.
- **Server boundary crossing** (checked where flows crossed): `session.rs:510` enforces `access.refuses(&seed.scope)` — tmp sessions refused without the secret-authenticated route; the promote handler is feature-gated and capability-based.

**Candidates evaluated and rejected as below the bar:** unvalidated `TmpTtl::ExpiresAt` strings (self-affecting retention preference, input-validation rather than auth, and an anonymous caller can already re-create content indefinitely), the unreserved `__tmp__` sentinel library ID (metadata-only visibility, no content or secret exposure), LIKE-wildcard prefixes (no boundary crossed — `prefix=None` lists the whole library anyway), and `document_id`-keyed building blocks (scope enforced by the server's `CollabAccess` check after seed).

```json
{
  "findings": []
}
```