# OpenLens UI v2: design handoff

This folder is the reference for the v2 UI. It holds:

- `mockups/*.html`: six reference screens, static HTML. Open them in a browser; they load `mockups/v2.css`. Two more files show an open state: `board-v2-period-open.html` (the period chip's popover) and `person-v2-forget-dialog.html` (the confirmation dialog).
- `mockups/*.light.png` and `mockups/*.dark.png`: full-page captures of the same screens, in each theme.
- `mockups/v2.css`: the reference stylesheet (tokens, layers, components).
- The live design canvas, with every current screen next to the proposals: https://claude.ai/artifact/TcC9zJP9wCAQjgJvcentce (pages "Focus board" and "V2 proposal").

The six reference screens are patterns. Every other screen is built from the same parts (section 6 maps each screen to its pattern).

| Mockup | Pattern it defines |
|---|---|
| `board-v2.html` | Dashboard: context bar, lead insight and ranked list, KPI tiles, chart card, data table, disclosures |
| `events-v2.html` | List page: refine form, active-filter line, data table with day groups, event pills, pager |
| `person-v2.html` | Detail page: entity header, summary cards, table, danger zone, confirmation dialog |
| `live-v2.html` | Real-time page and empty states: one explained empty state instead of many empty cards |
| `settings-v2.html` | Settings: setup steps, form card, help text under fields, form footer |
| `login-v2.html` | Entry pages without the shell |

---

## 1. Rules that do not move

Everything in `AGENTS.md` applies. These rules matter most for this work:

1. **No client-side JavaScript** (rule 12, D-011). Every interaction is a link, a form, or a native HTML feature: `popover`, `<dialog>`, `<details>`, `accesskey`, `<datalist>`. No JS island, not even a small one.
2. **The CSP stays strict.** `crates/server/src/web.rs` sends `default-src 'none'; style-src 'self'; img-src 'self' data:; form-action 'self'; frame-ancestors 'none'; base-uri 'none'`. Consequences:
   - **No `style="…"` attributes.** The mockups use inline styles for widths (bars, progress, heat cells) because they are mockups. In maud, render widths as SVG attributes, as `components.rs` already does (`svg.fill > rect width=…`). Cohort heat cells become SVG `rect`s with a `fill-opacity` attribute, or classes bucketed by tenths (`.p0`…`.p10`).
   - **No inline `<script>`**, so no speculation rules unless a decision allows them (see 4.4).
   - **Fonts need `font-src 'self'`** if we ship a webfont (see 3.3).
3. **No new configuration** (rule 17, D-014). The theme choice already exists (My profile → Appearance, cookie, `html[data-theme]`). The v2 UI adds no setting.
4. **One PR per issue** (rule 7). Follow the PR plan in section 7. Moving code and changing how it looks go in separate PRs.
5. **Clean room** (rules 20–22). The design is our own. Do not copy UI text, icons or layouts from other analytics products.
6. Before each PR, **run the real binary** on the demo data (`cargo xtask demo ./data`), open every screen you touched, in light and dark, at 1440 px and 390 px wide.

---

## 2. What changes, in one page

- **Shell.** The sidebar gets a project switcher that has a monogram fallback (no more broken favicon image), a "Go to…" field, the 8 sections with `accesskey` 1–8, and a user menu that replaces the four bottom links. Section tabs move into a bar under the top bar.
- **Context bar.** The period, environment, role, release and "Include the team" controls become one row of chips in a sticky top bar, on every analysis page. Each chip opens a popover holding a small GET form. This replaces the two filter forms (Show + Apply) and the help paragraph.
- **Signals.** The alert and setup banners become one line of compact pills.
- **Focus board.** The first insight is shown as a lead card (rank, severity tag, a before/after bar comparison, the next step, one primary action). Insights 2–5 become a ranked list. KPI tiles show a value, a coloured delta and what it is compared with. Secondary sections (cohorts, release comparison) are `<details>`.
- **Tables.** Day group rows, event pills coloured by family, monospace paths, links on users and features, a pager.
- **Detail pages.** An entity header with actions. Destructive actions move to a "Privacy" or "Danger" card at the bottom, and confirmation happens in a dialog.
- **Empty states.** One explained empty state per page, with the reason and one or two actions, instead of several cards saying "Nothing in the last 30 minutes".
- **Settings.** The setup checklist becomes numbered steps with a progress bar. Each form is a card with help text under its fields and a footer holding Save.
- **Entry pages.** Login and Welcome have no sidebar and no emoji.

---

## 3. Foundations

### 3.1 CSS architecture

Replace `crates/ui/assets/app.css` with the layered structure in `mockups/v2.css`:

```css
@layer tokens, base, layout, components, utilities;
```

- The mockup scopes everything under `.ol` (so it can live on the canvas). In the product, `.ol` becomes `:root`/`html`, and the `.ol .x` selectors become `.x`.
- **One palette.** Every colour is declared once with `light-dark()` in `oklch()`. The server picks the scheme only:

  ```css
  :root { color-scheme: light dark; }            /* follows the system */
  :root[data-theme="light"] { color-scheme: light; }
  :root[data-theme="dark"]  { color-scheme: dark; }
  ```

  This removes the palette that is declared three times today (`:root`, `[data-theme="dark"]`, `prefers-color-scheme`).
- **Tints use `color-mix(in oklab, …)`, never `in oklch`.** Mixing in oklch with white or a neutral swings the hue (violet turns pink). The reference file already does this.
- Keep what `app.css` does well: WCAG 1.4.1 underlines on links in text, 24 px minimum targets for small links, `tabular-nums`, `:focus-visible` rings, `prefers-reduced-motion`.
- Budget: the whole stylesheet stays under 12 kB gzipped. Serve it with a content hash in the URL and `Cache-Control: immutable`.

### 3.2 Tokens

| Token | Use |
|---|---|
| `--bg`, `--surface`, `--raised` | page, cards, nested blocks |
| `--ink`, `--ink-2`, `--muted` | text, secondary text, captions (all ≥ 4.5:1 on their ground) |
| `--line`, `--line-2` | borders, row separators |
| `--accent` / `--on-accent` | links, selection, primary data series |
| `--good`, `--bad`, `--warn`, `--info` | deltas, tags, event families; never the only signal (always an icon, a sign or a word too) |
| `--r-l` 16, `--r` 12, `--r-s` 8, `--r-pill` | radii |
| `--step--1` … `--step-4` | type scale: 12.5, 14, 15.5, 19, 26, 34 px |

### 3.3 Typography (needs a decision)

The mockups use **Geist** and **Geist Mono** (SIL OFL 1.1), loaded from Google Fonts on the canvas only. In the product:

1. Record a decision in `docs/decisions.md` (OFL-1.1 is not on the `deny.toml` allowlist, which covers crates, but rule 24 asks for provenance) and an entry in `ATTRIBUTION.md`.
2. Ship one subset woff2 variable file per family (Latin, weights 400–700), about 30 kB each, in `crates/ui/assets/`, served from `/assets/` with immutable caching. Preload only the text face.
3. Add `font-src 'self'` to both CSPs (`web.rs` and `share.rs`).
4. Use `font-display: swap` and a metric-matched fallback (`size-adjust`, `ascent-override`) so swapping does not shift the layout.

If the decision is "no webfont", keep the system stack: everything else in this document still applies.

### 3.4 Motion

`@layer motion` in `mockups/v2.css`. It is all CSS, and all of it sits inside `@media (prefers-reduced-motion: no-preference)`.

| What | How | Duration |
|---|---|---|
| Page to page | `@view-transition { navigation: auto; }`; sidebar and top bar named, so they stay still while the content fades and rises | 140 ms out, 240 ms in |
| Control feedback | colour and border transitions; `scale: .97` on `:active` for buttons and chips | 120 ms |
| Linked cards | lift 2 px with a soft shadow on hover | 200 ms |
| Content arrival | `main > *`, the focus grid and the KPI tiles rise 8 px with a 40 ms stagger | 420 ms |
| Data drawing in | chart bars `scale: 1 0 → 1` from their baseline (staggered), bar and progress fills `scale: 0 1 → 1`, the sparkline draws through `pathLength="1"` + `stroke-dashoffset`, heat cells fade row by row | 600–900 ms |
| Emphasis | the alerting KPI pulses a ring twice; the incident dot in the signals line breathes | 1.6 s ×2, 2.4 s loop |
| Popovers and dialogs | `transition` with `display`/`overlay allow-discrete` + `@starting-style`, so native `[popover]` and `<dialog>` fade and scale in and out | 180–260 ms |
| Disclosures | `interpolate-size: allow-keywords` + `::details-content` height transition (browsers without it just snap open) | 320 ms |
| Top bar | a scroll-driven animation (`animation-timeline: scroll(root)`) adds its shadow over the first 64 px of scroll | scroll-linked |

Rules: animate only `opacity`, `translate`, `scale` and colours (never layout properties). No animation longer than 900 ms except the two emphasis loops. Nothing moves on its own except the incident dot and the live dot, and only while there is something to report. In the SVG charts rendered from Rust, put the same classes on the `rect`s and `polyline`s, and use `pathLength="1"` on lines.

---

## 4. Shell and global parts (`crates/ui/src/lib.rs`: `layout_with`, `section_tabs`, `NAV`)

### 4.1 Sidebar

- **Project switcher:** `img.picon` currently shows a broken image when the project has no icon. The server decides instead: add `has_icon: bool` to the data behind `ProjectInfo` and render `span.monogram` (first letter) when it is false. No `onerror`.
- **Go to…:** `form method="get" action="/p/{id}/go"` with `input type="search" name="q" list="goto-list"`. The new route resolves `q` (section names, feature names, person ids) and answers `303` to the page, or renders a small results page. Server-render the `<datalist>` with at most 50 options (sections, then features). Bound the input length (rule 18).
- **Sections:** `accesskey="1"`…`"8"`. The `kbd` hint shows on hover or focus only.
- **User menu:** one `button popovertarget="me-menu"` opening `div#me-menu[popover]` with Instance settings (admins only), My profile, Appearance, Collapse the menu, and the existing `form method=post action=/logout`.
- The sidebar stays `position: sticky; top: 0; height: 100dvh` in the product (it is not sticky on the canvas, because of how the canvas renders).

### 4.2 Top bar and context bar (`components.rs`: `filter_bar`, `filter_fields`, `window_fields`)

- The top bar is `position: sticky; top: 0`, with a translucent background (`color-mix` + `backdrop-filter`). It holds the breadcrumb, then the context chips.
- Each chip is `button.chip popovertarget="ctx-period"` (or `ctx-env`, `ctx-role`, `ctx-release`). Its popover is a `form method="get"` holding its own field plus the **other filters as hidden inputs** (generalise `window_fields`), so applying one chip keeps the others.
- Position the popovers with anchor positioning (`anchor-name` on the chip, `position-anchor` + `position-area: block-end span-inline-end` on the popover). Where anchor positioning is not supported, the popover appears in the top layer at its default place, which is acceptable.
- "Team excluded / included" is a link that toggles `team=1`, styled as a dashed chip when off.
- Each chip's label always states the current value ("Since v2 · 10 days", "production"), so the page context is readable without opening anything.
- The date range moves into the period popover. Keep the rule text ("Filled dates win over the release…") as one short line in that popover, not on the page.

### 4.3 Section tabs

`section_tabs` keeps its markup (links with `aria-current`). Only the styling changes (`.subnav .tabs`, an underline on the current tab).

### 4.4 Navigation feel

- **Cross-document view transitions:** see 3.4. Pure CSS, no CSP change.
- **Speculation rules (optional, needs a decision):** prerendering on hover would make navigation feel instant, but an inline `<script type="speculationrules">` is blocked by the CSP. The narrow option is `script-src 'inline-speculation-rules'`, which allows speculation rules and nothing else. Check browser support and record a decision before doing it, in its own PR.
- **HTML streaming:** send the shell before the page's queries finish where the handler allows it. This is optional and separate from the visual work.

### 4.5 Signals line

The `alert` and `setup` banners (`components.rs`) become `span.signal` pills in one `div.signals[role=status]`: a coloured dot or a small meter, the text, and a link.

---

## 5. Components (in `mockups/v2.css` under `@layer components`)

| Part | Classes | Notes |
|---|---|---|
| Buttons | `.btn`, `.btn.primary`, `.btn.ghost`, `.btn.sm`, `.btn.danger-btn`, `.btn.danger-solid` | One primary per card. A link styled as a button stays an `<a>`. |
| Chips | `.chip`, `.chip.is-off`, `[aria-expanded=true]` | Context bar only |
| Tags | `.tag.tone-{bad,warn,good,info,accent}` | Insight kind, status, counts |
| Lead insight | `.card.lead`, `.compare`, `.next`, `.actions` | First insight only. Rank as `01`. |
| Ranked insights | `.card.ranked > ol > li.row` | Insights 2+. No left-border accent. |
| KPI tile | `.card.kpi`, `.kpi-head`, `.info`, `.kpi-value`, `.kpi-foot`, `.delta.{bad,good,flat}`, `.spark` | The whole tile links to its detail. The delta always says what it compares with. |
| Definition tooltips | `.info` → `button popovertarget` + `div[popover=hint]` (or `popover=auto`) | Replaces the ⓘ glyphs. Keep `aria-label="Definition: …"`. |
| Chart card | `.chart`, `.b`, `.axis`, `.marker` | In the product: SVG from Rust, with `<title>` per bar for the hover value. |
| Data table | `.card.table-card`, `tr.group`, `.ev.ev-{view,click,error,vital,identify}`, `.path`, `.pager` | Sticky header. Wide tables scroll inside `.scroll`. |
| Bar cell | `.barcell` | SVG rect for the width (CSP) |
| Cohort heatmap | `table.heat` | SVG cells or bucket classes (CSP) |
| Disclosure | `details.card.disclose` | Cohorts and release comparison. Open by default only when the section is the point of the page. |
| Entity header | `.entity`, `.avatar.lg`, `.facts` | Person, feature, release, flag pages |
| Mini card | `.card.mini` | Summary blocks on detail pages |
| Danger zone | `.card.danger > .item` | Always last on the page |
| Confirmation dialog | `.scrim > .dialog` | See below |
| Empty state | `.card.empty` + `.art` | The reason, what will make it fill, one or two actions |
| Setup steps | `.steps > .step.{done,todo}`, `.progress` | |
| Form card | `.card.form-card`, `.form-grid`, `.field`, `.form-foot` | Help text in `small` under the field |
| Refine bar | `form.card.refine`, `.active-filters` | Page-specific filters (Events, Errors, Audience) |
| Entry pages | `.auth`, `.auth-card`, `.or` | Login, Welcome, second factor |

**Confirmation without JS.** "Forget this person…" and "Delete forever" are links to `?confirm=forget` (or `?confirm=delete`). When that parameter is present, the server renders the same page with `<dialog open>` containing the type-to-confirm form, already focused through `autofocus`. This works in every browser. Optionally add `commandfor`/`command="show-modal"` on a button for an in-place modal where invoker commands are supported; the link stays the fallback.

---

## 6. Screen by screen

"Pattern" refers to the mockup that defines it. Source files are where the page's markup lives today (search for the page title to land on it).

| Screen | Source | Pattern | Specific changes |
|---|---|---|---|
| Log in | `lib.rs` `login` | login | No shell, no emoji. Token field `type=password`, monospace. SSO, email and password variants share the same card, separated by `.or`. |
| Welcome | `lib.rs` `welcome` | login | Same card. Primary: create my admin account. "Skip for now" as a ghost button. No emoji. |
| Second factor | `server/web/second_factor.rs`, `totp.rs` | login | Same card, a one-time-code field (`inputmode=numeric autocomplete=one-time-code`). |
| Focus board | `board.rs` | board | As drawn. "Paths to value" currently lists the same row twice: deduplicate (this is a bug, so open its own issue). |
| Focus board filtered on a release | `board.rs` | board | Same, with the release chip set. |
| Breakdown | `explore.rs` | events | The dimension pills become `.tabs`. The table uses `.barcell`. |
| Ask | `server/ask.rs`, `settings.rs` | live (empty state) | When off: an empty state explaining what Ask does, plus "Set up AI" for admins. When on: a question field, answers as cards with their numbers and links. |
| Live | `audience.rs` | live | One empty state when nothing happened in 30 min. Refresh stays a `<meta http-equiv=refresh>`, shown as the "Refreshes every 10 s" pill, with a live dot that pulses only when there is traffic. |
| Uptime | `environments.rs` / `audience.rs` | events + signals | Status header (up/down since, as a big status line), incidents table, "Set the app address" as a step card when missing. |
| Timeline | `goals.rs` | events | A vertical list grouped by day with typed markers (release, regression, map change). |
| Releases | `releases.rs` | board (disclosure table) | One row per release, with deltas vs the previous release coloured as on the board. A link to the board filtered on it. |
| Events | `explore.rs` | events | As drawn. Keep "Newest first", the 100-row pages and the UTC note. |
| Pages | `audience.rs` / `manage.rs` | events | Table with a bar cell for views. Each row links to its heatmap (see section 8). |
| Heatmap | `server/web/heatmap.rs`, `manage.rs` | see section 8 | See section 8. |
| Forms | `goals.rs` | events | Table per form with a funnel bar per field (SVG). |
| Feature flow | `audience.rs` | board (chart card) | The Sankey stays SVG. Use accent tints for steps, `--good` for success nodes, and keep "As a table" as a disclosure. |
| Errors | `explore.rs` / `manage.rs` | events | Group by message, with the count, the users hit, first/last seen and the release where it started. A row opens the error with its stack. |
| Replays | `server/web/replays.rs` | events | List of sessions. The player page is the only JS (vendored player, D-011): keep it isolated. |
| Audience | `audience.rs` | board | KPI tiles plus the world map. The map's country `<title>`s stay for accessibility. Fix the page `<title>`, which currently contains every country name. |
| Person | `explore.rs` | person | As drawn. |
| Acquisition | `audience.rs` | board | Channel table with bars, KPI tiles. |
| Churn | `audience.rs` | board | Lead finding first (the "last seen in billing" kind), then the chart and tables. |
| Goals | `goals.rs` | board + settings | Goal cards with progress bars. "New goal" is a form card. |
| Revenue | `audience.rs` | live (empty state) | Empty state explaining which event to send, with the snippet. |
| Feature map | `features.rs` | events + board | Features table, then "Map health" as a lead-style score with findings as a ranked list. Each finding keeps "Suggested change" in a `<details>` and "Create ticket". |
| Feature page | `feature_page.rs` | person | Entity header (feature name, status tag), KPI tiles, then tables. |
| Flags | `manage.rs` | events | Table of flags with an on/off tag and the rollout bar. |
| Flag editor | `manage.rs` | settings | Form card. "Delete" in a danger card. |
| Surveys | `manage.rs` | events + empty state | Empty state with the agent example as a code block. |
| Survey editor | `manage.rs` | settings | Form card. The question type is a segmented control (`.seg`, radio inputs). |
| Tickets | `server/tickets.rs`, `settings.rs` | events + settings | Empty state, then the GitHub connection as a form card. |
| New ticket | `server/tickets.rs` | settings | Title field, Markdown body in a monospace textarea, "Copy" and "Open on GitHub" actions. |
| Weekly digest | `server/digest.rs` | board | The board's components, read-only. |
| Settings: General | `settings.rs` | settings | As drawn. |
| Settings: Install | `settings.rs` | settings | One card per method (script tag, npm, server SDKs), code blocks with a "select all" hint, the key with a regenerate action in a danger card. |
| Settings: Alerts | `settings.rs` | settings | One row per alert type with an on/off switch (checkbox styled as a switch) and its threshold field. Channels as a form card. |
| Settings: Sharing | `settings.rs` | settings | Form card, then the existing links as a table. |
| Settings: Environments | `environments.rs` | settings + events | Rule form card, rules table, hosts-seen table. |
| Settings: Data | `settings.rs` | settings + person | SQL and export as cards. "Delete the project" in a danger card, with the confirmation dialog. |
| Instance: Team | `team.rs` | settings + events | Invite form card, people table with role tags. |
| Instance: API tokens | `team.rs` | settings + events | Create form card, tokens table. "Sign out everywhere" in a danger card. |
| Instance: AI | `settings.rs` | settings | The provider becomes a select or a segmented control. Show the key hint for the chosen provider only (`:has()` on the checked radio, no JS). |
| Instance: Storage | `settings.rs`, `server/storage.rs` | settings | S3 form card, backups table, cold tier in its own card. Never print the local data path in full: it can leak a home directory. |
| Instance: About | `lib.rs` / `team.rs` | settings | Version, update notice, links. |
| Audit log | `team.rs` / `goals.rs` | events | Table with `action` as a mono pill and day groups. |
| My profile | `team.rs` | settings | Profile form card. |
| My profile: Sign-in | `team.rs` | settings | Account creation as a form card. |
| My profile: Appearance | `team.rs`, `server/web/appearance.rs` | settings | Theme as a segmented control with three options, a preview swatch for each, and the folded-menu checkbox. |
| Not available | `lib.rs` `message` | live (empty state) | Generic empty-state card. |
| Shared (read-only) view | `lib.rs` `shared_layout` | board | No sidebar, a "Read-only view" pill in the top bar, no context popovers (the filters are fixed). |

---

## 7. PR plan

Open one issue per line and claim it before starting (rules 4–5). Each PR is mergeable on its own.

1. **CSS foundations.** Layers, tokens with `light-dark()` in `oklch`, the `data-theme` → `color-scheme` mapping, base and utilities. Existing class names keep working: there should be no visible change except colours.
2. **Shell.** Sidebar, user menu popover, monogram fallback (`has_icon`), section tabs restyle, sticky top bar, signals line.
3. **Go to….** The `/p/{id}/go` route, the datalist, bounded input, a test for an unknown `q`.
4. **Context bar.** Chips and popover forms replace `filter_bar` on every analysis page. Hidden fields keep the other filters. Test: applying one chip keeps the others in the URL.
5. **Focus board.** Lead insight, ranked list, KPI tiles with deltas, chart card, disclosures.
6. **Tables.** Events, Live, Person, Audit log, Errors: day groups, event pills, pager. Includes the Live and Revenue empty states.
7. **Detail pages and danger zones.** Person, Feature page, Flag editor, Settings Data: entity header, danger card, the `?confirm=` dialog.
8. **Settings and forms.** Setup steps, form cards, segmented controls, alert switches.
9. **Entry pages.** Login, Welcome, second factor, shared view.
10. **Remaining screens** of the table in section 6, one PR per section (Behaviour, Quality, People, Product).
11. **Motion.** The `motion` layer and view transitions (section 3.4). CSS only, plus `pathLength` on the SVG lines.
12. **Webfont**, after its decision.
13. **Speculation rules**, after its decision.

Separate bug issues: "Paths to value" shows a duplicate row; the Audience `<title>` lists every country; Releases' first-seen dates print the raw ISO `T`; date inputs follow the browser locale, so show the expected format next to them.

---

## 8. Heatmap (to settle before PR 10)

A click map only makes sense **for one page, at one layout width, with the page behind it**. The data already supports that: `$x` is a share of the viewport width, `$y` is the page offset, `$vw` is kept, and the rollups (`click_q`, `label_q`, `scroll_q`) store cells, top labels and scroll depth per page.

Proposed v2:

1. **Enter from a page**, not from a menu: every row of Pages (and every `/path` link) offers "Heatmap". The heatmap page has a page picker in its header.
2. **One layout width at a time:** Mobile < 640, Tablet 640–1024, Desktop ≥ 1024, picked as tabs, defaulting to the most common one. Positions from different widths are never mixed.
3. **The page behind it:** use the recorded page (the existing `heatmapMode` in `sdk/js/src/player.ts`), choosing a recording from the same release and the same width bucket. Label it: "Page as recorded on 24 Nov, release v2, 1280 px".
4. **Without a recording**, the page stays useful and needs no JS: the most clicked labels (`label_q`) as a ranked bar list, the scroll-depth bars (`scroll_q`), and rage clicks, with one line explaining that turning on replays adds the page view.
5. **Modes:** Clicks, Rage clicks and Scroll depth, as `.tabs` links.

Overlaying the **live** app (the SDK draws the map inside the customer's own page, opened from a signed link) is the most faithful option. It would add weight and attack surface to the SDK, so it needs its own decision and is not part of v2.

---

## 9. Done means

- Every screen of section 6, in light and dark, at 1440 px and 390 px, matches the patterns, and nothing overflows.
- No `style=` attribute and no `<script>` in any HTML response (add an integration test that scans the demo pages).
- With `prefers-reduced-motion: reduce`, nothing animates.
- axe-core reports no violations on the demo screens. Text contrast is ≥ 4.5:1, and ≥ 3:1 for large text and UI parts.
- Keyboard: every action is reachable with Tab, popovers close with Escape, `accesskey` 1–8 works, and focus is always visible.
- The stylesheet is ≤ 12 kB gzipped, and no page grows by more than 10 % in HTML bytes.
- `cargo xtask ci` passes, and the PR says what ran.
