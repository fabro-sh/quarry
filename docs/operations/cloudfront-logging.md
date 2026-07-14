# CloudFront Logging

The production CloudFront distribution sends privacy-minimized standard logs
v2 to a private S3 bucket. These logs are an abuse and availability signal, not
a document audit trail. AWS WAF is not enabled, and ALB access and connection
logging remain disabled because ALB cannot redact the capability secret from
the request line.

## Production resources

- Distribution: `E3J081CFY0XCL2`
- S3 bucket: `quarry-production-cloudfront-logs-449957914122`
- Delivery source: `quarry-production-cloudfront`
- Delivery destination: `quarry-production-cloudfront-s3`
- Output: JSON, partitioned below `cloudfront/`
- Retention: delete objects after 35 days; abort incomplete multipart uploads
  after 7 days

The bucket must retain S3 Block Public Access, bucket-owner-enforced ownership,
default SSE-S3 encryption, and no versioning or Object Lock. Access is limited
to the CloudWatch vended-log delivery service and production administrators.

## Allowed fields

The delivery configuration may contain only:

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

Never add `cs-uri-stem`, `cs-uri-query`, `cs(Cookie)`, `cs(Referer)`,
`cs(User-Agent)`, `x-forwarded-for`, `c-port`, or custom viewer log data. URI
paths for tmp documents contain bearer capability secrets.

The exact `/v1/tmp/documents` cache behavior supplies
`CloudFront-Viewer-Address` to the origin and gives creation traffic the
non-secret `cache-behavior-path-pattern` label. The broader `/v1/*` behavior
must not forward this header.

## Verification

Use the `quarry-production-admin` AWS profile in `us-east-1` and verify all of
the following after a distribution or logging change:

1. The exact creation behavior still precedes `/v1/*` and uses the dedicated
   origin request policy.
2. The v2 delivery field list exactly matches the allowlist above.
3. S3 public access is blocked, ownership and encryption controls are present,
   and the 35-day lifecycle is active.
4. A delivered object contains `c-ip` and the creation behavior label, but no
   URI, query, cookie, referrer, user agent, document content, or 32-hex tmp
   secret.
5. ALB access and connection logging remain disabled.

Standard logs are delayed and can take several hours to become consistently
available. Do not treat absence of a just-generated record as proof that the
delivery is broken.

## Abuse investigation

Start with aggregate SQL over a narrow time window; do not select document
paths or content unless an investigation requires them:

```sql
SELECT created_ip_address, COUNT(*) AS documents_created
FROM documents
WHERE document_scope = 'tmp'
  AND created_ip_address IS NOT NULL
  AND created_at >= ?1
  AND created_at < ?2
GROUP BY created_ip_address
ORDER BY documents_created DESC;
```

The application never logs the trusted header or derived address. Operator SQL
and private CloudFront logs are the two approved access paths for this data.
