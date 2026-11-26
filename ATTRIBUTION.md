# Attribution

Every vendored file, asset and fixture is recorded here, in the same change that adds it: source (URL and version), licence, and how it was obtained. Rust dependencies are covered by `Cargo.lock` and checked by `cargo deny`; list one here only when its licence requires a notice we ship.

| Item | Source | Version | Licence | How obtained |
| --- | --- | --- | --- | --- |
| LICENSE text | GNU AGPL-3.0 via the GitHub licences API | 3.0 | text is FSF-copyrighted, verbatim copying permitted | `gh api licenses/agpl-3.0` |
| rrweb recorder, bundled into `sdk/js/dist/openlens-replay.min.js` (served at `/r.js`) | https://github.com/rrweb-io/rrweb (npm `rrweb`) | 2.2.2 | MIT, © rrweb contributors | `npm install --save-exact rrweb@2.2.2`, bundled by esbuild |
| rrweb replayer, bundled into `sdk/js/dist/openlens-player.min.js` (served at `/assets/player.js`) | https://github.com/rrweb-io/rrweb (npm `rrweb`) | 2.2.2 | MIT, © rrweb contributors | bundled by esbuild from the same package |
| rrweb player stylesheet, copied to `sdk/js/dist/openlens-player.min.css` (served at `/assets/player.css`) | `rrweb/dist/style.min.css` | 2.2.2 | MIT, © rrweb contributors | copied by `npm run build` |
| Country outlines in `crates/ui/src/world.rs` (audience map) | Natural Earth 1:110m admin-0 countries, https://github.com/nvkelso/natural-earth-vector/blob/master/geojson/ne_110m_admin_0_countries.geojson | v5 (master, 2026-11-23) | public domain (Natural Earth terms of use) | downloaded, projected and simplified by `scripts/geo/world.py` |
| Deploy notification fixtures in `crates/server/tests/fixtures/deploys/` (Coolify, Vercel, Netlify, Render, GitHub, Argo CD, Flux, Fly, plain) | written by us from each platform's public payload documentation (`docs/clean-room.md`, 2026-11-26), with made-up names, ids and hashes | 2026-11-26 | project licence | hand-written |
| IP-to-country database, shipped in the Docker image at `/usr/share/openlens/dbip-country-lite.mmdb.gz` (not in the repository) | DB-IP Lite country, https://db-ip.com/db/download/ip-to-country-lite | the month of the image build | CC BY 4.0, © DB-IP.com; credited on the Audience page ("IP geolocation by DB-IP") | downloaded by the `Dockerfile` at build time, refreshed by the monthly rebuild |

rrweb's MIT notice ships at the top of each bundle above (added by `sdk/js/scripts/banner.mjs`), and its text is in `sdk/js/licenses/rrweb-LICENSE.txt`, fetched from https://github.com/rrweb-io/rrweb/blob/master/LICENSE.
