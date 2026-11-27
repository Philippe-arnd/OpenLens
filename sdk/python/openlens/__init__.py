"""OpenLens server SDK for Python: a thin client over the HTTP API.

Standard library only. Events are queued and sent in batches from a
background thread; errors never raise into the app (they go to `on_error`).

    from openlens import OpenLens
    ol = OpenLens(host="https://lens.example.com", key="pk_...", release="v1.4.2")
    ol.identify("42", role="owner", account="acme")
    ol.capture("42", "invoice_sent")
    if ol.is_enabled("new-billing", "42", role="owner"):
        ...
    ol.shutdown()

Server errors: `ol.capture_exception(err)` in an `except` block, or
`ol.capture_uncaught()` once at start-up for crashes.
"""

from __future__ import annotations

import json
import os
import re
import sys
import sysconfig
import threading
import time
import traceback
import urllib.parse
import urllib.request
import uuid
from datetime import datetime, timezone
from typing import Any, Callable, Dict, Mapping, Optional

__all__ = ["OpenLens", "detect_release", "request_context", "SERVER_ID", "SESSION_HEADER"]
__version__ = "0.1.0"

#: The distinct id of events that belong to no user (server errors).
SERVER_ID = "$server"
#: The header the browser SDK adds to its own requests: the session id.
SESSION_HEADER = "x-openlens-session"
#: Variables hosts set to the deployed commit, in order of preference (#495).
COMMIT_VARS = (
    "SOURCE_COMMIT",
    "GITHUB_SHA",
    "VERCEL_GIT_COMMIT_SHA",
    "CF_PAGES_COMMIT_SHA",
    "RENDER_GIT_COMMIT",
    "HEROKU_SLUG_COMMIT",
)
#: At most this many errors with one message per minute.
MAX_SAME_ERROR = 5

_FULL_SHA = re.compile(r"^[0-9a-fA-F]{13,64}$")
_TRACEPARENT = re.compile(r"^([0-9a-f]{2})-([0-9a-f]{32})-([0-9a-f]{16})-[0-9a-f]{2}$")
_TRACE_ID = re.compile(r"^[0-9a-f]{32}$")
_SESSION_ID = re.compile(r"^[A-Za-z0-9_-]{1,100}$")
_LIB_DIRS = tuple(
    os.path.normcase(os.path.abspath(d)) + os.sep
    for d in {sysconfig.get_paths().get("stdlib"), sysconfig.get_paths().get("platstdlib")}
    if d
)


def detect_release(env: Mapping[str, str]) -> Optional[str]:
    """The release the host exposes: the deployed commit (shortened to 12
    characters when it is a full hash), else None."""
    for name in COMMIT_VARS:
        v = (env.get(name) or "").strip()
        if v:
            return v[:12].lower() if _FULL_SHA.match(v) else v[:200]
    return None


def request_context(headers: Mapping[str, str]) -> Dict[str, str]:
    """`trace_id` of the incoming W3C `traceparent` (read, never written)
    and `session_id` from the browser SDK's `x-openlens-session` header,
    for `capture_exception`. Header names are matched case-insensitively."""
    h = {str(k).lower(): v for k, v in headers.items()}
    ctx: Dict[str, str] = {}
    m = _TRACEPARENT.match(str(h.get("traceparent") or "").strip())
    if m and m.group(1) != "ff" and m.group(2).strip("0") and m.group(3).strip("0"):
        ctx["trace_id"] = m.group(2)
    sid = str(h.get(SESSION_HEADER) or "").strip()
    if _SESSION_ID.match(sid):
        ctx["session_id"] = sid
    return ctx


def _app_frame(tb: Any) -> Optional[str]:
    """The innermost frame outside installed packages and the standard
    library, as `file:line:0`: the error's group is its message @ this file."""
    for f in reversed(traceback.extract_tb(tb, limit=50)):
        name = f.filename or ""
        if not name or name.startswith("<"):
            continue
        norm = os.path.normcase(os.path.abspath(name))
        if "site-packages" in norm or "dist-packages" in norm or norm.startswith(_LIB_DIRS):
            continue
        return f"{name}:{f.lineno or 0}:0"
    return None


def _now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


