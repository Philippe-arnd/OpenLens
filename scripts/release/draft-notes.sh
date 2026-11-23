#!/usr/bin/env bash
# Draft docs/releases/vX.Y.Z.md for the next release (#258), in the house
# template: highlights from the CHANGELOG's [Unreleased], changes in
# behaviour from its Changed/Removed/Fixed parts, install, known limits and
# thanks (authors of the PRs merged since the last tag). Deterministic, no
# LLM: a person (or an agent) edits the PR before Tuesday's train.
# Usage: draft-notes.sh <version> [<changelog>] [<pr titles file>]
set -euo pipefail
v="$1"
changelog="${2:-CHANGELOG.md}"
prs="${3:-}"   # optional: lines "title<TAB>author", else read with gh
section() { # the [Unreleased] subsection $1 (Added, Changed...)
  awk -v want="### $1" '
    /^## \[Unreleased\]/ {u=1; next}
    u && /^## \[/ {exit}
    u && /^### / {on = ($0 == want); next}
    u && on {print}' "$changelog" | sed '/^[[:space:]]*$/d'
}
added=$(section Added)
changed=$(section Changed; section Removed)
fixed=$(section Fixed)
if [ -z "$prs" ] && command -v gh > /dev/null; then
  last=$(git tag --list 'v*' --sort=-v:refname | grep -v -- - | head -1 || true)
  since=$(git log -1 --format=%cI "$last" 2>/dev/null || echo 1970-01-01)
  prs=$(mktemp)
  gh pr list --state merged --base main --limit 200 --search "merged:>$since" \
    --json title,author -q '.[] | "\(.title)\t\(.author.login)"' > "$prs" || true
fi
thanks=""
if [ -n "$prs" ] && [ -s "$prs" ]; then
  thanks=$(cut -f2 "$prs" | grep -vE '^(app/|renovate|dependabot|github-actions)' | sort -u \
    | sed 's/^/`/; s/$/`/' | paste -sd, - | sed 's/,/, /g')
fi
cat <<MD
# 🔭 OpenLens v$v

<!-- Draft from the CHANGELOG (scripts/release/draft-notes.sh). Edit freely:
     the release train uses this file as it is on Tuesday. -->

## ✨ Highlights

${added:-- (nothing under Added)}

## 🚀 Install or upgrade

- **Image:** \`ghcr.io/philippe-arnd/openlens:v$v\` (also \`$v\` and \`${v%.*}\`); a running install upgrades in place and takes a backup first.
- **Binaries:** attached to the release (Linux x86_64 and aarch64), with \`SHA256SUMS\`.
- **Helm:** \`helm upgrade --install openlens oci://ghcr.io/philippe-arnd/charts/openlens --version $v\`

MD
if [ -n "$changed" ]; then
  printf '## ⚠️ Changes in behaviour\n\n%s\n\n' "$changed"
fi
if [ -n "$fixed" ]; then
  printf '## 🐛 Fixed\n\n%s\n\n' "$fixed"
fi
cat <<MD
## 🧭 Known limitations

- See the open issues labelled \`bug\` and docs/gaps.md.

MD
if [ -n "$thanks" ]; then
  printf '## 🙏 Thanks\n\nTo everyone whose changes are in this release: %s.\n' "$thanks"
fi
