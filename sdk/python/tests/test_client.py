import json
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlparse

from openlens import OpenLens

GOT = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers["content-length"]))
        GOT.append((self.headers["x-openlens-key"], json.loads(body)))
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'{"accepted":1}')

    def do_GET(self):
        q = parse_qs(urlparse(self.path).query)
        flags = ["beta"] if q.get("role") == ["admin"] else []
        if q.get("plan") == ["pro"] and q.get("p.seats") == ["12"] and q.get("p.trial") == ["true"]:
            flags.append("pro-only")
        self.send_response(200)
        self.end_headers()
        self.wfile.write(json.dumps({"flags": flags}).encode())


class ClientTest(unittest.TestCase):
    def test_batches_and_flags(self):
        srv = HTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        ol = OpenLens(host=f"http://127.0.0.1:{srv.server_port}/", key="pk_t", release="v9", flush_interval=60)
        ol.identify("u1", role="admin")
        ol.capture("u1", "invoice_sent")
        ol.feature("u1", "export")
        ol.shutdown()
        self.assertEqual(len(GOT), 1)
        key, body = GOT[0]
        self.assertEqual(key, "pk_t")
        self.assertEqual([e["event"] for e in body["batch"]], ["$identify", "invoice_sent", "$feature"])
        self.assertEqual(body["batch"][0]["properties"]["$role"], "admin")
        self.assertEqual(body["batch"][1]["properties"]["$release"], "v9")
        self.assertTrue(ol.is_enabled("beta", "u1", role="admin"))
        self.assertFalse(ol.is_enabled("beta", "u2", role="member"))
        self.assertTrue(ol.is_enabled("pro-only", "u1", plan="pro", properties={"Seats": 12, "trial": True}))
        self.assertFalse(ol.is_enabled("pro-only", "u1", plan="free", properties={"seats": 12, "trial": True}))
        srv.shutdown()

    def test_never_raises_when_down(self):
        errors = []
        ol = OpenLens(host="http://127.0.0.1:9", key="pk_t", on_error=errors.append, flush_interval=60, timeout=1)
        ol.capture("u1", "x")
        ol.shutdown()
        self.assertFalse(ol.is_enabled("beta", "u1"))
        self.assertGreaterEqual(len(errors), 1)


if __name__ == "__main__":
    unittest.main()


CHECK_INS = []


class CheckInHandler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        CHECK_INS.append((self.path, self.headers["x-openlens-key"]))
        self.send_response(200 if len(CHECK_INS) == 1 else 429)
        self.end_headers()
        self.wfile.write(b"{}")


class CheckInTest(unittest.TestCase):
    def test_check_in_posts_the_slug_and_never_raises(self):
        srv = HTTPServer(("127.0.0.1", 0), CheckInHandler)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        errors = []
        ol = OpenLens(host=f"http://127.0.0.1:{srv.server_port}", key="pk_t", flush_interval=60, on_error=errors.append)
        self.assertTrue(ol.check_in("nightly-export", status="error", ms=1234.4, expect_every=86400))
        path, key = CHECK_INS[0]
        u = urlparse(path)
        self.assertEqual(u.path, "/check-in/nightly-export")
        self.assertEqual(key, "pk_t")
        self.assertNotIn("pk_t", path)
        q = parse_qs(u.query)
        self.assertEqual((q["status"], q["ms"], q["expectEvery"]), (["error"], ["1234"], ["86400"]))
        self.assertFalse(ol.check_in("nightly-export"), "a refusal is False")
        self.assertGreaterEqual(len(errors), 1)
        ol.shutdown()
        srv.shutdown()


class ReleaseTest(unittest.TestCase):
    def test_detects_the_host_commit(self):
        from openlens import detect_release

        sha = "0123456789abcdef0123456789abcdef01234567"
        self.assertEqual(detect_release({"GITHUB_SHA": sha}), sha[:12])
        self.assertEqual(detect_release({"RENDER_GIT_COMMIT": "abc", "GITHUB_SHA": sha}), sha[:12], "earlier variable first")
        self.assertEqual(detect_release({"SOURCE_COMMIT": "  ", "HEROKU_SLUG_COMMIT": "v12"}), "v12")
        self.assertIsNone(detect_release({}))

    def test_explicit_release_wins(self):
        import os
        from unittest import mock

        with mock.patch.dict(os.environ, {"SOURCE_COMMIT": "deadbeef"}):
            self.assertEqual(OpenLens(host="http://x", key="k", flush_interval=60).release, "deadbeef")
            self.assertEqual(OpenLens(host="http://x", key="k", release="v2", flush_interval=60).release, "v2")


class ExceptionTest(unittest.TestCase):
    def test_server_error_with_request_context(self):
        from openlens import request_context

        GOT.clear()
        srv = HTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        ol = OpenLens(host=f"http://127.0.0.1:{srv.server_port}", key="pk_t", flush_interval=60)
        ctx = request_context({"TraceParent": f"00-{'a' * 32}-{'b' * 16}-01", "X-OpenLens-Session": "s1"})
        try:
            raise KeyError("entry 42")
        except KeyError as e:
            err = e
            ol.capture_exception(e, route="/api/entries/<id>", method="get", **ctx)
        ol.capture_exception(err)  # the same object is reported once
        for _ in range(10):
            ol.capture_exception(ValueError("same"))
        ol.shutdown()
        srv.shutdown()
        errs = [e for _, b in GOT for e in b["batch"] if e["event"] == "$error"]
        first = errs[0]
        p = first["properties"]
        self.assertEqual((first["distinct_id"], first["session_id"]), ("$server", "s1"))
        self.assertEqual((p["$kind"], p["exception.type"], p["http.route"], p["http.request.method"]), ("server", "KeyError", "/api/entries/<id>", "GET"))
        self.assertEqual(p["trace_id"], "a" * 32)
        self.assertIn("KeyError", p["$message"])
        self.assertIn("test_client.py:", p["$source"], "the app's own frame, not the SDK's")
        self.assertIn("raise KeyError", p["$stack"])
        same = [e for e in errs if e["properties"]["$message"] == "ValueError: same"]
        self.assertLess(len(same), 10, "a repeated message is rate-limited")
        self.assertEqual(sum(1 for e in errs if e["properties"]["exception.type"] == "KeyError"), 1)

    def test_rejects_hostile_context(self):
        from openlens import request_context

        self.assertEqual(request_context({"traceparent": f"00-{'0' * 32}-{'b' * 16}-01", "x-openlens-session": "a b"}), {})
        self.assertEqual(request_context({"traceparent": f"ff-{'a' * 32}-{'b' * 16}-01"}), {})


class UncaughtTest(unittest.TestCase):
    def test_crash_is_reported_and_still_fails(self):
        import os
        import subprocess
        import sys

        GOT.clear()
        srv = HTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        code = (
            "from openlens import OpenLens\n"
            f"ol = OpenLens(host='http://127.0.0.1:{srv.server_port}', key='pk_t', flush_interval=60)\n"
            "ol.capture_uncaught()\n"
            "raise LookupError('crash')\n"
        )
        src = os.path.join(os.path.dirname(__file__), "..")
        r = subprocess.run([sys.executable, "-c", code], cwd=src, capture_output=True, timeout=30)
        srv.shutdown()
        self.assertNotEqual(r.returncode, 0, "the process still fails")
        self.assertIn(b"LookupError: crash", r.stderr, "Python's own report is kept")
        errs = [e for _, b in GOT for e in b["batch"] if e["event"] == "$error"]
        self.assertEqual([e["properties"]["$message"] for e in errs], ["LookupError: crash"])
