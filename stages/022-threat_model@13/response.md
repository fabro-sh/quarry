{
  "entryPoints": [
    ".fabro/workflows/security-review/scripts/security_review.py:3222 — main() parses workflow-invoked CLI arguments (prepare --mode/--effort/--scope/--base/--commit/--range/--focus)",
    ".fabro/workflows/security-review/scripts/security_review.py:391 — scan_id_from_args reads a scan ID from stdin (context.internal.run_id via stdin_source)",
    ".fabro/workflows/security-review/scripts/security_review.py:1977 — read_merge_input reads up to 30 MiB of untrusted JSON from stdin (agent outputs forwarded by Fabro via stdin_source=context.output.* / context.parallel.results)",
    ".fabro/workflows/security-review/scripts/security_review.py:2051 — merge() accepts agent-produced inventory, threat-model, findings, and verdict JSON and normalizes it into runtime/state.json",
    ".fabro/workflows/security-review/scripts/security_review.py:368 — stable_target_identity reads remote.origin.url from the reviewed repository's .git/config (untrusted)",
    ".fabro/workflows/security-review/scripts/security_review.py:678-708 — diff_stats parses git diff -z/--numstat output from the reviewed repository's history (untrusted)",
    ".fabro/workflows/security-review/scripts/render_report.py:429-464 — read_json/read_jsonl load the canonical bundle (scan-manifest.json, candidate-ledger.jsonl, findings.json, coverage.json, panel-votes.jsonl) whose content derives from agent output",
    ".fabro/workflows/security-review/scripts/render_report.py:2037 — render() entry point taking a bundle directory path supplied by the caller",
    ".fabro/workflows/security-review/scripts/git_readonly.py:107 — main(sys.argv[1:]) takes git arguments from scan agents; the reviewed tree's content flows back through stdout",
    ".fabro/workflows/security-review/security-review.fabro:32 — prepare node interpolates {{ inputs.* }} workflow inputs into a shell script line",
    ".fabro/workflows/security-review/scripts/security_review.py:2647-2677 — code_frame reads a source file from the reviewed tree at a model-supplied file:line and embeds its text into findings"
  ],
  "sinks": [
    ".fabro/workflows/security-review/scripts/security_review.py:293 — subprocess.run(['git', '-C', root, *arguments]) executes git with repository-influenced arguments (revisions, ranges, remote URL parsing upstream)",
    ".fabro/workflows/security-review/scripts/security_review.py:3147-3154 — load_renderer() loads render_report.py via importlib spec_from_file_location + exec_module (dynamic code execution of an in-repo file; integrity pinned by the SHA-256 check in security-review.fabro:32)",
    ".fabro/workflows/security-review/scripts/git_readonly.py:94 — subprocess.run(['git', ...]) executes git subcommands requested by agents, guarded by validate_arguments (lines 43-65) and --no-ext-diff/--no-textconv",
    ".fabro/workflows/security-review/scripts/security_review.py:212-237 — write_json/write_jsonl + os.replace write state.json, canonical bundle, and report files derived from agent JSON",
    ".fabro/workflows/security-review/scripts/security_review.py:3085-3090 — write_canonical_bundle writes agent-derived content into SECURITY-REVIEW-* products directory",
    ".fabro/workflows/security-review/scripts/security_review.py:1114-1118 — unique_report_dir/mkdir + .gitignore writes create the report directory in the reviewed tree",
    ".fabro/workflows/security-review/scripts/render_report.py:2014-2024 — atomic_write via tempfile.mkstemp/os.replace writes SECURITY-REVIEW-RESULTS.{md,html,jsonl} and the revision stamp",
    ".fabro/workflows/security-review/scripts/render_report.py:1957-1970 — embed_json serializes agent-controlled text into a <script> block in the HTML report (XSS surface; relies on ensure_ascii plus <, >, & escaping)",
    ".fabro/workflows/security-review/scripts/render_report.py:1389-1416 — escape_markdown/markdown_block render agent-controlled finding text into Markdown (injection into rendered reports)",
    ".fabro/workflows/security-review/scripts/security_review.py:711-734 — workspace_digest reads and hashes every file in the reviewed tree (file I/O over attacker-controlled paths, including symlinks via os.readlink at line 725)",
    ".fabro/workflows/security-review/scripts/security_review.py:2652-2672 — code_frame opens and reads bytes of a reviewed-tree file chosen by model-supplied path (bounded by normalize_repo_path and CODE_FRAME_MAX_BYTES)",
    ".fabro/workflows/security-review/scripts/security_review.py:1984 — json.loads of agent stdin (deserialization of untrusted data, capped at MAX_STDIN_BYTES)",
    ".fabro/workflows/security-review/scripts/security_review.py:345 — urlsplit() parses the untrusted remote.origin.url (credential-strip parsing; parsed.port at line 347 can raise ValueError, caught)"
  ],
  "assumptions": [
    "security_review.py:278-309 git() assumes the reviewed repository's .git/config cannot re-enable external diff drivers or prompts because GIT_CONFIG_GLOBAL is nulled — but repository-local config (diff drivers, textconv, alias) is not explicitly disabled for every invocation; diff calls pass --no-ext-diff/--no-textconv (lines 680-695) while other git calls do not",
    "security-review.fabro:32 — the SHA-256 pinning of scripts assumes the prepare step itself (python3 -c hash check embedded in an untrusted-tree file) is executed before any other workflow code and that hash failure aborts the run",
    "security_review.py:386-405 scan_id_from_args assumes Fabro's stdin_source supplies the run ID, and that a 256-byte, SCAN_ID_RE-validated token is safe to embed in occurrence IDs",
    "security_review.py:467-473 validate_revision assumes SAFE_REV_RE keeps model/user-supplied revisions from becoming git option injection (leading-dash tokens rejected by character class)",
    "security_review.py:565-580 normalize_repo_path assumes rejecting '..', absolute paths, and backslashes makes a model-supplied file path safe to read under root() — no symlink escape check before code_frame reads the file",
    "render_report.py:400-413 safe_text assumes the control-character allowlist (\\n, \\t) plus UNSAFE_DISPLAY_CODEPOINTS is sufficient to neutralize display attacks in the HTML/Markdown reports",
    "git_readonly.py:25-34 FORBIDDEN_ARGUMENTS assumes the blocklist (-c, --exec-path, --ext-diff, --output, --upload-pack, etc.) plus --no-ext-diff/--no-textconv covers all git config/exec escalation paths available to diff|show|log|blame",
    "security_review.py:737-752 assert_workspace_unchanged assumes a full-tree SHA-256 digest at publication gates proves scan agents did not modify the tree (TOCTOU between final-tally and render-report is covered only by re-checking)",
    "security_review.py:1996-2047 merge_phase assumes results array order matches phase_jobs order (positional matching, optional index cross-check) — assumes Fabro preserves parallel result ordering",
    "render_report.py:1973-1991 read_template assumes templates/report.html contains exactly one __REPORT_DATA__ marker and that escaping in embed_json prevents script breakout"
  ],
  "trustBoundaries": [
    ".fabro/workflows/security-review/scripts/security_review.py:2030-2046 — agent (LLM) JSON crosses into deterministic state: normalize_findings_result/finding_or_rejection validate before anything enters state.json",
    ".fabro/workflows/security-review/scripts/security_review.py:2100-2111 verification_claim — researcher-produced candidate text is re-fed to panel/repanel/redteam agents as their input (model-to-model boundary; withheld fields noted in docstring)",
    ".fabro/workflows/security-review/scripts/git_readonly.py:43-74 — agent-supplied argv crosses into a git subprocess through the validation gate",
    ".fabro/workflows/security-review/security-review.fabro:32 — workflow inputs (mode/scope/base/commit/range/focus) cross into a shell script; validation happens only later inside prepare (parse_scope, validate_revision)",
    ".fabro/workflows/security-review/scripts/security_review.py:367-383 — the reviewed repository's .git/config remote URL crosses into target identity material after canonical_remote_identity strips credentials",
    ".fabro/workflows/security-review/scripts/render_report.py:2037-2117 — agent-derived canonical bundle content crosses into HTML/Markdown artifacts a human opens in a browser",
    ".fabro/workflows/security-review/scripts/security_review.py:2647-2693 — reviewed-tree source bytes cross into the report as the code excerpt, gated by normalize_repo_path, NUL/UTF-8 checks, and size caps",
    ".fabro/workflows/security-review/scripts/security_review.py:3141-3155 — in-repo render_report.py crosses from data (a file in the reviewed tree) to executed code via exec_module, gated only by the fabro-side hash pin"
  ],
  "hotFiles": [
    ".fabro/workflows/security-review/scripts/security_review.py",
    ".fabro/workflows/security-review/scripts/render_report.py",
    ".fabro/workflows/security-review/scripts/git_readonly.py",
    ".fabro/workflows/security-review/security-review.fabro",
    ".fabro/workflows/security-review/templates/report.html",
    ".fabro/workflows/security-review/workflow.toml"
  ]
}