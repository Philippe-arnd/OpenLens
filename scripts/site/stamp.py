#!/usr/bin/env python3
"""Stamp image references with a hash of the image (#439).

Every reference to a PNG of docs/img in README.md and site/index.html gets
`?v=<first 10 hex of its sha256>`, so the URL changes when the picture does,
and only then. Browsers, GitHub's image proxy and the Pages CDN cannot keep
serving an old image under a new URL.

Usage: stamp.py          rewrite the references in place
       stamp.py --check  list stale references and exit 1, change nothing
"""
import hashlib, os, re, sys

IMG = "docs/img"
# The files, and the prefix their image references start with.
FILES = {"README.md": "docs/img/", "site/index.html": "img/"}
# The absolute og:image URL on the site uses the same file names.
REF = r"(?P<pre>(?:^|[\"'(/= ])(?:https://[^\"' ]*/)?{prefix})(?P<name>[A-Za-z0-9_.-]+\.png)(?:\?v=[0-9a-f]*)?"


def version(name, img_dir=IMG):
    """The stamp of docs/img/<name>, or None when the file is missing."""
    path = os.path.join(img_dir, name)
    if not os.path.isfile(path):
        return None
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()[:10]


def stamp(text, prefix, img_dir=IMG):
    """The text with every image reference stamped, and the missing names."""
    missing = []

    def sub(m):
        v = version(m["name"], img_dir)
        if v is None:
            missing.append(m["name"])
            return m[0]
        return f"{m['pre']}{m['name']}?v={v}"

    pattern = re.compile(REF.format(prefix=re.escape(prefix)), re.M)
    return pattern.sub(sub, text), missing


def main():
    check = "--check" in sys.argv[1:]
    stale, missing = [], []
    for path, prefix in FILES.items():
        with open(path, encoding="utf-8") as f:
            text = f.read()
        new, gone = stamp(text, prefix)
        missing += [f"{path}: {n}" for n in gone]
        if new != text:
            stale.append(path)
            if not check:
                with open(path, "w", encoding="utf-8") as f:
                    f.write(new)
    for m in missing:
        print(f"missing image: {m}", file=sys.stderr)
    if check and stale:
        print("stale image references in " + ", ".join(stale) + ": run scripts/site/stamp.py", file=sys.stderr)
    elif stale:
        print("stamped " + ", ".join(stale))
    sys.exit(1 if missing or (check and stale) else 0)


if __name__ == "__main__":
    main()
