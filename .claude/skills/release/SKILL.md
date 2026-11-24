---
name: release
description: Cut, hold, hotfix, check or roll back an OpenLens release. Use when asked to release, to write or edit release notes, to stop Tuesday's train, to ship a hotfix on an older minor, or to check that a release is really out.
---

# Releasing OpenLens

Releases are automatic. Act only when a person asks, and prefer the
train over doing it by hand. Rules of the repository (AGENTS.md) apply:
PRs only on `main`, no deploy of a production instance without the owner.

## How a release happens by itself
Every Tuesday at 06:17 UTC, `.github/workflows/release-train.yml` releases
the version in `Cargo.toml` when `[Unreleased]` in `CHANGELOG.md` has
entries, the last nightly CI and image runs on `main` are green, and no
open issue is labelled `release-blocker` or `release-hold`. It tags
`main`, starts `publish.yml`, `release.yml` and `k8s.yml`, then opens and
merges a PR that names the version in the CHANGELOG and moves `main` to
the next minor. After the release is published, `screenshots.yml`
regenerates the README and site images in a PR, and `pages.yml` rebuilds
the product page.

## Ask: "release now" / "what would be released?"
1. `gh workflow run release-train.yml -f dry_run=true`, then read its notice: it says why it would or would not release.
2. To release now: `gh workflow run release-train.yml -f dry_run=false`.

## Ask: "hold the release"
Open (or label) an issue with `release-hold` and say why. Remove the label to let the next Tuesday go. Use `release-blocker` for a bug that must be fixed first.

## Notes
- Patch release: the CHANGELOG section is the notes. Nothing to write.
- Minor or major: add `docs/releases/vX.Y.Z.md` in a PR before Tuesday, in the house style of the existing files there: a one-sentence promise in bold, *Upgrade now if…* when a bug fix matters, Highlights (bullets, one idea each), *Changes in behaviour*, *Install or upgrade*, *Known limitations*. Plain words, short sentences, no marketing superlatives, no analytics vendor names (clean room).
- Every merged change already adds its line under `[Unreleased]`; the notes regroup those lines, they do not invent new claims.
- A draft arrives by itself on Monday (`release-notes.yml`, PR "Release notes for vX.Y.Z (draft)"): edit that PR rather than starting from nothing. `scripts/release/draft-notes.sh X.Y.Z` prints the same draft locally.

## Hotfix on an older minor
1. `git checkout -b release/X.Y vX.Y.Z` (the last tag of that minor), cherry-pick the fix, set the version to `X.Y.(Z+1)` in `Cargo.toml` and the SDK manifests, add a CHANGELOG section.
2. Push the branch, tag `vX.Y.(Z+1)` on it, then run *Release* (`publish.yml`) with `tag` set, `release.yml` and `k8s.yml` on the tag (a tag pushed with a workflow token starts nothing).

## A release is done when
- the GitHub release `vX.Y.Z` exists, not a pre-release, with binaries, SDK bundles, chart and `SHA256SUMS`;
- `ghcr.io/philippe-arnd/openlens:vX.Y.Z` and `:X.Y` exist, and the `Image` run (with the upgrade and rollback check) is green;
- the chart `oci://ghcr.io/philippe-arnd/charts/openlens --version X.Y.Z` is published (`k8s.yml` green);
- the "Release vX.Y.Z, develop …" PR merged (opened by the workflow token, it needs a close/reopen to start its checks and the code owner's approval): it also moves the chart's `appVersion` and the Kustomize pins to `vX.Y.Z` (plain-manifest users install that tag). By hand (hotfix): edit `Chart.yaml` and `deploy/k8s/example`, then `scripts/render-k8s.sh`;
- the Pages site shows `vX.Y.Z`;
- the screenshots PR merged (or says the images did not change);
- `main` is on the next minor (`Cargo.toml`) and the CHANGELOG names the version.
Check with `scripts/release/summary.sh vX.Y.Z` (the train also runs it an hour after tagging, as `release-summary.yml`): every row ✅, or say which is not. Report what is missing instead of calling it done.

## Roll back
- Running installs follow `:X.Y` or `:latest`: point them back at the previous tag (`vX.Y.(Z-1)`); the data is compatible both ways within a minor (the upgrade check proves it), and the first start of a new version left a copy in `backups/upgrade/`.
- Never delete a published tag or image. Fix forward with a patch release.
