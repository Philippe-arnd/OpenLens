#!/usr/bin/env python3
"""Build the product page (site/) into _site/ for GitHub Pages.

Usage: build.py <tag> <date> <release-url> [notes.md]
The notes are docs/releases/<tag>.md when present: its first paragraph is the
summary and the top-level bullets of "Highlights" become the list.
"""
import html, os, re, shutil, sys

tag, date, url = sys.argv[1:4]
notes_path = sys.argv[4] if len(sys.argv) > 4 else f"docs/releases/{tag}.md"

def inline(s):
    s = html.escape(s, quote=False)
    s = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", s)
    s = re.sub(r"`([^`]+)`", r"<code>\1</code>", s)
    s = re.sub(r"\[([^\]]+)\]\((https?://[^)]+)\)", r'<a href="\2">\1</a>', s)
    s = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", s)  # relative links: keep text
    return s

summary, items = "", []
if os.path.exists(notes_path):
    text = open(notes_path, encoding="utf-8").read()
    paras = [p.strip() for p in re.split(r"\n\s*\n", text) if p.strip() and not p.lstrip().startswith("#")]
    summary = paras[0] if paras else ""
    m = re.search(r"^## .*Highlights.*?\n(.*?)(?=^## |\Z)", text, re.S | re.M)
    if m:
        items = [l[2:].strip() for l in m.group(1).splitlines() if l.startswith("- ")]
highlights = "<ul>" + "".join(f"<li>{inline(i)}</li>" for i in items) + "</ul>" if items else ""
minor = ".".join(tag.lstrip("v").split(".")[:2])

out = "_site"
shutil.rmtree(out, ignore_errors=True)
shutil.copytree("site", out)
os.makedirs(f"{out}/img", exist_ok=True)
for f in ["logo.svg", "board-light.png", "board-dark.png", "social-preview.png"]:
    shutil.copy(f"docs/img/{f}", f"{out}/img/{f}")
page = open(f"{out}/index.html", encoding="utf-8").read()
for k, v in {"VERSION": html.escape(tag), "DATE": html.escape(date), "RELEASE_URL": html.escape(url),
             "CHANNEL": html.escape(minor), "SUMMARY": inline(summary), "HIGHLIGHTS": highlights}.items():
    page = page.replace("{{" + k + "}}", v)
if "{{" in page:
    sys.exit("unfilled placeholder in index.html")
open(f"{out}/index.html", "w", encoding="utf-8").write(page)
open(f"{out}/.nojekyll", "w").close()
print(f"built {out}/ for {tag}: {len(items)} highlights")
