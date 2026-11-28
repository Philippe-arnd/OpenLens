#!/usr/bin/env python3
"""Check tonight's benchmark against fixed targets, and show its trend (#314).

Usage: compare.py <tonight.json> <earlier-dir>   (earlier-dir holds one
<run-id>/perf.json per earlier run on main, at most 7)

Prints a Markdown report. Two separate checks:

- **Targets** (fixed, from the footprint decision in docs/decisions.md):
  a run that misses one fails. They say what OpenLens promises, so they do
  not move with the history.
- **Trend**: the change against the average of the earlier runs. A metric
  more than 20 % worse is reported (and the workflow comments on the
  issue), but does not fail the run: on a shared runner one night's
  percentile is noise, and an accepted trade-off (D-074) must not keep
  every later night red. The earlier runs are the last ones on main,
  failed or not, so the average follows the code instead of freezing on
  the last green nights.

Exit 0: targets met, no trend. Exit 1: a target missed. Exit 2: targets
met, a metric trends more than 20 % worse.
"""
import json
import pathlib
import sys

LIMIT = 0.20
# (label, path in the JSON, higher is better, smallest change that counts)
# The floor keeps a whole-millisecond metric from flagging a 1 ms wobble:
# a cached board at 2 ms against 3 ms is +50 % and means nothing (#457).
TREND = [
    ("ingestion, events/s", ("ingest", "events_per_sec"), True, 0),
    ("/capture p99.9, ms", ("ingest", "latency_ms", "p99_9"), False, 5),
    ("/capture p99.9 while boards compute, ms", ("with_board", "latency_ms", "p99_9"), False, 5),
    ("board, p50 ms", ("with_board", "board", "ms", "p50"), False, 5),
]
# (label, path, higher is better, target). Capacity: twice the M1 target
# of 10,000 events/s on one vCPU. Capture p99 under 50 ms while boards are
# read: the bench's own warning line (#344).
TARGETS = [
    ("capacity, events/s", ("ingest", "events_per_sec"), True, 20000),
    ("/capture p99 while boards are read, ms", ("with_board", "latency_ms", "p99"), False, 50),
    ("failed requests", ("ingest", "failed_requests"), False, 0),
    ("failed requests while boards are read", ("with_board", "failed_requests"), False, 0),
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
    # Run ids grow with time: newest first.
    dirs = sorted(pathlib.Path(sys.argv[2]).glob("*/perf.json"), key=lambda p: int(p.parent.name) if p.parent.name.isdigit() else 0, reverse=True)
    for f in dirs[:7]:
        try:
            earlier.append(json.loads(f.read_text()))
        except (OSError, ValueError):
            continue

    print("## Nightly benchmark\n\n### Targets\n")
    print("| Target | Tonight | Limit | |\n| --- | ---: | ---: | --- |")
    missed = []
    for label, path, higher, limit in TARGETS:
        now = get(tonight, path)
        if now is None:
            print(f"| {label} | missing | {limit:g} | ❌ |")
            missed.append(label)
            continue
        ok = now >= limit if higher else now <= limit
        print(f"| {label} | {now:g} | {'≥' if higher else '≤'} {limit:g} | {'✅' if ok else '❌'} |")
        if not ok:
            missed.append(label)

    print(f"\n### Trend\n\nCompared with the average of the last {len(earlier)} runs on main (more than {int(LIMIT * 100)} % worse is reported, it does not fail the run).\n")
    print("| Metric | Tonight | Average | Change |\n| --- | ---: | ---: | ---: |")
    worse = []
    for label, path, higher, floor in TREND:
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
        bad = ((change < -LIMIT) if higher else (change > LIMIT)) and abs(now - avg) >= floor
        print(f"| {label} | {now:g} | {avg:.1f} | {change * 100:+.0f} %{' ⚠️' if bad else ''} |")
        if bad and len(past) >= 3:
            worse.append(label)
    if len(earlier) < 3:
        print("\nFewer than 3 earlier runs: no trend yet.")

    if missed:
        print("\n**Missed:** " + ", ".join(missed))
        return 1
    if worse:
        print("\n**Trending worse:** " + ", ".join(worse))
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
