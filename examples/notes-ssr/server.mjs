// Notes: a tiny server-rendered app wired to OpenLens with the script tag
// (docs/agents/wire.md, step 2, "server-rendered app"). No dependencies.
// Config comes from the environment, the app's existing config mechanism:
//   OPENLENS_HOST, OPENLENS_KEY, RELEASE (default "notes-1"), PORT (default 3001)
import http from "node:http";

const host = process.env.OPENLENS_HOST ?? "";
const key = process.env.OPENLENS_KEY ?? "";
const release = process.env.RELEASE ?? "notes-1";
const port = Number(process.env.PORT ?? 3001);
const notes = [];

const esc = (s) => String(s).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

function layout(user, body, after = "") {
  const identify = user
    ? `openlens.identify(${JSON.stringify(user.id)}, { role: ${JSON.stringify(user.role)}, account: "acme" });`
    : "";
  return `<!doctype html><html><head><meta charset="utf-8"><title>Notes</title>
<script src="${esc(host)}/c.js" data-key="${esc(key)}" data-release="${esc(release)}" defer></script>
</head><body>
<nav><a href="/">Notes</a> · <a href="/notes/new" data-feature="write-note">New note</a> · <a href="/search" data-feature="search">Search</a></nav>
<main>${body}</main>
<script>addEventListener("load", () => { ${identify} ${after} });</script>
</body></html>`;
}

function user(req) {
  const m = /(?:^|; )user=([a-z0-9]+):([a-z]+)/.exec(req.headers.cookie ?? "");
  return m ? { id: m[1], role: m[2] } : null;
}

http
  .createServer((req, res) => {
    const url = new URL(req.url, "http://x");
    const u = user(req);
    const page = (html) => {
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      res.end(html);
    };
    if (url.pathname === "/login") {
      // Demo login: /login?as=<id>:<role>, e.g. u1:owner. The role is the app's real field.
      const as = (url.searchParams.get("as") ?? "u1:member").replace(/[^a-z0-9:]/g, "");
      res.writeHead(302, { "set-cookie": `user=${as}; path=/`, location: "/" });
      return res.end();
    }
    if (url.pathname === "/notes" && req.method === "POST") {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        notes.push(new URLSearchParams(body).get("text") ?? "");
        res.writeHead(303, { location: "/?saved=1" });
        res.end();
      });
      return;
    }
    if (url.pathname === "/notes/new") {
      return page(layout(u, `<h1>New note</h1><form method="post" action="/notes">
<textarea name="text" data-openlens-mask></textarea>
<button type="submit" data-feature="write-note">Save</button></form>`));
    }
    if (url.pathname === "/search") {
      const q = url.searchParams.get("q");
      const hits = q ? notes.filter((n) => n.includes(q)).length : 0;
      return page(
        layout(
          u,
          `<h1>Search</h1><form><input name="q"><button data-feature="search">Search</button></form>${q ? `<p>${hits} notes</p>` : ""}`,
          q ? `openlens.track("search_done", { hits: ${hits} });` : "",
        ),
      );
    }
    const saved = url.searchParams.get("saved") === "1";
    return page(
      layout(u, `<h1>Notes</h1><ul>${notes.map((n) => `<li data-openlens-mask>${esc(n)}</li>`).join("")}</ul>`, saved ? `openlens.track("note_saved");` : ""),
    );
  })
  .listen(port, () => console.log(`notes on :${port}`));
