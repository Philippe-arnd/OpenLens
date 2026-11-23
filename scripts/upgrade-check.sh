#!/usr/bin/env bash
# Upgrade safety (#158): data written by the previous release must read the
# same with the new image. Starts OLD on a fresh volume, writes a fixed set
# of facts (events, a flag, a feature map), records them, then starts NEW on
# the same volume and compares. Board numbers are not compared: their
# definitions may change between releases (see the CHANGELOG); the board
# must still load. Then starts OLD again on the data NEW opened: going back
# to the previous image must keep working (same data format) and read the
# same facts. A newer data format is refused by the binary itself (#199,
# unit-tested in apps/openlens/src/upgrade.rs).
# Usage: scripts/upgrade-check.sh <old-image> <new-image>   (needs docker, curl, python3)
set -euo pipefail
OLD=$1 NEW=$2
VOL=openlens-upgrade-$$ NAME=openlens-upgrade-$$ PORT=18686
TOKEN=adm_upgrade KEY=pk_upgrade
B="http://localhost:$PORT"
fail() { echo "UPGRADE FAIL: $*" >&2; docker logs "$NAME" 2>&1 | tail -30 >&2 || true; exit 1; }
cleanup() { docker rm -f "$NAME" > /dev/null 2>&1 || true; docker volume rm "$VOL" > /dev/null 2>&1 || true; }
trap cleanup EXIT

start() {
  docker run -d --name "$NAME" -p "$PORT:8686" -v "$VOL:/data" \
    -e OPENLENS_ADMIN_TOKEN=$TOKEN -e OPENLENS_PROJECT_KEY=$KEY "$1" > /dev/null
  for _ in $(seq 1 60); do curl -fs "$B/healthz" > /dev/null 2>&1 && return 0; sleep 1; done
  fail "$1 did not become healthy"
}
api() { curl -fsS -H "authorization: Bearer $TOKEN" "$@"; }
sql() { api -X POST "$B/api/projects/default/sql" -H 'content-type: application/json' -d "{\"sql\":\"$1\"}" |
  python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["rows"], sort_keys=True))'; }
facts() {
  echo "events: $(sql 'SELECT count(*) AS n, count(DISTINCT distinct_id) AS ids, count(DISTINCT session_id) AS sessions FROM raw_events')"
  echo "by event: $(sql 'SELECT event, count(*) AS n FROM raw_events GROUP BY event ORDER BY event')"
  echo "projects: $(api "$B/api/projects" | python3 -c 'import json,sys; print(sorted((p["id"], p["key"]) for p in json.load(sys.stdin)["projects"]))')"
  echo "flags: $(api "$B/api/projects/default/flags" | python3 -c 'import json,sys; d=json.load(sys.stdin); d=d.get("flags", d); print(sorted((f["key"], f["enabled"], f["rollout"]) for f in d))')"
  echo "feature map: $(api "$B/api/projects/default/feature-map" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("yaml", "").strip().splitlines()[:3])')"
}

echo "1. $OLD writes the data"
start "$OLD"
NOW=$(date -u +%s)
python3 - "$NOW" > /tmp/upgrade-batch.json <<'PY'
import json, sys
now = int(sys.argv[1]); batch = []
iso = lambda t: __import__("datetime").datetime.utcfromtimestamp(t).strftime("%Y-%m-%dT%H:%M:%SZ")
for u in range(20):
    for d in range(3):
        t = now - (d * 86400) - u * 60
        sid = f"s{u}-{d}"
        role = "admin" if u % 4 == 0 else "member"
        batch.append({"event": "$identify", "distinct_id": f"u{u}", "session_id": sid, "timestamp": iso(t), "properties": {"$role": role, "$release": "r1"}})
        batch.append({"event": "$pageview", "distinct_id": f"u{u}", "session_id": sid, "timestamp": iso(t + 5), "properties": {"$pathname": "/notes", "$role": role, "$release": "r1"}})
        batch.append({"event": "note_saved", "distinct_id": f"u{u}", "session_id": sid, "timestamp": iso(t + 30), "properties": {"$feature": "notes", "$role": role, "$release": "r1"}})
print(json.dumps({"sent_at": iso(now), "batch": batch}))
PY
curl -fsS -X POST "$B/capture?key=$KEY" -H 'content-type: text/plain' --data-binary @/tmp/upgrade-batch.json | grep -q '"accepted":180' || fail "capture"
api -X PUT "$B/api/projects/default/flags/upgrade-check" -H 'content-type: application/json' \
  -d '{"key":"upgrade-check","description":"kept across upgrades","enabled":true,"rollout":25,"roles":[],"accounts":[]}' > /dev/null || fail "flag"
printf 'roles:\n  - key: admin\n    match: [admin]\nfeatures:\n  - key: notes\n    name: Notes\n    success: note_saved\n    entry: { routes: ["/notes"] }\n' |
  api -X PUT "$B/api/projects/default/feature-map" --data-binary @- > /dev/null || fail "feature map"
for _ in $(seq 1 20); do [ "$(sql 'SELECT count(*) AS n FROM raw_events')" = '[{"n": 180}]' ] && break; sleep 1; done
facts > /tmp/upgrade-before.txt
cat /tmp/upgrade-before.txt
docker stop -t 20 "$NAME" > /dev/null && docker rm "$NAME" > /dev/null

echo "2. $NEW reads it"
start "$NEW"
facts > /tmp/upgrade-after.txt
diff -u /tmp/upgrade-before.txt /tmp/upgrade-after.txt || fail "the data reads differently after the upgrade"
CODE=$(api -o /dev/null -w '%{http_code}' "$B/api/projects/default/board")
[ "$CODE" = 200 ] || fail "board gave $CODE"
docker stop -t 20 "$NAME" > /dev/null && docker rm "$NAME" > /dev/null

# Releases up to v0.2.0 name their upgrade copy after the second they
# start, and refuse a name that exists: give them a fresh second.
sleep 2
echo "3. $OLD again (rollback)"
start "$OLD"
facts > /tmp/upgrade-rollback.txt
diff -u /tmp/upgrade-before.txt /tmp/upgrade-rollback.txt || fail "the data reads differently after going back to $OLD"
echo "upgrade check passed: $OLD -> $NEW -> $OLD"
