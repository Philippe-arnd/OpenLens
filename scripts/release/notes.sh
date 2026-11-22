#!/usr/bin/env bash
# Release notes for a tag: docs/releases/<tag>.md when it exists, else the
# CHANGELOG section of that version (or "Unreleased" for a snapshot). Links
# are made absolute (they are read on the release page), and a "Full
# changelog" compare link to the previous release is appended.
set -euo pipefail
tag="$1"            # vX.Y.Z, or vX.Y.Z-YYMMDD-sha for a snapshot
prev="${2:-}"       # previous release tag, if any
repo="https://github.com/Philippe-arnd/OpenLens"
version="${tag#v}"
if [ -f "docs/releases/$tag.md" ]; then
  body=$(sed -e "s#](\.\./\.\./#]($repo/blob/$tag/#g" -e "s#](\.\./#]($repo/blob/$tag/docs/#g" "docs/releases/$tag.md")
else
  section="$version"
  case "$version" in *-*) section="Unreleased" ;; esac
  body=$(awk -v s="## [$section]" 'index($0, s) == 1 {on=1; next} on && /^## \[/ {exit} on {print}' CHANGELOG.md \
    | sed -e "s#](docs/#]($repo/blob/$tag/docs/#g" -e "s#](sdk/#]($repo/blob/$tag/sdk/#g")
  [ -n "${body//[[:space:]]/}" ] || body="See the [CHANGELOG]($repo/blob/$tag/CHANGELOG.md)."
fi
printf '%s\n' "$body"
cat <<EOF

## Install

- **Image:** \`ghcr.io/philippe-arnd/openlens:$tag\`
- **Binaries:** attached below (Linux x86_64), with \`SHA256SUMS\`. Check them with \`sha256sum -c SHA256SUMS\`.
EOF
if git cat-file -e "$tag:deploy/helm/openlens/Chart.yaml" 2>/dev/null; then
  printf -- '- **Helm:** `helm install openlens oci://ghcr.io/philippe-arnd/charts/openlens --version %s`\n' "$version"
fi
if [ -n "$prev" ]; then
  printf '\n**Full changelog:** %s/compare/%s...%s\n' "$repo" "$prev" "$tag"
fi
