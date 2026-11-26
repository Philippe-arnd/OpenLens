# Prompt for Claude Code

Copy the `docs/design/ui-v2/` folder into the repository, then paste the text below into Claude Code. Run it once per PR in the plan, changing only the PR number.

---

Read `AGENTS.md`, then `docs/design/ui-v2/README.md`, and look at the mockups in `docs/design/ui-v2/mockups/` (the `.html` files and the `.light.png`/`.dark.png` captures).

Do **PR 1** of the plan in section 7 of the README, and only that one.

1. Search the issues, the PRs and recent commits for the same work (rule 4). If there is no issue, open one titled like the plan line, and claim it.
2. Work on a branch named `ui-v2/pr-01-<short-name>`.
3. Follow the README's constraints. They are strict:
   - no client-side JavaScript;
   - no `style=` attribute and no `<script>` in any HTML (the CSP blocks them): widths go in SVG attributes or bucketed classes;
   - no new setting;
   - colour tints use `color-mix(in oklab, …)`;
   - every animation sits inside `prefers-reduced-motion: no-preference`.
4. `mockups/v2.css` is a reference, not a file to paste: it scopes everything under `.ol` for the design canvas. Port what the PR needs into `crates/ui/assets/app.css` with the selectors described in section 3.1, and keep the existing class names working until the screens that use them are migrated.
5. When you are done:
   - run `cargo xtask demo ./data`;
   - start the server;
   - open every screen you touched in light and dark, at 1440 px and 390 px;
   - compare them with the mockups;
   - run `cargo xtask ci`.
6. Open the PR. List what ran and what did not, put before/after screenshots of the touched screens in it, and stop. Do not merge.

If something in the README conflicts with `AGENTS.md` or with the code as it is, `AGENTS.md` wins: say so in the PR and do not work around it.
