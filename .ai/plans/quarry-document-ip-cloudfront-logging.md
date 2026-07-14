# Quarry document IP and CloudFront logging plan

Date: 2026-07-14

## Goal

Add a durable creation IP address to the SQL `documents` row for new anonymous temporary documents, and enable privacy-minimized CloudFront standard logging v2 to a private S3 bucket. Do not add AWS WAF in this change. Keep ALB access and connection logging disabled because ALB sees CloudFront as its client and its unredactable request line would copy Quarry capability secrets into logs.

## Decisions and invariants

- Add nullable `documents.created_ip_address TEXT`. Existing rows and documents created outside a trusted HTTP deployment remain `NULL`.
- Store the canonical full IPv4 or IPv6 address returned by `std::net::IpAddr::to_string()`. Do not store the viewer source port.
- Populate the column only when a new tmp document row is inserted. Updates, restores, collaboration checkpoints, TTL changes, and promotion must never replace the creation IP.
- Do not expose the IP through `Document`, `DocumentListEntry`, REST responses, OpenAPI, browser state, events, or application logs. Abuse investigations query SQL under operator access.
- Trust `CloudFront-Viewer-Address` only when the server is explicitly configured with `--client-ip-source cloudfront-viewer-address` / `QUARRY_CLIENT_IP_SOURCE=cloudfront-viewer-address`. Local/default mode remains `none` and stores `NULL`.
- In trusted CloudFront mode, fail tmp-document creation closed with a generic service error if the header is missing or malformed. This makes an edge-policy regression visible instead of silently losing the abuse signal.
- Add an exact CloudFront behavior for `/v1/tmp/documents`, ahead of `/v1/*`, so only the creation route receives `CloudFront-Viewer-Address` and the selected log field `cache-behavior-path-pattern` can identify creation traffic without recording the URI.
- CloudFront logs must exclude URI path, query string, cookies, referrer, user agent, `X-Forwarded-For`, request bodies, and all document content.
- Use 35-day CloudFront log retention, slightly longer than Quarry's default 30-day tmp-document lifetime. Raw IPs in the SQL database follow database/backup retention, per the explicit abuse-protection decision.
- Do not backfill IP addresses for existing documents; the required source data was not retained.

## Current state

- `POST /v1/tmp/documents` accepts anonymous creation and passes content, metadata, content type, and TTL to `QuarryStore::create_tmp_document`.
- `documents` has no creation-IP column. Schema setup is an inline idempotent schema plus compatibility helpers in `crates/quarry-storage/src/schema.rs`.
- Production CloudFront distribution `E3J081CFY0XCL2` uses managed `AllViewer` for `/v1/*`, has no exact creation behavior, no WAF, no legacy/v2/real-time logging, and no Web ACL.
- Production ALB `quarry-production-alb` has access and connection logs disabled; leave those attributes unchanged.
- Production is account `449957914122`, region `us-east-1`, administered with profile `quarry-production-admin` and SSM-only host access.

## Implementation

### 1. Add trusted client-IP server configuration

Files:

- `crates/quarry-cli/src/lib.rs`
- `crates/quarry-server/src/lib.rs`
- `crates/quarry-server/src/tmp_document_handlers.rs`

Changes:

1. Introduce a small validated enum such as `ClientIpSource::{None, CloudFrontViewerAddress}`. Default to `None` so local and embedded Quarry servers do not trust arbitrary forwarding headers.
2. Add `--client-ip-source` with environment binding `QUARRY_CLIENT_IP_SOURCE`; accept only `none` and `cloudfront-viewer-address`.
3. Thread the setting into `AppState` through a server configuration constructor while preserving default `app_state(store)`, `router(store)`, and test behavior.
4. Add `HeaderMap` extraction to `create_tmp_document`.
5. When configured for CloudFront, parse `CloudFront-Viewer-Address` as `SocketAddr`, extract its `IpAddr`, canonicalize it, and pass it to storage. Accept both IPv4 (`198.51.100.10:46532`) and bracketed IPv6 (`[2001:db8::1]:46532`). Do not fall back to the leftmost `X-Forwarded-For` entry.
6. Treat missing, repeated, non-UTF-8, or invalid trusted headers as deployment errors. Return the existing generic API error envelope and emit only a fixed structured warning with a reason code; never log the raw header or derived IP.

Tests:

- CLI parsing/default/env tests for both enum values and rejection of unknown values.
- Unit tests for IPv4, IPv6, missing, repeated, and malformed `CloudFront-Viewer-Address` values.
- Router tests proving default mode permits local creation and stores no IP, while trusted mode requires a valid CloudFront header.

