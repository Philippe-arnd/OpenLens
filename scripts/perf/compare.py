#!/usr/bin/env python3
"""Compare tonight's benchmark with the average of earlier runs (#314).

Usage: compare.py <tonight.json> <earlier-dir>   (earlier-dir holds one
<run-id>/perf.json per earlier successful run, newest first, at most 7)

Prints a Markdown report. Exit 0: no regression or too little history
(fewer than 3 earlier runs). Exit 1: a metric moved more than 20 % the
wrong way, listed in the report.
"""
import json
import pathlib
import sys

LIMIT = 0.20
# (label, path in the JSON, higher is better)
METRICS = [
    ("ingestion, events/s", ("ingest", "events_per_sec"), True),
    ("/capture p99.9, ms", ("ingest", "latency_ms", "p99_9"), False),
    ("/capture p99.9 while boards compute, ms", ("with_board", "latency_ms", "p99_9"), False),
    ("board, p50 ms", ("with_board", "board", "ms", "p50"), False),
]


def get(d, path):
    for k in path:
        if not isinstance(d, dict) or k not in d:
            return None
        d = d[k]
    return d if isinstance(d, (int, float)) else None


def main() -> int:
    tonight = json.loads(pathlib.Path(sys.argv[1]).read_text())
    earlier = []
    for f in sorted(pathlib.Path(sys.argv[2]).glob("*/perf.json"))[:7]:
        try:
            earlier.append(json.loads(f.read_text()))
        except (OSError, ValueError):
            continue
    print(f"## Nightly benchmark\n\nCompared with the average of the last {len(earlier)} runs (more than {int(LIMIT * 100)} % worse = regression).\n")
    print("| Metric | Tonight | Average | Change |\n| --- | ---: | ---: | ---: |")
    worse = []
    for label, path, higher in METRICS:
        now = get(tonight, path)
        past = [v for v in (get(e, path) for e in earlier) if v is not None]
        if now is None:
            print(f"| {label} | missing | | |")
            continue
        if not past:
            print(f"| {label} | {now:g} | | |")
            continue
        avg = sum(past) / len(past)
        change = (now - avg) / avg if avg else 0.0
        bad = (change < -LIMIT) if higher else (change > LIMIT)
        mark = " ⚠️" if bad else ""
        print(f"| {label} | {now:g} | {avg:.1f} | {change * 100:+.0f} %{mark} |")
        if bad and len(past) >= 3:
            worse.append(label)
    if len(earlier) < 3:
        print("\nFewer than 3 earlier runs: nothing compared yet.")
        return 0
    if worse:
        print("\n**Regressed:** " + ", ".join(worse))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
