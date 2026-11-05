import json
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from replant_client import ReplantClient, ReplantError


class Handler(BaseHTTPRequestHandler):
    calls = []
    response = {}

    def log_message(self, *args):
        pass

    def _reply(self, code=200):
        body = json.dumps(self.response).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self.calls.append(("GET", self.path, self.headers.get("Authorization"), None))
        self._reply()

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(length)) if length else None
        self.calls.append(("POST", self.path, self.headers.get("Authorization"), body))
        if self.headers.get("Authorization") == "Bearer bad":
            self.response = {"status": "failed", "reason": "unauthorized"}
            self._reply(401)
        else:
            self._reply()


class ClientTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.url = f"http://127.0.0.1:{cls.server.server_port}"

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def setUp(self):
        Handler.calls.clear()
        Handler.response = {"status": "miss"}

    def test_run_posts_the_request_and_auth_header(self):
        client = ReplantClient(self.url, "secret")
        receipt = client.run({"prompt": "p", "input": {}, "context": {}})
        self.assertEqual(receipt["status"], "miss")
        self.assertEqual(Handler.calls[0][0], "POST")
        self.assertEqual(Handler.calls[0][1], "/run")
        self.assertEqual(Handler.calls[0][2], "Bearer secret")
        self.assertEqual(Handler.calls[0][3]["prompt"], "p")

    def test_stats_is_get_and_review_methods_are_routed(self):
        client = ReplantClient(self.url, "secret")
        client.stats()
        client.approve({"prompt": "p", "input": {}, "context": {}}, "a" * 64)
        client.reject({"prompt": "p", "input": {}, "context": {}}, "b" * 64)
        client.undo("c" * 64)
        self.assertEqual([call[1] for call in Handler.calls], ["/stats", "/approve", "/reject", "/undo"])
        self.assertEqual(Handler.calls[0][0], "GET")
        self.assertEqual(Handler.calls[1][3]["candidate"], "a" * 64)
        self.assertEqual(Handler.calls[3][3]["id"], "c" * 64)

    def test_inspect_uses_read_only_endpoint_and_preserves_null(self):
        Handler.response = None
        request = {"prompt": "absent"}
        self.assertIsNone(ReplantClient(self.url, "secret").inspect(request))
        self.assertEqual(Handler.calls, [("POST", "/inspect", "Bearer secret", request)])

    def test_http_errors_raise_with_the_receipt(self):
        client = ReplantClient(self.url, "bad")
        with self.assertRaises(ReplantError) as caught:
            client.run({"prompt": "p", "input": {}, "context": {}})
        self.assertEqual(caught.exception.status, 401)
        self.assertEqual(caught.exception.receipt["status"], "failed")


if __name__ == "__main__":
    unittest.main()