### 2. Add the SQL column, index, and additive migration

Files:

- `crates/quarry-storage/src/lib.rs`
- `crates/quarry-storage/src/schema.rs`
- `crates/quarry-storage/src/tmp_documents.rs`
- `crates/quarry-storage/tests/storage_lifecycle.rs`

Changes:

1. Add `created_ip_address TEXT` to the canonical `documents` schema.
2. Add an idempotent `ensure_documents_created_ip_address_column` compatibility migration using `PRAGMA table_info(documents)` and `ALTER TABLE documents ADD COLUMN created_ip_address TEXT`. Run it after `migrate_documents_scope_ttl`, since that migration can rebuild the table.
3. Add a partial abuse-investigation index:

   ```sql
   CREATE INDEX IF NOT EXISTS idx_documents_created_ip_address_created_at
     ON documents(created_ip_address, created_at)
     WHERE created_ip_address IS NOT NULL;
   ```

4. Extend the new-row insert path to accept `Option<IpAddr>` and bind its canonical string into `created_ip_address` for tmp documents. Library/local inserts bind `NULL`.
5. Keep the current public update APIs source-compatible by introducing a private creation-context/helper path: normal `put_tmp_document*` calls pass no creation IP, while `create_tmp_document` passes the trusted IP. `ensure_tmp_document_conn` must only use the IP when inserting a new row.
6. Do not add the column to public core response types or existing document SELECT projections.

Tests:

- Opening a pre-column database adds the nullable column and index without changing existing rows.
- A tmp document created with IPv4 stores the canonical IPv4 string.
- A tmp document created with IPv6 stores the canonical compressed IPv6 string.
- A subsequent PUT, block transaction, checkpoint, restore, TTL update, and promotion preserve the original IP.
- Local/library creation stores `NULL`.
- REST creation responses and OpenAPI contain no creation-IP field.

### 3. Update documentation and the threat model

Files:

- `docs/security/threat-model.md`
- `docs/development.md`
- `docs/operations/backup-restore.md`
- new `docs/operations/cloudfront-logging.md`

Changes:

1. Record raw document-creation IP as a stored data type, including its presence in database backups and its operator-only purpose.
2. Update T3 to reflect that Quarry now has an accountability/abuse signal but still has no WAF or application rate limiting. Remove any claim that edge rate limiting is already deployed.
3. Document the trusted-header configuration and why direct/local servers default to `none`.
4. Document the abuse query without printing content or capability URLs by default, for example grouping/counting by IP and time window before selecting specific rows.
5. Document the CloudFront field allowlist, prohibited fields, S3 retention, verification, and the decision to leave ALB logs disabled.

### 4. Configure CloudFront to send a trustworthy viewer address

AWS changes, all with explicit `--profile quarry-production-admin`; verify STS account `449957914122` first.

1. Export and save the current CloudFront distribution config and ETag as a rollback artifact outside the repository, without logging origin-verification secrets.
2. Create a custom origin request policy named `quarry-production-create-document` that preserves the current behavior for viewer headers/cookies/query strings and uses `allViewerAndWhitelistCloudFront` to add only `CloudFront-Viewer-Address`.
3. Add an ordered exact-path behavior `/v1/tmp/documents` before `/v1/*`. Clone the existing `/v1/*` origin, cache policy, allowed/cached methods, compression, protocol policy, function associations, and other settings; change only the path pattern and origin request policy.
4. Wait for the distribution status to return to `Deployed`.
5. Verify a request through CloudFront reaches the current application normally before enabling trusted mode in Quarry.

This exact behavior limits propagation of the raw viewer address to the one endpoint that persists it and provides a non-secret behavior label for access logs.

### 5. Create the private CloudFront log bucket and v2 delivery

Proposed resources:

- S3 bucket: `quarry-production-cloudfront-logs-449957914122`
- CloudWatch delivery source: `quarry-production-cloudfront`
- CloudWatch delivery destination: `quarry-production-cloudfront-s3`
- Prefix/partition: `cloudfront/{distributionid}/{yyyy}/{MM}/{dd}/{HH}`
- Output: JSON

Bucket controls:

1. Create in `us-east-1` with all four S3 Block Public Access settings enabled.
2. Use bucket-owner-enforced object ownership, default SSE-S3 encryption, no public ACLs/policies, and tags for service/environment/data classification.
3. Grant only the CloudWatch vended-log delivery service the required write/check permissions, constrained by source account and CloudFront distribution ARN where supported. Grant read/list only to the production administrator role used for incident response.
4. Add lifecycle deletion after 35 days and abort incomplete multipart uploads after 7 days. Do not enable versioning or Object Lock, which would defeat predictable deletion.