class OpenLens:
    def __init__(
        self,
        host: str,
        key: str,
        release: Optional[str] = None,
        environment: Optional[str] = None,
        flush_at: int = 50,
        flush_interval: float = 5.0,
        timeout: float = 5.0,
        on_error: Optional[Callable[[Exception], None]] = None,
    ) -> None:
        if not host or not key:
            raise ValueError("OpenLens: host and key are required")
        self.host = host.rstrip("/")
        self.key = key
        # An explicit release wins; else what the host exposes (#495).
        self.release = release or detect_release(os.environ)
        self.environment = environment or os.environ.get("OPENLENS_ENVIRONMENT", "production")
        self.flush_at = flush_at
        self.timeout = timeout
        self.on_error = on_error or (lambda e: None)
        self._queue: list[dict[str, Any]] = []
        self._seen_minute = -1
        self._seen_errors: dict[str, int] = {}
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._loop, args=(flush_interval,), daemon=True)
        self._thread.start()

    def capture(self, distinct_id: str, event: str, properties: Optional[dict[str, Any]] = None) -> None:
        """Record an event for a user."""
        self._push(distinct_id, event, properties, None)

    def _push(self, distinct_id: str, event: str, properties: Optional[dict[str, Any]], session_id: Optional[str]) -> None:
        if not distinct_id or not event:
            return
        props: dict[str, Any] = {"$lib": "python", "$environment": self.environment}
        if self.release:
            props["$release"] = self.release
        props.update(properties or {})
        item = {
            "event": event,
            "distinct_id": str(distinct_id),
            "uuid": str(uuid.uuid4()),
            "timestamp": _now(),
            "properties": props,
        }
        if session_id:
            item["session_id"] = session_id
        with self._lock:
            self._queue.append(item)
            full = len(self._queue) >= self.flush_at
        if full:
            self.flush()

    def feature(self, distinct_id: str, key: str, properties: Optional[dict[str, Any]] = None) -> None:
        """Mark a use of a feature (a task start)."""
        self.capture(distinct_id, "$feature", {**(properties or {}), "$feature": key})

    def identify(self, distinct_id: str, role: Optional[str] = None, account: Optional[str] = None, plan: Optional[str] = None) -> None:
        """Set the user's role (and account, plan): what OpenLens slices every KPI by."""
        p = {k: v for k, v in (("$role", role), ("$account", account), ("$plan", plan)) if v}
        self.capture(distinct_id, "$identify", p)

    def is_enabled(
        self,
        flag: str,
        distinct_id: str,
        role: Optional[str] = None,
        account: Optional[str] = None,
        plan: Optional[str] = None,
        properties: Optional[Dict[str, Any]] = None,
    ) -> bool:
        """Is a flag on for this user? False on any error. `plan` and
        `properties` feed the flag's rules (never stored)."""
        q = {"key": self.key, "distinct_id": str(distinct_id)}
        if role:
            q["role"] = role
        if account:
            q["account"] = account
        if plan:
            q["plan"] = plan
        for k, v in list((properties or {}).items())[:20]:
            if v is not None and v != "":
                q[f"p.{str(k).lower()}"] = str(v).lower() if isinstance(v, bool) else str(v)
        try:
            url = f"{self.host}/flags?{urllib.parse.urlencode(q)}"
            with urllib.request.urlopen(url, timeout=self.timeout) as r:
                flags = json.loads(r.read().decode("utf-8")).get("flags", [])
            return isinstance(flags, list) and flag in flags
        except Exception as e:  # noqa: BLE001 - never raise into the app
            self.on_error(e)
            return False

    def check_in(
        self,
        slug: str,
        status: Optional[str] = None,
        ms: Optional[float] = None,
        expect_every: Optional[float] = None,
    ) -> bool:
        """Job check-in (#609): call it at the end of a cron or background
        job, so OpenLens alerts when the job does not run, fails or gets
        slower. `status` is "ok" (default) or "error", `ms` the run's
        duration, `expect_every` the seconds between runs (instead of the
        schedule OpenLens learns). Sent at once; False on any error, never
        raises."""
        q: dict[str, str] = {}
        if status:
            q["status"] = status
        if ms is not None:
            q["ms"] = str(max(0, round(ms)))
        if expect_every is not None:
            q["expectEvery"] = str(round(expect_every))
        try:
            url = f"{self.host}/check-in/{urllib.parse.quote(str(slug), safe='')}"
            if q:
                url += f"?{urllib.parse.urlencode(q)}"
            req = urllib.request.Request(url, data=b"", method="POST", headers={"x-openlens-key": self.key})
            with urllib.request.urlopen(req, timeout=self.timeout) as r:
                r.read()
            return True
        except Exception as e:  # noqa: BLE001 - never raise into the app
            self.on_error(e)
            return False

    def capture_exception(
        self,
        err: BaseException,
        distinct_id: Optional[str] = None,
        session_id: Optional[str] = None,
        trace_id: Optional[str] = None,
        route: Optional[str] = None,
        method: Optional[str] = None,
        properties: Optional[dict[str, Any]] = None,
    ) -> None:
        """Report a server exception. It is sent as `$error` with
        `$kind: "server"`, so it is grouped, alerted on and judged per
        release like a browser error. `session_id` and `trace_id` come from
        `request_context(request.headers)`; `route` is the route template
        (`/api/entries/<id>`), never the raw URL. Never raises."""
        try:
            if getattr(err, "__openlens_reported__", False):
                return
            try:
                err.__openlens_reported__ = True  # type: ignore[attr-defined]
            except Exception:  # noqa: BLE001 - some exceptions take no attribute
                pass
            name = type(err).__name__ if isinstance(err, BaseException) else "Error"
            message = f"{name}: {err}"[:500]
            if not self._allow_error(message):
                return
            props: dict[str, Any] = {**(properties or {}), "$kind": "server", "$message": message}
            tb = getattr(err, "__traceback__", None)
            if tb is not None:
                props["$stack"] = "".join(traceback.format_exception(type(err), err, tb, limit=50))[-4000:]
                source = _app_frame(tb)
                if source:
                    props["$source"] = source
            # OpenTelemetry semantic-convention names next to OpenLens's own.
            props["exception.type"] = name[:200]
            if route:
                props["http.route"] = str(route)[:300]
            if method:
                props["http.request.method"] = str(method).upper()[:10]
            if trace_id and _TRACE_ID.match(trace_id) and trace_id.strip("0"):
                props["trace_id"] = trace_id
            sid = session_id if session_id and _SESSION_ID.match(session_id) else None
            self._push(distinct_id or SERVER_ID, "$error", props, sid)
        except Exception as e:  # noqa: BLE001 - never raise into the app
            self.on_error(e)

    def capture_uncaught(self) -> None:
        """Report uncaught exceptions (main thread and other threads), send
        them at once, then let Python handle them as it would have. Opt-in."""
        prev = sys.excepthook
        prev_thread = threading.excepthook

        def hook(t: Any, v: BaseException, tb: Any) -> None:
            if not isinstance(v, (KeyboardInterrupt, SystemExit)):
                self.capture_exception(v)
                self.flush()
            prev(t, v, tb)

        def thread_hook(args: Any) -> None:
            if args.exc_value is not None and not isinstance(args.exc_value, SystemExit):
                self.capture_exception(args.exc_value)
                self.flush()
            prev_thread(args)

        sys.excepthook = hook
        threading.excepthook = thread_hook

    def _allow_error(self, message: str) -> bool:
        minute = int(time.time() // 60)
        with self._lock:
            if minute != self._seen_minute:
                self._seen_minute = minute
                self._seen_errors.clear()
            n = self._seen_errors.get(message, 0) + 1
            if len(self._seen_errors) < 1000 or message in self._seen_errors:
                self._seen_errors[message] = n
        return n <= MAX_SAME_ERROR

    def flush(self) -> None:
        """Send queued events now."""
        with self._lock:
            batch, self._queue = self._queue, []
        if not batch:
            return
        body = json.dumps({"sent_at": _now(), "batch": batch}).encode("utf-8")
        req = urllib.request.Request(
            f"{self.host}/capture",
            data=body,
            method="POST",
            headers={"content-type": "application/json", "x-openlens-key": self.key},
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as r:
                r.read()
        except Exception as e:  # noqa: BLE001
            self.on_error(e)

    def shutdown(self) -> None:
        """Flush and stop the background thread (call on exit)."""
        self._stop.set()
        self._thread.join(timeout=self.timeout)
        self.flush()

    def _loop(self, interval: float) -> None:
        while not self._stop.wait(interval):
            self.flush()
