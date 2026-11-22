#!/usr/bin/env bash
# M2 exit check (D-013): each reference app, wired by following
# docs/agents/wire.md, reaches a lint-clean map and a green verify with every
# feature fired. Needs a running server at $OPENLENS_SERVER (token in
# $OPENLENS_TOKEN), built binaries in target/debug, and Playwright next to
# scripts/e2e.
set -euo pipefail
CLI=./target/debug/openlens-cli
port=3101
for app in notes-ssr tasks-spa; do
  dir=examples/$app
  created=$($CLI init --name "$app")
  project=$(jq -r .id <<<"$created")
  key=$(jq -r .key <<<"$created")
  (cd "$dir" && ../../$CLI features lint --src . > /dev/null)
  $CLI features push --project "$project" --file "$dir/openlens.features.yaml" > /dev/null
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
  port=$((port + 1))
done
echo "reference apps: green"