CloudFront standard logging v2 field allowlist:

```text
date
time
timestamp(ms)
x-edge-location
c-ip
cs-method
sc-status
x-edge-result-type
x-edge-response-result-type
x-edge-detailed-result-type
x-edge-request-id
cs-protocol
cs-protocol-version
ssl-protocol
ssl-cipher
sc-bytes
cs-bytes
time-taken
time-to-first-byte
c-country
asn
cache-behavior-path-pattern
```

Explicitly exclude `cs-uri-stem`, `cs-uri-query`, `cs(Cookie)`, `cs(Referer)`, `cs(User-Agent)`, `x-forwarded-for`, `c-port`, and custom viewer log data. Keep CloudFront cookie logging disabled.

Verification:

1. Confirm the v2 delivery source, destination, and delivery select exactly the approved fields.
2. Confirm bucket public-access, ownership, encryption, policy, and lifecycle settings.
3. Generate ordinary health/static traffic and one test creation request. CloudFront notes that standard-log delivery can take several hours to become reliable, so keep the rollout open until at least one object arrives.
4. Inspect a delivered object and assert that it contains the viewer IP and `/v1/tmp/documents` behavior label, but contains no 32-hex tmp secret, URI path, query, cookie, referrer, user agent, or document content.

### 6. Production rollout

Order matters so trusted mode never runs before CloudFront supplies the header.

1. Verify AWS identity, current instance, healthy target, CloudFront distribution, and clean repository/release state.
2. Create the S3 bucket and CloudFront v2 log delivery; these are independent of the application release.
3. Deploy the exact creation behavior and trusted CloudFront header; wait for `Deployed` and smoke-test creation against the old application, which ignores the new header.
4. Before the additive database migration, stop Quarry briefly through SSM, create a consistent backup and an encrypted EBS snapshot of the retained data volume, record the snapshot ID, then restart and verify health. Do not expose SSH or a public instance IP.
5. Release the tested application through the existing immutable-image release workflow.
6. Update `/opt/quarry/compose.yaml` to set `QUARRY_CLIENT_IP_SOURCE=cloudfront-viewer-address`, preserving a timestamped compose backup, and recreate the container through SSM.
7. Verify local Docker health, public health, ALB target health, browser creation, agent/CLI creation, updates, collaboration/WebSockets, and deletion.
8. Create a disposable document through the public endpoint, query SQL through a consistent read-only database copy, and confirm `created_ip_address` matches the test request's CloudFront viewer IP. Confirm later document mutations do not alter it. Delete the disposable document and remove the working copy.
9. Verify CloudFront logs as described above and reconfirm both ALB access and connection logging remain disabled.
10. Update the Foreman `techops-quarry-upgrade` runbook to verify the trusted-IP environment setting, CloudFront v2 delivery, S3 lifecycle/private controls, and ALB logging posture during future production checks.

## Verification commands

Run the repository's checked-in CI-equivalent checks after implementation:

```sh
cd ui && bun install --frozen-lockfile && bun run build
cargo fmt --check --all
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo clippy --locked -p quarry-server -p quarry-cli -p quarry --all-features --all-targets -- -D warnings
cargo test --locked --workspace
cargo test --locked -p quarry-server -p quarry-cli -p quarry --all-features
```

Also run the targeted migration, storage, REST, and CLI tests while iterating, followed by a fresh-server end-to-end test with both default `none` mode and trusted CloudFront mode.

## Rollback

- Application rollback: restore the timestamped compose file or pin the previous immutable image digest and recreate the container. The nullable additive column and index are safe for the previous binary to ignore; do not attempt an emergency column drop.
- CloudFront rollback: restore the saved distribution config with the matching current ETag, removing the exact behavior and custom origin request policy after it is no longer referenced.
- Logging rollback: disable/delete the v2 delivery while retaining the private bucket long enough to diagnose the incident; lifecycle deletion continues to bound retained data.
- Data rollback: if migration startup damages the database, stop Quarry, restore the pre-migration backup or EBS snapshot, and redeploy the previous digest before reopening traffic.

## Out of scope

- AWS WAF, CAPTCHA/challenge, or any rate-limiting policy.
- ALB access or connection logging.
- Retrofitting IPs onto existing documents.
- Exposing creation IP through public/admin HTTP APIs or the browser UI.
- IP-based automatic blocking or takedown workflows; this change records and indexes the signal for operator use.

## Unresolved questions

None. This plan assumes IP collection applies only to newly created anonymous tmp documents, CloudFront logs expire after 35 days, full canonical IPs are retained with the document row/backups, and ALB logging stays disabled.
