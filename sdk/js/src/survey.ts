/**
 * Micro-survey widget, a separate bundle served at `/s.js`, loaded only
 * when a survey is due. It reads `window.__openlensSurvey` (set by the core
 * SDK), shows one question in a small card (shadow DOM, so the page's CSS
 * and ours do not mix), and reports the answer or the dismissal back.
 * The card is excluded from replays.
 */
interface Def {
  key: string;
  question: string;
  kind: "rating" | "yes_no" | "choice" | "text";
  choices: string[];
}
interface Bridge {
  survey: Def;
  answer: (v: string | number) => void;
  dismiss: () => void;
}
declare global {
  interface Window {
    __openlensSurvey?: Bridge;
    __openlensSurveyShow?: () => void;
  }
}

const STYLE = `
:host { all: initial; }
.card { position: fixed; right: 20px; bottom: 20px; z-index: 2147483000; width: min(340px, calc(100vw - 40px));
  background: #fff; color: #17161f; border: 1px solid #e9e6e0; border-radius: 14px; box-shadow: 0 10px 30px rgba(23,22,31,.15);
  font: 14px/1.4 system-ui, -apple-system, "Segoe UI", sans-serif; padding: 16px; }
@media (prefers-color-scheme: dark) { .card { background: #15151c; color: #ecebf5; border-color: #262532; } }
.q { font-weight: 600; margin: 0 24px 12px 0; }
.x { position: absolute; top: 8px; right: 10px; border: 0; background: none; font-size: 18px; color: #7b7889; cursor: pointer; }
.row { display: flex; gap: 6px; flex-wrap: wrap; }
.row button, .send { border: 1px solid #e9e6e0; background: transparent; color: inherit; border-radius: 999px; padding: 6px 12px; cursor: pointer; font: inherit; }
.row button:hover, .send:hover { border-color: #6d5efc; color: #6d5efc; }
textarea { width: 100%; box-sizing: border-box; min-height: 60px; border: 1px solid #e9e6e0; border-radius: 9px; padding: 8px; font: inherit; background: transparent; color: inherit; }
.thanks { margin: 0; }
`;

function show(): void {
  const b = window.__openlensSurvey;
  if (!b || document.getElementById("openlens-survey")) return;
  const host = document.createElement("div");
  host.id = "openlens-survey";
  host.setAttribute("data-openlens-block", "");
  const root = host.attachShadow({ mode: "open" });
  try {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(STYLE);
    root.adoptedStyleSheets = [sheet];
  } catch {
    const st = document.createElement("style");
    st.textContent = STYLE;
    root.append(st);
  }
  const card = document.createElement("div");
  card.className = "card";
  card.setAttribute("role", "dialog");
  card.setAttribute("aria-label", "Quick question");
  const q = document.createElement("p");
  q.className = "q";
  q.textContent = b.survey.question;
  const x = document.createElement("button");
  x.className = "x";
  x.setAttribute("aria-label", "Close");
  x.textContent = "×";
  const done = (v?: string | number) => {
    if (v === undefined) b.dismiss();
    else b.answer(v);
    card.replaceChildren(Object.assign(document.createElement("p"), { className: "thanks", textContent: "Thanks!" }));
    setTimeout(() => host.remove(), 1200);
  };
  x.onclick = () => done();
  const row = document.createElement("div");
  row.className = "row";
  const btn = (label: string, v: string | number) => {
    const e = document.createElement("button");
    e.textContent = label;
    e.onclick = () => done(v);
    row.append(e);
  };
  const s = b.survey;
  if (s.kind === "rating") for (let i = 1; i <= 5; i++) btn(String(i), i);
  else if (s.kind === "yes_no") {
    btn("Yes", "yes");
    btn("No", "no");
  } else if (s.kind === "choice") s.choices.forEach((c) => btn(c, c));
  card.append(x, q);
  if (s.kind === "text") {
    const t = document.createElement("textarea");
    t.maxLength = 500;
    const send = document.createElement("button");
    send.className = "send";
    send.textContent = "Send";
    send.onclick = () => (t.value.trim() ? done(t.value.trim()) : done());
    card.append(t, send);
  } else card.append(row);
  root.append(card);
  document.body.append(host);
}

window.__openlensSurveyShow = show;
show();

export {};
