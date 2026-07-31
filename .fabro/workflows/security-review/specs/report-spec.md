# Deterministic security report and canonical bundle

The completed canonical bundle is the source of truth for one security scan. `render_report.py` validates that bundle and derives every presentation artifact from it. No model writes or rewrites the final report.

## Canonical files

- `scan-manifest.json` identifies the scan, target, revision, request, completion status, counts, and canonical file set.
- `candidate-ledger.jsonl` contains every unique candidate after deduplication. Each record has a rank and one disposition: `reportable`, `rejected`, `deferred`, or `verification-incomplete`. Candidates beyond a work budget remain in this ledger.
- `findings.json` contains only the reportable subset. It is the authoritative finding list. Each reportable finding also carries a `code` excerpt, which the ledger's candidate records do not.
- `coverage.json` records what the scan examined, skipped, deferred, or could not complete.
- `panel-votes.jsonl` contains one record for each dispatched panel, repanel, or red-team vote. Each record preserves the exact claim and cited evidence shown to that verifier, plus its verdict and reasoning when it completed.

The JSON Schema contracts for these records are in `schemas/`. The deterministic renderer also checks cross-file rules that JSON Schema cannot express.

## Derived files

The renderer creates only these presentation artifacts from the five canonical files:

- `SECURITY-REVIEW-RESULTS.md` for people.
- `SECURITY-REVIEW-RESULTS.html` for people, from `templates/report.html`.
- `SECURITY-REVIEW-RESULTS.jsonl` for finding consumers and CI gates.
- `SECURITY-REVIEW-REVISION-<tag>.json` for compatibility with current revision consumers.

SARIF is not generated. It can be added later as another deterministic view of the same canonical data.

## Ratings and display names

Severity describes impact. Difficulty describes the access, knowledge, and effort exploitation takes, so `LOW` difficulty is the worse case. When two reporters describe one candidate with different difficulties, deduplication keeps the lower one: a researcher who found a cheaper path is evidence the cheaper path exists.

Canonical `category` slugs are part of `ruleId`, and therefore of a finding's stable identity. The HTML report maps them to display names in the renderer instead. Renaming a slug would change every `findingId` derived from it.

## Evidence

`evidence` is the source-to-sink proof as a list of citations, one entry per hop
from the untrusted source to the dangerous operation. The report gives it its own
block, collapsed by default, so a long proof does not crowd out the claim. The
Markdown report lists the citations under **Evidence.**

A researcher that reports one blob of prose instead still normalizes, becoming a
one-entry list. That fallback exists so the older shape keeps working while
researchers catch up, and it goes when the shape is retired. The verifier's
`evidenceAsCited` is the citations joined by newlines, so the claim a panel saw
stays one string.

## Source excerpts

A researcher quotes one sink line in `snippet`. The excerpt shown in a report is not that quote: `final-tally` reads the lines around the finding from the reviewed tree, so the line numbers are the tree's own and no agent transcribes them. The excerpt is omitted when the file is unreadable, binary, oversized, or when the quoted line does not match the line that was read — a mismatch a stale line number or a commit-mode revision gap would cause. The report then falls back to the reporter's quoted line.

## HTML rendering

`templates/report.html` carries the page and its own script. The renderer substitutes one JSON payload into it and never builds markup from finding text. The payload escapes `<`, `>`, `&`, and every non-ASCII codepoint, so no finding text can close the script element, open an HTML comment, or end a JavaScript statement. The template's script writes model-authored text with `textContent` only.

## Required relationships

A reportable ledger record must match one entry in `findings.json`. Every reported finding must have a complete initial three-lens panel and at least two `TRUE_POSITIVE` votes. Deferred candidates must not appear in `findings.json`. Manifest counts must match the canonical records.

`findingId` identifies the same root vulnerability across scans. `occurrenceId` identifies that finding in one Fabro run. The Markdown and JSONL views copy both IDs from `findings.json`.

Severity describes impact. Confidence describes certainty. A two-of-three panel limits confidence to `medium`. Only a unanimous panel can produce `high` confidence.

A scan is `partial` when work was deferred, verification was incomplete, a research result was unusable, a reported finding was dropped for failing the finding contract, or verification status is `unverified`. The report must show this status and its reasons, and the HTML report says so above its findings.

`coverage.rejectedFindingReports` names every finding an agent reported that failed the contract, with the reason and the researcher that sent it. A dropped finding never becomes a candidate, so without this record a scan that discarded everything it was given would be indistinguishable from one that found nothing. The reasons are fixed strings naming the field at fault; they never quote the model's own text.

## Rendering safety

The renderer rejects invalid stable IDs, unsafe repository paths, control characters, duplicate identities, inconsistent vote claims, inconsistent cross-file counts, and an excerpt that does not highlight the finding's own line exactly once. It escapes model-authored text before placing it in Markdown. Code excerpts use Markdown code blocks.

Findings are derived from source and history review. The workflow does not attest whether agents executed commands.
