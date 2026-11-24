# Contributing

Read [`AGENTS.md`](../AGENTS.md) first. It applies to humans and agents alike, and it wins over this page.

## The short version

1. Pick an issue in the current milestone (see [`ROADMAP.md`](../ROADMAP.md)) and comment to claim it.
2. Branch from `main`, keep the PR to one issue, and put `Closes #N` in the commit.
3. Run `cargo xtask ci` before you push. It is the same gate CI runs.
4. In the PR (the [template](../.github/pull_request_template.md) asks for it): what changed, why, what you ran, and what you could not run.
5. Add one line under `Unreleased` in [`CHANGELOG.md`](../CHANGELOG.md).

## Environment

- **Rust:** the toolchain is pinned by `rust-toolchain.toml`, so `rustup` installs the right one on the first `cargo` command. The minimum supported Rust version is the `rust-version` in `Cargo.toml`.
- **Node 22+:** only to rebuild the browser SDK in `sdk/js`. The built file is committed, so you do not need Node to work on the server.
- **Optional:** `cargo-deny`. CI always runs it, and `cargo xtask ci` tells you when it skipped it.
- **Nothing else:** no database or service to install. The server keeps its data in one directory (Parquet files plus SQLite).

## What to expect from a build

The workspace pulls in DataFusion and Arrow, so the first build is the slow one.

| | Time | Disk |
|---|---|---|
| Cold debug build (`cargo build -p openlens -p xtask`), 4 vCPU, `jobs = 2` | about 6 min | about 2.5 GB in `target/` |
| Full gate after that (`cargo xtask ci`) | a few more minutes | about 6.5 GB, up to 13 GB over many rebuilds |
| CI PR gate (warm cache) | about 3 min | n/a |

- On a small machine, cap the parallel jobs (`CARGO_BUILD_JOBS=2`, or `[build] jobs = 2` in `~/.cargo/config.toml`). A full-width DataFusion build can use more than 1 GB of RAM per job.
- `target/` only grows: stale artifacts pile up across rebuilds. Run `cargo clean` once it passes about 6 GB.
- While you iterate, use `cargo check` and `cargo test -p <crate>`, and save the full gate for just before you push.
- For timing, build with `cargo build --profile perf -p openlens`: optimised and without LTO. A debug build is about 5 times slower, so its numbers mean nothing.

## Running it

The everyday commands (demo data, a local server, the benchmarks) are in [`docs/development.md`](development.md#everyday-commands). This page only links to them, so they do not drift.

## End-to-end tests

- **Browser e2e, dashboard screenshots under the real CSP, reference apps:** CI runs them nightly, and on PRs that touch the SDK or capture. To start them on demand: `gh workflow run ci.yml`. To get the screenshots: `gh run download <run-id> -n dashboard-screens`.
- **Locally:** the scripts are in `scripts/e2e/`. `browser.mjs` covers the SDK in a real browser, `screens.mjs` every dashboard page in light and dark (on `xtask demo` data), and `reference.sh` the reference apps in `examples/`. They need Node, a running server, and Playwright with Chromium installed in `scripts/e2e` (`npm install playwright@1` there, as CI does). The header of each script gives its arguments.
- **Image smoke test:** `scripts/smoke.sh` runs against a started container, as `release.yml` does.
- **Fuzzing:** see [`docs/development.md`](development.md#fuzzing). It needs a nightly toolchain. Prefer the nightly CI job over long local runs.

## Maintainers

OpenLens has a single maintainer, [@Philippe-arnd](https://github.com/Philippe-arnd). They review and merge every PR, cut releases (with the [release train](development.md#releasing)), and answer security reports (see [`SECURITY.md`](../SECURITY.md)).

## Support

- **Questions and bugs:** open a GitHub issue using a template.
- **Supported versions:** only the latest minor release gets fixes. See [`SECURITY.md`](../SECURITY.md#supported-versions).
- **Conduct:** [`CODE_OF_CONDUCT.md`](../CODE_OF_CONDUCT.md).
