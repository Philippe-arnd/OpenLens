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
