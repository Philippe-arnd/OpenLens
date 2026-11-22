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

OpenLens follows [SemVer](https://semver.org) and keeps a [CHANGELOG](../CHANGELOG.md) in the Keep a Changelog format.

1. Move the `Unreleased` entries of `CHANGELOG.md` under the new version and date. Set the same version in `sdk/js/package.json`, and in the `Cargo.toml` workspace version.
2. Merge that through a PR as usual.
3. Tag `main`: `git tag -a vX.Y.Z -m "OpenLens vX.Y.Z" && git push origin vX.Y.Z`. The tag builds, smoke-tests and publishes the image `ghcr.io/philippe-arnd/openlens:vX.Y.Z` (`release.yml`), and publishes `@openlens/web` (`npm.yml`, public repo only).
4. Create the GitHub release from the tag, with the changelog section as notes: `gh release create vX.Y.Z --notes-file <notes>`.

**npm, once.** On npmjs.com, add a *trusted publisher* for `@openlens/web`: repository `Philippe-arnd/OpenLens`, workflow `npm.yml`. No token is needed; `NPM_TOKEN` as a repository secret also works.

