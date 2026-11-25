#!/usr/bin/env bash
# M2 exit check (D-013): each reference app, wired by following
# docs/agents/wire.md, reaches a lint-clean map and a green verify with every
# feature fired. Needs a running server at $OPENLENS_SERVER (token in
# $OPENLENS_TOKEN), built binaries in target/debug, and Playwright next to
# scripts/e2e.
#
# It also writes what it measured (#305) to $RUNNER_TEMP/reference.json, and
# a shields.io endpoint badge to $RUNNER_TEMP/badges/wiring.json that Pages
# publishes for the README: seconds from `init` to a pushed, lint-clean map
# (the agent's part), seconds to a green `verify`, features wired, and
# insight cards on the board's first display.
set -euo pipefail
CLI=./target/debug/openlens-cli
port=3101
results=()
for app in notes-ssr tasks-spa; do
  dir=examples/$app
  start=$(date +%s)
  created=$($CLI init --name "$app")
  project=$(jq -r .id <<<"$created")
  key=$(jq -r .key <<<"$created")
  (cd "$dir" && ../../$CLI features lint --src . > /dev/null)
  $CLI features push --project "$project" --file "$dir/openlens.features.yaml" > /dev/null
  wired=$(date +%s)
  if [ "$app" = tasks-spa ]; then (cd "$dir" && npm install --no-audit --no-fund > /dev/null && npm run build > /dev/null); fi
  script=server.mjs; [ "$app" = tasks-spa ] && script=serve.mjs
  OPENLENS_HOST=$OPENLENS_SERVER OPENLENS_KEY=$key PORT=$port node "$dir/$script" > "$RUNNER_TEMP/$app.log" 2>&1 &
  app_pid=$!
  for i in $(seq 1 20); do curl -fs "localhost:$port/" > /dev/null && break; sleep 0.5; done
  node scripts/e2e/reference.mjs "$app" "http://localhost:$port"
  kill "$app_pid"
  sleep 12 # the server flushes buffered events to Parquet every 10 s
  report=$($CLI verify --project "$project" || true)
  echo "$report" | jq -c '{app: "'"$app"'", ok, features}'
  jq -e '.ok == true and ([.features[] | .events_last_day > 0] | all)' <<<"$report" > /dev/null \
    || { echo "::error::$app: verify is not green"; echo "$report"; exit 1; }
  green=$(date +%s)
  insights=$(curl -fs -H "authorization: Bearer $OPENLENS_TOKEN" \
    "$OPENLENS_SERVER/api/projects/$project/board" | jq '.insights | length')
  results+=("$(jq -cn --arg app "$app" --argjson wire $((wired - start)) --argjson verify $((green - start)) \
    --argjson insights "$insights" --argjson report "$report" \
    '{app: $app, wire_seconds: $wire, verify_seconds: $verify, features: ($report.features | length), insights: $insights}')")
  port=$((port + 1))
done
echo "reference apps: green"
printf '%s\n' "${results[@]}" | jq -s --arg date "$(date -u +%F)" --arg commit "${GITHUB_SHA:-local}" \
  '{date: $date, commit: $commit, apps: .}' > "$RUNNER_TEMP/reference.json"
mkdir -p "$RUNNER_TEMP/badges"
jq '{schemaVersion: 1, label: "agent wiring",
     message: "\(([.apps[].wire_seconds] | max)) s · \([.apps[].features] | add) features · \([.apps[].insights] | min) insights at first login",
     color: "brightgreen"}' "$RUNNER_TEMP/reference.json" > "$RUNNER_TEMP/badges/wiring.json"
jq -c . "$RUNNER_TEMP/reference.json"
