#!/usr/bin/env bash
# One memory cap of the `scale` job of perf.yml (#321, #460): the board of
# the generated app, computed cold three times (each after a restart) and
# warm, with the server's peak memory, under a cgroup v2 memory.max.
#
# Usage: perf-scale-cap.sh CAP      CAP: 1G, 512M... or nocap
# Env:   EVENTS, PROFILE; GEN, the generated dir (data/, map.yaml,
#        gen.json), which is never written to: the server runs on a copy;
#        BIN, the dir of the perf binaries; GEN_SECONDS (empty when the
#        data came from the cache) and CACHED (true or false).
# Writes out/CAP/: scale.json, diag.txt, queries.txt, server.log.
set -euo pipefail
CAP="$1"
MEMORY=""
[ "$CAP" != nocap ] && MEMORY="$CAP"
CG=/sys/fs/cgroup/openlens
OUT="$PWD/out/$CAP"
mkdir -p "$OUT"
cd "$OUT"
# The server writes to its data dir (the map push, the background rollup
# pass, sessions), so every cap starts from a fresh copy of the generated
# dir, as each job started from fresh data before (D-046).
D="$RUNNER_TEMP/scale-$CAP"
rm -rf "$D"
cp -a "$GEN/data" "$D"
# The memory cap: the server goes into a cgroup with memory.max (no
# swap), as a container limit would. Only the server: the copy above and
# the curls stay outside, so they are not charged to the cap.
if [ -n "$MEMORY" ]; then
  echo "$MEMORY" | sudo tee "$CG/memory.max" > /dev/null
  echo 0 | sudo tee "$CG/memory.swap.max" > /dev/null || true
fi
PID=""
trap 'if [ -n "$PID" ]; then kill "$PID" 2> /dev/null || true; fi' EXIT
start() {
  # Each query with its time (the query layer logs it at debug), so a slow
  # board says which block is slow: see queries.txt.
  (
    if [ -n "$MEMORY" ]; then echo "$BASHPID" | sudo tee "$CG/cgroup.procs" > /dev/null; fi
    exec env RUST_LOG=info,openlens_query=debug OPENLENS_DATA_DIR="$D" OPENLENS_ADDR=127.0.0.1:18741 OPENLENS_ADMIN_TOKEN=perf "$BIN/openlens"
  ) >> server.log 2>&1 &
  PID=$!
  for _ in $(seq 120); do curl -sf 127.0.0.1:18741/healthz > /dev/null && break; sleep 1; done
}
# Idle: less than half a CPU-second over 5 s, for at most 30 min (the
# rollups of closed days run in the background after start). How long
# each wait took goes into scale.json (idle_seconds).
idles=()
idle() {
  local t0 a b
  t0=$(date +%s)
  while :; do
    a=$(awk '{print $14+$15}' "/proc/$PID/stat"); sleep 5; b=$(awk '{print $14+$15}' "/proc/$PID/stat")
    [ $(( b - a )) -lt 50 ] && break
    [ $(( $(date +%s) - t0 )) -gt 1800 ] && { echo "::warning::$EVENTS events, cap ${MEMORY:-none}: still busy after 30 min"; break; }
  done
  idles+=("$(( $(date +%s) - t0 ))")
}
start
# Diagnostic (#452): which rollups exist and which map version the board
# will compare them with, before and after the push.
api() { curl -s -H "authorization: Bearer perf" "127.0.0.1:18741$1"; }
{
  echo "map version before push: $(api /api/projects/default/feature-map | python3 -c 'import json,sys; print(json.load(sys.stdin).get("version"))')"
  R="$D/rollups/v1/project=default"
  echo "rolled day dirs: $(ls "$R" 2>/dev/null | wc -l)"
  for t in person_day session_day session_feature_day success_day; do
    echo "days with $t: $(ls "$R"/*/"$t".parquet 2>/dev/null | wc -l)"
  done
  echo "one day: $(ls "$R/$(ls "$R" | sort | tail -3 | head -1)" | tr '\n' ' ')"
} > diag.txt
OPENLENS_SERVER=http://127.0.0.1:18741 OPENLENS_TOKEN=perf "$BIN/openlens-cli" features push --project default --file "$GEN/map.yaml" > /dev/null
echo "map version after push: $(api /api/projects/default/feature-map | python3 -c 'import json,sys; print(json.load(sys.stdin).get("version"))')" >> diag.txt
idle
# Days read from rollups vs events for the 30 closed days before
# yesterday, as the board's own estimate counts them (D-061).
from=$(date -u -d '32 days ago' +%F); to=$(date -u -d '3 days ago' +%F)
echo "estimate $from..$to: $(api "/api/projects/default/board/estimate?from=$from&to=$to")" >> diag.txt
cat diag.txt
runs=()
for run in 1 2 3; do
  [ "$run" -gt 1 ] && { kill "$PID"; wait "$PID" || true; start; idle; }
  C=$(curl -si -XPOST 127.0.0.1:18741/login -d token=perf | tr -d '\r' | sed -n 's/^[Ss]et-[Cc]ookie: \(ol_session=[^;]*\).*/\1/p')
  cpu0=$(awk '{print $14+$15}' "/proc/$PID/stat")
  cold=$(curl -s -o cold.html -w '%{http_code} %{time_total}' -H "cookie: $C" 127.0.0.1:18741/p/default)
  cpu1=$(awk '{print $14+$15}' "/proc/$PID/stat")
  # A board that failed still answers 200 (its page streams), and is not
  # cached: the warm load then computes it again (D-071).
  if grep -q "t show that" cold.html; then echo "::warning::$EVENTS events, cap ${MEMORY:-none}, run $run: the board failed, see server.log"; fi
  warm=$(curl -s -o /dev/null -w '%{time_total}' -H "cookie: $C" 127.0.0.1:18741/p/default)
  peak=$(awk '/VmHWM/ {print $2}' "/proc/$PID/status")
  runs+=("{\"status\": ${cold%% *}, \"board_cold_s\": ${cold##* }, \"board_warm_s\": $warm, \"cold_cpu_s\": $(( (cpu1 - cpu0) ))e-2, \"peak_rss_mb\": $(( peak / 1024 ))}")
