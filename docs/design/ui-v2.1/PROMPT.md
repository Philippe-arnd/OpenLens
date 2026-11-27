# Prompt for Claude Code (v2.1)

Copy `docs/design/ui-v2.1/` into the repository, then paste this, changing the ticket id each time:

---

Read `AGENTS.md`, `docs/design/ui-v2.1/README.md` and `docs/design/ui-v2.1/TICKETS.md`, and look at `docs/design/ui-v2.1/mockups/` (the HTML files and the light and dark captures).

Do ticket **UI21-01**, and only that one.

1. Search the issues and PRs for it (rule 4). If it is not open yet, open it with the title, labels and body from TICKETS.md, then claim it.
2. Branch `ui21/<ticket-id>-<short-name>`.
3. The `sample` values in the mockups are made up: never hard-code them. Read every number from the engine.
4. The constraints are the same as v2: no client-side JS, no `style=` attribute (CSP), no new setting, our own wording.
5. Run `cargo xtask demo ./data`, open the touched screens in light and dark at 1440 and 390 px, compare them with the mockups, and run `cargo xtask ci`.
6. Open the PR with before/after screenshots and what ran. Do not merge.

If the ticket conflicts with `AGENTS.md` or with the code as it is, say so in the PR and stop.
