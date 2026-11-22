# Development

## Requirements
- Rust stable (pinned by `rust-toolchain.toml`)
- Optional: `cargo-deny` (CI always runs it; `cargo xtask ci` says when it skipped it)
- Node 22+ for `sdk/js` (only to rebuild the SDK; the built file is committed)

## Everyday commands
```sh
cargo xtask ci          # the full gate
cargo xtask layers      # layering only
cargo run -p openlens   # OPENLENS_ADDR=0.0.0.0:8686 by default
cargo xtask demo ./data # 300 synthetic users with planted patterns, to look at the board
OPENLENS_DATA_DIR=./data OPENLENS_ADMIN_TOKEN=demo cargo run -p openlens  # then log in with "demo"
cargo build --profile perf -p openlens   # optimised, no LTO: use for timing (debug is ~5x slower)
cargo xtask bench http://127.0.0.1:PORT KEY   # ingestion load test, see docs/benchmarks.md
```

## CI
- **PR gate** (about 3 min): `cargo xtask ci` plus the SDK build, size budget and freshness. Browser e2e runs on PRs only when the SDK or capture changes.
- **Nightly and on demand** (`gh workflow run ci.yml`):
  - the browser e2e;
  - every dashboard page in Chromium, light and dark, under the real CSP, which fails on any console error;
  - the reference apps in `examples/`, which must reach a green `verify`.
- The screenshots are in the `dashboard-screens` artifact: `gh run download <run-id> -n dashboard-screens`.
- **Image** (`release.yml`): Docker build, compose, smoke test, then publish to GHCR. It runs nightly, on tags, on demand, or when packaging changes.

## Docker
```sh
docker compose up --build    # http://localhost:8686/healthz
```
The healthcheck lives in `docker-compose.yml`, not in the Dockerfile, so Coolify picks it up.

## Releasing

OpenLens follows [SemVer](https://semver.org) and keeps a [CHANGELOG](../CHANGELOG.md) in the Keep a Changelog format. Every merged change adds its line under `Unreleased`.

**A release (vX.Y.Z):**
1. In a PR:
   - move the `Unreleased` entries of `CHANGELOG.md` under the new version and date;
   - set the version in the `Cargo.toml` workspace, `sdk/js/package.json`, `sdk/node/package.json` and `sdk/python`;
   - add `docs/releases/vX.Y.Z.md` (highlights, install, breaking changes, known limits, thanks) for a minor or major release.
2. Merge it, then tag `main`: `git tag -a vX.Y.Z -m "OpenLens vX.Y.Z" && git push origin vX.Y.Z`.
3. The tag starts, with nothing more to do:
   - `release.yml`: the image `ghcr.io/philippe-arnd/openlens:vX.Y.Z` (also `X.Y.Z` and `X.Y`), built and smoke-tested;
   - `k8s.yml`: the chart, installed on kind, then pushed to `oci://ghcr.io/philippe-arnd/charts/openlens`;
   - `publish.yml`: the GitHub release, with the notes (`docs/releases/vX.Y.Z.md`, or the CHANGELOG section, plus a *Full changelog* link), the Linux binaries, the SDK bundles, the chart and `SHA256SUMS`;
   - `npm.yml`: `@openlens/web` (public repository only).

**Upgrades are checked** before any image is published: `scripts/upgrade-check.sh <last release image> <new image>` (run by `release.yml`) writes data with the last release and reads it back with the new image. A change that has to rewrite stored data needs its own migration and a CHANGELOG *Changed* entry.

**Right after a release**, set the `Cargo.toml` workspace version to the next one being developed (after `v0.1.0`: `0.2.0`), so build releases are named after it (`v0.2.0-…` sorts after `v0.1.0`).

**Build releases.** Every push to `main` that changes code publishes a build release named **`vX.Y.Z-YYMMDD-sha`**: `X.Y.Z` is the version being developed (`Cargo.toml`), `YYMMDD` the UTC date, `sha` the commit (for example `v0.2.0-261122-a1b2c3d`). It is a valid SemVer pre-release of `X.Y.Z`, so npm, Helm and image tags accept it.
- It is marked *pre-release* until `vX.Y.Z` itself is tagged, and it is not published to npm.
- It has the same assets as a release, with the `Unreleased` notes and a *Full changelog* link to the previous build.
- The image `ghcr.io/philippe-arnd/openlens:vX.Y.Z-YYMMDD-sha` and the chart are published for it too.
- A newer push cancels an older build that is still running. Docs-only pushes make no build.
- *Release* → *Run workflow* with `snapshot` ticked makes one now.

**Re-attach assets** to an existing tag: run *Release* by hand with `tag` set.

**npm, once.** On npmjs.com, add a *trusted publisher* for `@openlens/web`: repository `Philippe-arnd/OpenLens`, workflow `npm.yml`. No token is needed; `NPM_TOKEN` as a repository secret also works.