done
role=$(curl -s -o /dev/null -w '%{time_total}' -H "cookie: $C" "127.0.0.1:18741/p/default?role=admin")
# Four filtered boards at once (#418): they should share one snapshot.
( for r in member viewer owner guest; do
    curl -s -o /dev/null -w '%{time_total}\n' -H "cookie: $C" "127.0.0.1:18741/p/default?role=$r" &
  done; wait ) > concurrent.txt
concurrent_max=$(sort -n concurrent.txt | tail -1)
disk=$(du -sm "$D" | cut -f1)
kill "$PID"; wait "$PID" || true; PID=""
rm -rf "$D"
printf '%s\n' "${runs[@]}" > runs.jsonl
python3 - "${GEN_SECONDS:-}" "$role" "$disk" "$concurrent_max" "$MEMORY" "$PROFILE" "$CACHED" "${idles[*]}" "$GEN/gen.json" <<'PY' > scale.json
import json, sys
gen, role, disk, conc, mem, profile, cached, idles, gen_json = sys.argv[1:]
runs = [json.loads(l) for l in open("runs.jsonl") if l.strip()]
g = json.load(open(gen_json))
# bench-gen also writes the rollups of closed days (D-046): report that
# time apart, so generation times stay comparable. From the cache, both
# are those of the run that generated the data, in "generator".
rolled = float(g.get("rollup_seconds") or 0)
cached = cached == "true"
print(json.dumps({
    "generator": g, "profile": profile or "default",
    "memory_cap": mem or None,
    "generate_seconds": None if cached or not gen else int(gen),
    "cached": cached,
    "rollup_seconds": rolled, "days_rolled": g.get("days_rolled"),
    "idle_seconds": [int(s) for s in idles.split()],
    "runs": runs, "board_role_filter_cold_s": float(role),
    "four_filtered_boards_max_s": float(conc), "data_dir_mb": int(disk),
}, indent=2))
PY
cat scale.json
# The 25 slowest queries of this cap, longest first.
sed 's/\x1b\[[0-9;]*m//g' server.log | grep ' query ' | sed -n 's/.* ms=\([0-9]*\) sql=\(.*\)/\1\t\2/p' | sort -rn | head -25 > queries.txt || true
rm -f runs.jsonl concurrent.txt cold.html
{ echo "## Board at scale: $EVENTS events, cap ${MEMORY:-none}"; echo; echo '```json'; cat scale.json; echo '```'; echo '```'; cat diag.txt; echo '```'; } >> "$GITHUB_STEP_SUMMARY"
