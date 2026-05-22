#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA_DIR="$(mktemp -d /tmp/quarry-v1-data.XXXXXX)"
RAW_EXPORT="$(mktemp -d /tmp/quarry-v1-export.XXXXXX)"
GIT_REPO="$(mktemp -d /tmp/quarry-v1-git.XXXXXX)"
BINARY_FILE="$(mktemp /tmp/quarry-v1-binary.XXXXXX)"
PORT="${QUARRY_DEMO_PORT:-7834}"

run_quarry() {
  cargo run -q -p quarry-cli --manifest-path "$ROOT/Cargo.toml" -- --data-dir "$DATA_DIR" "$@"
}

printf '\000\001\002opaque-binary\n' > "$BINARY_FILE"

echo "== init"
run_quarry init

echo "== published write"
run_quarry write docs/v1.md --content "published base" --actor-id navan

echo "== draft, agent edit, comment, publish"
run_quarry draft start --name draft/codex-review --actor-id navan
run_quarry write docs/v1.md --ref draft/codex-review --content "draft update from agent" --actor-id codex --actor-kind agent
run_quarry comment --target ref:draft/codex-review:path:docs/v1.md --body "looks ready" --actor-id navan
run_quarry draft publish draft/codex-review --target published/main --actor-id navan

echo "== opaque binary pointer"
run_quarry binary add assets/mock.pdf --file "$BINARY_FILE" --media-type application/pdf --actor-id navan

echo "== structured document, events, presence, snapshots"
DOC_JSON="$(run_quarry document create docs/rich.md --text "rich base" --actor-id navan)"
DOC_ID="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["document"]["id"])' <<<"$DOC_JSON")"
run_quarry document op "$DOC_ID" --op-json '{"kind":"replace_text","text":"rich update"}' --actor-id codex --actor-kind agent
run_quarry document presence "$DOC_ID" --cursor-json '{"path":[0,0],"offset":4}' --actor-id navan
run_quarry document state "$DOC_ID"
SNAPSHOT_ID="$(run_quarry snapshots --ref published/main --limit 20 | python3 -c 'import json,sys; snaps=json.load(sys.stdin); print(next(s["id"] for s in snaps if any(e["path"]=="docs/rich.md" for e in s["entries"])))')"
run_quarry restore "$SNAPSHOT_ID" --ref published/main --actor-id navan
run_quarry events --limit 5

echo "== raw export"
run_quarry export "$RAW_EXPORT" --ref published/main
test -f "$RAW_EXPORT/docs/v1.md"
test -f "$RAW_EXPORT/assets/mock.pdf"
test -f "$RAW_EXPORT/docs/rich.md"

echo "== git materialize"
run_quarry git materialize "$GIT_REPO" --ref published/main --branch main
git -C "$GIT_REPO" log --oneline -1

echo "== external git conflict and ingest"
printf 'external conflicting change\n' > "$GIT_REPO/docs/v1.md"
git -C "$GIT_REPO" add docs/v1.md
git -C "$GIT_REPO" -c user.name=External -c user.email=external@example.invalid commit -m "external change" >/dev/null
run_quarry git ingest "$GIT_REPO" --ref published/main --actor-id git --actor-kind git-import

echo "== policy guardrail"
if run_quarry delete docs/v1.md --ref published/main --actor-id codex --actor-kind agent >/tmp/quarry-v1-delete.out 2>/tmp/quarry-v1-delete.err; then
  echo "agent delete unexpectedly succeeded" >&2
  exit 1
else
  cat /tmp/quarry-v1-delete.err
fi

echo "== api and mcp smoke"
run_quarry server --addr "127.0.0.1:$PORT" >"$DATA_DIR/server.log" 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

for _ in $(seq 1 50); do
  if curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

curl -fsS "http://127.0.0.1:$PORT/stats"
echo
curl -fsS -X POST "http://127.0.0.1:$PORT/mcp/tools/quarry_read" \
  -H 'content-type: application/json' \
  -d '{"ref":"published/main","path":"docs/v1.md"}'
echo
curl -fsS -X POST "http://127.0.0.1:$PORT/mcp" \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' >/dev/null
curl -fsS "http://127.0.0.1:$PORT/events?limit=3" >/dev/null
curl -fsS "http://127.0.0.1:$PORT/" | sed -n '1,8p'

echo "== demo artifacts"
echo "data_dir=$DATA_DIR"
echo "raw_export=$RAW_EXPORT"
echo "git_repo=$GIT_REPO"
