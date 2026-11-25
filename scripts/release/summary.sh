#!/usr/bin/env bash
# One table for a release (#258): did every part of it land? Each check
# prints a row; a check whose input is not configured is "skipped", never a
# failure. Exit 1 when a check failed.
# Usage: summary.sh <tag>   (GH_TOKEN, GITHUB_REPOSITORY; optional
# OPENLENS_PROD_URL: a running instance that should be on the version)
set -uo pipefail
tag="$1"
v="${tag#v}"
repo="${GITHUB_REPOSITORY:-Philippe-arnd/OpenLens}"
fail=0
row() { printf '| %s | %s | %s |\n' "$1" "$2" "$3"; [ "$2" = "❌ failed" ] && fail=1; return 0; }
run_of() { # conclusion of the latest run of a workflow for the tag
  gh run list -R "$repo" --workflow "$1" --limit 100 --json conclusion,headBranch,displayTitle \
    -q "[.[] | select(.headBranch == \"$tag\" or (.displayTitle | contains(\"$tag\")))][0].conclusion // \"\""
}
echo "| Check | Result | Detail |"
echo "|---|---|---|"
if gh release view "$tag" -R "$repo" --json isDraft,isPrerelease,assets \
     -q '"assets: \(.assets | length)"' > /tmp/rel.txt 2>/dev/null; then
  row "GitHub release" "✅ ok" "$(cat /tmp/rel.txt)"
else
  row "GitHub release" "❌ failed" "no release $tag"
fi
for wf in publish.yml release.yml k8s.yml; do
  c=$(run_of "$wf")
  case "$c" in
    success) row "$wf" "✅ ok" "run for $tag" ;;
    "") row "$wf" "⏳ not found" "no run for $tag yet" ;;
    *) row "$wf" "❌ failed" "$c" ;;
  esac
done
# The public image, read like a registry client would (no docker needed).
ghcr_token=$(curl -fsS --max-time 20 "https://ghcr.io/token?scope=repository:philippe-arnd/openlens:pull" \
  | sed -n 's/.*"token":"\([^"]*\)".*/\1/p' || true)
if [ -n "$ghcr_token" ] && curl -fsSI --max-time 20 -H "Authorization: Bearer $ghcr_token" \
     -H "Accept: application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.docker.distribution.manifest.v2+json" \
     "https://ghcr.io/v2/philippe-arnd/openlens/manifests/$tag" > /dev/null; then
  row "Image" "✅ ok" "ghcr.io/philippe-arnd/openlens:$tag"
else
  row "Image" "❌ failed" "ghcr.io/philippe-arnd/openlens:$tag not found"
fi
pages=$(gh api "repos/$repo/pages" -q .html_url 2>/dev/null || true)
if [ -n "$pages" ]; then
  if curl -fsSL --max-time 20 "$pages" | grep -q "$v"; then
    row "Site" "✅ ok" "$pages names $v"
  else
    row "Site" "❌ failed" "$pages does not name $v yet"
  fi
  # The images the site serves are the ones on main (#439): Pages builds
  # from main, after the screenshots PR of the release merges. A warning,
  # not a failure: Pages and its CDN can lag a few minutes.
  shots=$(curl -fsSL --max-time 20 "$pages" | grep -o 'img/[A-Za-z0-9_.-]*\.png' | sort -u || true)
  stale=""
  for img in $shots; do
    served=$(curl -fsSL --max-time 20 "${pages%/}/$img" | sha256sum | cut -c1-64)
    want=$(curl -fsSL --max-time 20 "https://raw.githubusercontent.com/$repo/main/docs/$img" | sha256sum | cut -c1-64)
    [ "$served" = "$want" ] || stale="$stale ${img#img/}"
  done
  if [ -z "$shots" ]; then
    row "Images" "⚠️ warning" "the site names no image"
  elif [ -n "$stale" ]; then
    row "Images" "⚠️ warning" "differ from docs/img on main:$stale"
  else
    row "Images" "✅ ok" "$(wc -w <<< "$shots") images match docs/img on main"
  fi
else
  row "Site" "⏭️ skipped" "no Pages site"
fi
if [ -n "${OPENLENS_PROD_URL:-}" ]; then
  got=$(curl -fsS --max-time 20 "${OPENLENS_PROD_URL%/}/version" | sed -n 's/.*"version":"\([^"]*\)".*/\1/p' || true)
  if [ "$got" = "$v" ]; then
    row "Reference instance" "✅ ok" "runs $got"
  else
    row "Reference instance" "❌ failed" "runs ${got:-unknown}, expected $v"
  fi
else
  row "Reference instance" "⏭️ skipped" "set the OPENLENS_PROD_URL variable to check it"
fi
exit "$fail"
