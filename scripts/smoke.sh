#!/usr/bin/env bash
# End-to-end smoke test against a running OpenLens (CI runs it after
# `docker compose up`). Needs curl and python3.
# Usage: scripts/smoke.sh <base-url>   (env: OPENLENS_ADMIN_TOKEN, OPENLENS_PROJECT_KEY)
set -euo pipefail
BASE=${1:-http://localhost:8686}
TOKEN=${OPENLENS_ADMIN_TOKEN:?set OPENLENS_ADMIN_TOKEN}
KEY=${OPENLENS_PROJECT_KEY:?set OPENLENS_PROJECT_KEY}
fail() { echo "SMOKE FAIL: $*" >&2; exit 1; }

echo "1. SDK is served"
curl -fsS "$BASE/c.js" | grep -q "openlens" || fail "/c.js"

echo "2. capture accepts a batch"
NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)
R=$(curl -fsS -X POST "$BASE/capture?key=$KEY" -H 'content-type: text/plain' \
  -d "{\"sent_at\":\"$NOW\",\"batch\":[{\"event\":\"\$identify\",\"distinct_id\":\"smoke\",\"properties\":{\"\$role\":\"owner\"}},{\"event\":\"\$pageview\",\"distinct_id\":\"smoke\",\"session_id\":\"s1\",\"properties\":{\"\$pathname\":\"/\",\"\$role\":\"owner\",\"\$release\":\"smoke-1\"}}]}")
echo "$R" | grep -q '"accepted":2' || fail "capture: $R"

echo "3. a wrong key is refused"
CODE=$(curl -s -o /dev/null -w '%{http_code}' -X POST "$BASE/capture?key=pk_wrong" -d '{"batch":[]}')
[ "$CODE" = 401 ] || fail "wrong key gave $CODE"

echo "4. events become queryable after the flush (<= 15 s)"
for i in $(seq 1 15); do
  N=$(curl -fsS -X POST "$BASE/api/projects/default/sql" -H "authorization: Bearer $TOKEN" \
      -H 'content-type: application/json' -d '{"sql":"SELECT count(*) AS n FROM events"}' |
      python3 -c 'import json,sys; print(json.load(sys.stdin)["rows"][0]["n"])')
  [ "$N" -ge 2 ] && break
  sleep 1
done
[ "$N" -ge 2 ] || fail "only $N events queryable"

echo "5. the API refuses requests without the token"
CODE=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/projects")
[ "$CODE" = 401 ] || fail "api without token gave $CODE"

echo "6. a login link opens the board, which shows the data"
URL=$(curl -fsS -X POST "$BASE/api/login-links" -H "authorization: Bearer $TOKEN" |
      python3 -c 'import json,sys; print(json.load(sys.stdin)["url"])')
PATH_ONLY="/l/${URL##*/l/}"
curl -fsS "$BASE$PATH_ONLY" | grep -q "Continue" || fail "login link page"
COOKIE=$(curl -s -o /dev/null -D - -X POST "$BASE$PATH_ONLY" | tr -d '\r' | sed -n 's/^[Ss]et-[Cc]ookie: \(ol_session=[^;]*\).*/\1/p')
[ -n "$COOKIE" ] || fail "login link set no cookie"
curl -fsS -H "cookie: $COOKIE" "$BASE/p/default" | grep -q "Daily active users" || fail "board"
curl -fsS -H "cookie: $COOKIE" "$BASE/p/default/events" | grep -q "smoke" || fail "events page"

echo "SMOKE OK"
