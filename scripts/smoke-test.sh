#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${GEMINI_BASE_URL:-http://127.0.0.1:8000}"
BASE_URL="${BASE_URL%/}"
IMAGE_TEST="${GEMINI_TEST_IMAGE:-0}"
COMPAT_TEST="${GEMINI_TEST_COMPAT:-0}"
API_KEY="${GEMINI_API_KEY:-}"

if [[ -z "$API_KEY" && -f .env ]]; then
  API_KEY="$(sed -n 's/^GEMINI_API_KEY=//p' .env | head -n 1 | tr -d '\r' | sed 's/^\"//;s/\"$//')"
fi
if [[ -z "$API_KEY" ]]; then
  echo "No API key found. Run './gemini-rust-gateway init' or export GEMINI_API_KEY." >&2
  exit 1
fi
command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required for JSON checks" >&2; exit 1; }

AUTH=( -H "Authorization: Bearer $API_KEY" )
echo "[1/5] Frontend and health"
curl -fsS "$BASE_URL/" | grep -q "Gemini Gateway"
curl -fsS "$BASE_URL/health" | python3 -c 'import json,sys; assert json.load(sys.stdin)["status"] == "ok"'

echo "[2/5] Readiness"
curl -fsS "$BASE_URL/readyz" >/dev/null

echo "[3/5] Authentication and models"
MODELS="$(curl -fsS "${AUTH[@]}" "$BASE_URL/v1/models")"
MODEL="$(python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["data"]; print(d["data"][0]["id"])' <<<"$MODELS")"
echo "      model: $MODEL"

echo "[4/5] Non-streaming chat"
CHAT_BODY="$(python3 -c 'import json,sys; print(json.dumps({"model":sys.argv[1],"messages":[{"role":"user","content":"Reply with a short sentence confirming this Rust gateway chat test works."}]}))' "$MODEL")"
curl -fsS "${AUTH[@]}" -H "Content-Type: application/json" -d "$CHAT_BODY" "$BASE_URL/v1/chat/completions" |
  python3 -c 'import json,sys; d=json.load(sys.stdin); text=d["choices"][0]["message"]["content"]; assert text.strip(); print("      ok:", text[:80].replace("\n", " "))'

if [[ "$COMPAT_TEST" == "1" ]]; then
  echo "[compat] OpenAI Responses API"
  RESPONSES_BODY="$(python3 -c 'import json,sys; print(json.dumps({"model":sys.argv[1],"input":"Reply with exactly: rust-responses-ok"}))' "$MODEL")"
  curl -fsS "${AUTH[@]}" -H "Content-Type: application/json" -d "$RESPONSES_BODY" "$BASE_URL/v1/responses" |
    python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["status"] == "completed" and d["output"]'
  echo "[compat] Gemini v1beta generateContent"
  GEMINI_BODY='{"contents":[{"role":"user","parts":[{"text":"Reply with exactly: rust-v1beta-ok"}]}]}'
  curl -fsS -H "x-goog-api-key: $API_KEY" -H "Content-Type: application/json" -d "$GEMINI_BODY" "$BASE_URL/v1beta/models/$MODEL:generateContent" |
    python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["candidates"]'
fi

if [[ "$IMAGE_TEST" == "1" ]]; then
  echo "[5/5] Real image generation"
  IMAGE_BODY="$(python3 -c 'import json,sys; print(json.dumps({"type":"image","prompt":"A small matte green robot on a clean white background, product photography","model":sys.argv[1],"count":1}))' "$MODEL")"
  TASK="$(curl -fsS "${AUTH[@]}" -H "Content-Type: application/json" -d "$IMAGE_BODY" "$BASE_URL/v1/generations")"
  TASK_ID="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])' <<<"$TASK")"
  for _ in {1..320}; do
    sleep 3
    TASK="$(curl -fsS "${AUTH[@]}" "$BASE_URL/v1/tasks/$TASK_ID")"
    STATUS="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])' <<<"$TASK")"
    echo "      $STATUS  $TASK_ID"
    [[ "$STATUS" == "completed" ]] && break
    [[ "$STATUS" =~ ^(failed|interrupted|rejected|canceled)$ ]] && { echo "$TASK" >&2; exit 1; }
  done
  python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["status"] == "completed"; assert any(x.get("content_type", "").startswith("image/") for x in d["result"]["files"])' <<<"$TASK"
else
  echo "[5/5] Image test skipped (run with GEMINI_TEST_IMAGE=1)"
fi
echo "PASS - smoke test completed"
