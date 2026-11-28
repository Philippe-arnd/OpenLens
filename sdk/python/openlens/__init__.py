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
"""

from __future__ import annotations

import json
import os
import threading
import urllib.parse
import urllib.request
import uuid
from datetime import datetime, timezone
from typing import Any, Callable, Dict, Optional

__all__ = ["OpenLens"]
__version__ = "0.1.0"


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
        self.release = release
        self.environment = environment or os.environ.get("OPENLENS_ENVIRONMENT", "production")
        self.flush_at = flush_at
        self.timeout = timeout
        self.on_error = on_error or (lambda e: None)
        self._queue: list[dict[str, Any]] = []
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._loop, args=(flush_interval,), daemon=True)
        self._thread.start()

    def capture(self, distinct_id: str, event: str, properties: Optional[dict[str, Any]] = None) -> None:
        """Record an event for a user."""
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
        with self._lock:
            self._queue.append(item)
            full = len(self._queue) >= self.flush_at
        if full:
            self.flush()

    def feature(self, distinct_id: str, key: str, properties: Optional[dict[str, Any]] = None) -> None:
        """Mark a use of a feature (a task start)."""
        self.capture(distinct_id, "$feature", {**(properties or {}), "$feature": key})

    def identify(
        self,
        distinct_id: str,
        role: Optional[str] = None,
        account: Optional[str] = None,
        plan: Optional[str] = None,
        created_at: Any = None,
    ) -> None:
        """Set the user's role (and account, plan): what OpenLens slices every KPI by.

        ``created_at`` (an RFC 3339 string or a ``datetime``) is when the account
        was created: its signup, so a user who signed up before the wiring is
        not counted as new."""
        p = {k: v for k, v in (("$role", role), ("$account", account), ("$plan", plan)) if v}
        if created_at:
            p["$created_at"] = created_at.isoformat() if hasattr(created_at, "isoformat") else str(created_at)
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
