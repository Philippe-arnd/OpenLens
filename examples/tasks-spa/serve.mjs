// Static server with SPA fallback; injects the app config from the
// environment: OPENLENS_HOST, OPENLENS_KEY, RELEASE (default "tasks-1"), PORT (3002).
import http from "node:http";
import { readFileSync } from "node:fs";

const port = Number(process.env.PORT ?? 3002);
const config = JSON.stringify({
  openlensHost: process.env.OPENLENS_HOST ?? "",
  openlensKey: process.env.OPENLENS_KEY ?? "",
  release: process.env.RELEASE ?? "tasks-1",
}).replace(/</g, "\\u003c");
const page = `<!doctype html><html><head><meta charset="utf-8"><title>Tasks</title></head><body>
<nav><a href="/tasks" data-link>Tasks</a> · <a href="/tasks/new" data-link data-feature="create-task">New task</a> · <a href="/reports" data-link data-feature="reports">Reports</a></nav>
<main id="app"></main>
<script>window.APP_CONFIG = ${config};</script>
<script type="module" src="/app.js"></script>
</body></html>`;

http
  .createServer((req, res) => {
    if (req.url === "/app.js" || req.url === "/app.js.map") {
      res.writeHead(200, { "content-type": req.url.endsWith(".map") ? "application/json" : "text/javascript" });
      return res.end(readFileSync(new URL(`./dist${req.url}`, import.meta.url)));
    }
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    res.end(page);
  })
  .listen(port, () => console.log(`tasks on :${port}`));
