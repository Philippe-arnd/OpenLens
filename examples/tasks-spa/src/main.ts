// Tasks: a tiny SPA wired to OpenLens with the npm package
// (docs/agents/wire.md, step 2, "SPA"). The server injects its config.
import openlens from "@openlens/web";

declare global {
  interface Window {
    APP_CONFIG: { openlensHost: string; openlensKey: string; release: string };
  }
}

const cfg = window.APP_CONFIG;
openlens.init({ host: cfg.openlensHost, key: cfg.openlensKey, release: cfg.release });

// The logged-in user, as the app knows it (demo: from ?as=<id>:<role>).
const as = new URLSearchParams(location.search).get("as") ?? localStorage.getItem("as") ?? "t1:manager";
localStorage.setItem("as", as);
const [id, role] = as.split(":");
openlens.identify(id ?? "t1", { role: role ?? "manager", account: "acme", plan: "team" });

const tasks: string[] = [];
const app = document.getElementById("app") as HTMLElement;

function go(path: string): void {
  history.pushState({}, "", path);
  render();
}

function render(): void {
  const p = location.pathname;
  if (p === "/tasks/new") {
    app.innerHTML = `<h1>New task</h1><form id="f"><input name="title" data-openlens-mask><button data-feature="create-task">Create</button></form>`;
    (document.getElementById("f") as HTMLFormElement).onsubmit = (e) => {
      e.preventDefault();
      tasks.push(String(new FormData(e.target as HTMLFormElement).get("title") ?? ""));
      openlens.track("task_created");
      go("/tasks");
    };
  } else if (p === "/reports") {
    openlens.feature("reports");
    app.innerHTML = `<h1>Reports</h1><p>${tasks.length} tasks this week.</p>`;
    openlens.track("report_viewed");
  } else {
    app.innerHTML = `<h1>Tasks</h1><ul>${tasks.map((t) => `<li data-openlens-mask>${t.replace(/</g, "&lt;")}</li>`).join("")}</ul>`;
  }
}

document.addEventListener("click", (e) => {
  const a = (e.target as Element).closest("a[data-link]");
  if (a) {
    e.preventDefault();
    go(a.getAttribute("href") ?? "/");
  }
});
addEventListener("popstate", render);
render();
