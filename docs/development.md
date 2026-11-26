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

## Coverage and mutants

`coverage.yml` (#313) measures line coverage on every push to `main` and on demand (`cargo llvm-cov`). The job summary has the table per crate, with `engine` and `storage` first, and the JSON is an artifact. It reports only: a floor will be set once the numbers are known. Every Sunday, `cargo mutants -p openlens-engine` lists the code changes no test catches in one rolling issue, *Surviving mutants (engine)*.

## Crash test

`cargo xtask crash` proves durability (#331). It starts the real server (`target/debug/openlens`, so build it first; `--bin` for another), ingests batches with known uuids, SIGKILLs the server at seeded random intervals and restarts it. It resends every batch that got no 200, as the SDK does. At the end it reads the stored uuids back through the SQL API. It passes when no acknowledged event is missing, none is stored twice, and the run reached its kill count:

```sh
cargo build -p openlens
cargo xtask crash                          # 5 minutes, at least 50 kills
cargo xtask crash --secs 60 --kills 10     # quick local run
cargo xtask crash --seed 1234              # replay a failure (the seed is in its report)
```

It prints a JSON report. On failure, it keeps the data directory and the server log in the temp directory. The run does not shorten the 10-minute compaction interval (no setting, D-014), so a 5-minute run never kills the server during a compaction.

## Fuzzing

`fuzz/` holds coverage-guided targets for what strangers send (#299): `capture_batch` (a `/capture` body after decompression), `feature_map` (a pushed map), `error_stack` (a browser stack) and `page_url` (URL cleaning). It is not a workspace member; it needs a nightly toolchain and `cargo-fuzz`:

```sh
cd fuzz
cargo run --bin seed                                   # corpus from crates/testkit's hostile inputs
cargo +nightly fuzz run capture_batch -- -max_total_time=600
```

A crash leaves its input in `fuzz/artifacts/<target>/`. Fix it, and add that input to `crates/testkit` (rule 27).

## Docker
```sh
docker compose up --build    # http://localhost:8686/healthz
```
The healthcheck lives in `docker-compose.yml`, not in the Dockerfile, so Coolify picks it up.

## Releasing

Agents: the [`release` skill](../.claude/skills/release/SKILL.md) is the checklist (release now, hold, notes, hotfix, done, roll back).

OpenLens follows [SemVer](https://semver.org) and keeps a [CHANGELOG](../CHANGELOG.md) in the Keep a Changelog format. Every merged change adds its line under `Unreleased`. A change that asks the operator or an app to act starts with **Breaking:** and links [the stability policy](stability.md).

**The release train** (`.github/workflows/release-train.yml`, #197) cuts stable releases by itself every Tuesday at 06:17 UTC. It releases the version in `Cargo.toml` when:
- `[Unreleased]` has entries;
- the last nightly CI and image runs on `main` are green;
- no open issue is labelled `release-blocker` or `release-hold`.

It then:
1. tags `main` as it is (branch protection only takes PRs on `main`; tags are free), so the notes come from `[Unreleased]`;
2. starts the publish, image and chart workflows;
3. opens a PR that moves `[Unreleased]` under the version and the date and sets `main` to the next minor version, and merges it. If the workflow token may not open PRs (Settings → Actions → *Allow GitHub Actions to create pull requests*) and no `RELEASE_TOKEN` secret is set (a fine-grained token with contents and pull requests: write), it opens a `release-hold` issue instead, and the branch waits to be merged by hand: open its PR, let CI pass, merge, close the issue. A PR opened with the workflow token starts no `pull_request` checks (close and reopen it to start them), and since it touches `deploy/` and `Cargo.toml` it needs the code owner's approval (CODEOWNERS); a `RELEASE_TOKEN` of the owner avoids both.

Run it by hand from the Actions tab: `dry_run` (on by default) only reports what it would do. After a stable release, **`screenshots.yml`** retakes the README and site images on demo data (`scripts/e2e/readme-shots.mjs`, 1440×900 ×2 and a phone, optimised with oxipng) and opens a PR with them, which merges itself and rebuilds Pages. Run it by hand to refresh them now. Each view waits for its content, the fonts and the icons, and the run fails on a page that is missing, still loading or shows an alert; a new view is one line in `VIEWS`. Images are referenced as `name.png?v=<hash>` (`scripts/site/stamp.py`; `--check` reports stale references), so browsers, GitHub's image proxy and the Pages CDN fetch a new picture under a new URL. The release summary's *Images* row compares what the site serves with `docs/img` on `main`. The screenshots PR lists the images that changed (more than 0.5% of pixels beyond a 2% colour fuzz, so moving dates and counts on demo data are ignored), with links to the image before and after.

**Notes draft** (`release-notes.yml`, Mondays 15:43 UTC): when the next version has no `docs/releases/vX.Y.Z.md` and `[Unreleased]` has entries, a PR adds a draft in the house template, made from the CHANGELOG (`scripts/release/draft-notes.sh`, no LLM). It does not merge itself: reword the highlights, then merge before Tuesday and the train publishes it as the notes. Without it, the notes come from the CHANGELOG.

**Release summary** (`release-summary.yml`): an hour after tagging, the train starts a check of every part of the release (`scripts/release/summary.sh`): the GitHub release and its assets, the publish, image and chart runs, the image on GHCR, the site naming the version, and the reference instance's `GET /version` when the `OPENLENS_PROD_URL` repository variable is set. It is the run summary, and a comment on the release PR. Run it by hand with any tag.

**A release (vX.Y.Z):** (by hand, for a hotfix or when the train is held)
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

**Upgrades are checked** before any image is published: `scripts/upgrade-check.sh <last release image> <new image>` (run by `release.yml`) writes data with the last release, reads it back with the new image, then goes back to the last release (a rollback) and reads it again. A change that has to rewrite stored data needs its own migration and a CHANGELOG *Changed* entry.

**Right after a release**, set the `Cargo.toml` workspace version to the next one being developed (after `v0.1.0`: `0.2.0`), so build releases are named after it (`v0.2.0-…` sorts after `v0.1.0`).

**Build releases.** Every push to `main` that changes code publishes a build release named **`vX.Y.Z-YYMMDD-sha`**: `X.Y.Z` is the version being developed (`Cargo.toml`), `YYMMDD` the UTC date, `sha` the commit (for example `v0.2.0-261122-a1b2c3d`). It is a valid SemVer pre-release of `X.Y.Z`, so npm, Helm and image tags accept it.
- It is marked *pre-release* until `vX.Y.Z` itself is tagged, and it is not published to npm.
- It has the same assets as a release, with the `Unreleased` notes and a *Full changelog* link to the previous build.
- The image `ghcr.io/philippe-arnd/openlens:vX.Y.Z-YYMMDD-sha` and the chart are published for it too.
- A newer push cancels an older build that is still running. Docs-only pushes make no build.
- *Release* → *Run workflow* with `snapshot` ticked makes one now.

**Re-attach assets** to an existing tag: run *Release* by hand with `tag` set.

**npm, once.** On npmjs.com, add a *trusted publisher* for `@openlens/web`: repository `Philippe-arnd/OpenLens`, workflow `npm.yml`. No token is needed; `NPM_TOKEN` as a repository secret also works.


## Maintenance on autopilot (M9)

Nobody should have to babysit dependencies:
- **Renovate** (`renovate.json`) opens grouped PRs every Monday morning, Paris time, and security fixes at once. DataFusion, Arrow and Parquet move together; the other non-major Cargo updates, the npm ones and the GitHub Actions each come as one PR (a 0.x minor of a crate comes apart, since it is reviewed). New releases wait 3 days before they are proposed.
- **Auto Merge** (`.github/workflows/auto-merge.yml`, the shared `reusable-auto-merge.yml`) merges a Renovate PR once `cargo xtask ci`, `sdk` and the RustSec check are green, and the browser e2e too when it ran. A failing PR gets one issue labelled `renovate-ci-failure`, closed when the PR is fixed. A major update, including a 0.x minor of a crate, gets the `major-update` label and waits for a person or an agent.
- **Lockfile maintenance merges itself too** (#349): the weekly `Cargo.lock` refresh goes through the same gate, after the 3-day wait. A signed image or binary proves where it was built, not that each transitive update was read; the defences are the wait, RustSec on every PR and nightly, `cargo deny` licences, and the stable tag, which still needs the release train's checks. A human review there would put a person back in a loop M9 removes.
- **RustSec:** `cargo deny check advisories` runs on every PR and nightly. On `main` it keeps one `security-advisory` issue open while it fails. An ignore in `deny.toml` needs its reason, and `advisory-ignores.yml` checks every ignored advisory again on the 1st of each month: one that now has a fix opens an issue (#196).
