import json
import os
import pathlib
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch

import scan_automation as scan


def workflow():
    return {"id": "wf1", "name": "Test workflow", "nodes": [
        {"id": "loop", "name": "Loop", "type": "n8n-nodes-base.splitInBatches"},
        {"id": "model", "name": "Model", "type": "n8n-nodes-base.httpRequest",
         "parameters": {"url": "https://provider.example/v1/chat/completions",
                        "secret": "sk-DO_NOT_COPY_1234567890123456789"}},
    ], "connections": {"Loop": {"main": [[], [{"node": "Model"}]]},
                       "Model": {"main": [[{"node": "Loop"}]]}}}


def execution(usage=True, identifier="ex1"):
    response = {"choices": [{"message": {"content": "PRIVATE CUSTOMER PROMPT"}}]}
    if usage:
        response["usage"] = {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
    return {"id": identifier, "workflowId": "wf1", "data": {"resultData": {"runData": {
        "Model": [{"data": {"main": [[{"json": response}]]}}]
    }}}}


class AnalysisTests(unittest.TestCase):
    def test_loop_candidate_and_real_usage_are_not_savings(self):
        report = scan.analyze_workflows([workflow()], [execution()])
        self.assertEqual(len(report["workflows"][0]["loop_candidates"]), 1)
        self.assertEqual(report["usage"]["total_tokens"], 15)
        self.assertEqual(report["usage"]["status"], "observed")
        self.assertIsNone(report["customer_savings"])
        self.assertEqual(report["replay_eligibility"], "unverified")
        body = json.dumps(report)
        self.assertNotIn("DO_NOT_COPY", body)
        self.assertNotIn("PRIVATE CUSTOMER PROMPT", body)

    def test_no_cycle_and_empty_telemetry_do_not_invent_usage(self):
        wf = workflow()
        wf["connections"]["Model"] = {}
        report = scan.analyze_workflows([wf], [])
        self.assertFalse(report["workflows"][0]["loop_candidates"])
        self.assertIsNone(report["usage"]["total_tokens"])
        self.assertEqual(report["usage"]["status"], "unknown")

    def test_missing_unsupported_and_inconsistent_usage_stay_partial(self):
        for variant in ["missing", "inconsistent", "unsupported"]:
            wf, ex = workflow(), execution()
            response = ex["data"]["resultData"]["runData"]["Model"][0]["data"]["main"][0][0]["json"]
            if variant == "missing":
                del response["usage"]
            elif variant == "inconsistent":
                response["usage"]["total_tokens"] = 999
            else:
                wf["nodes"][1]["type"] = "@n8n/n8n-nodes-langchain.lmChatOpenAi"
            report = scan.analyze_workflows([wf], [ex])
            self.assertIsNone(report["usage"]["total_tokens"], variant)
            self.assertGreater(report["usage"]["unmeasured_responses"], 0)

    def test_duplicate_exports_count_once_and_conflicts_fail(self):
        self.assertEqual(scan.analyze_workflows([workflow()], [execution(), execution()])["usage"]["total_tokens"], 15)
        changed = execution(False)
        with self.assertRaises(ValueError):
            scan.analyze_workflows([workflow()], [execution(), changed])

    def test_expressions_remain_inert_and_names_are_scrubbed(self):
        wf = workflow()
        wf["nodes"][0]["parameters"] = {"command": "={{process.exit(91)}}"}
        wf["name"] = "Bearer sk-DO_NOT_COPY_1234567890123456789"
        result = scan.analyze_workflows([wf], [])
        self.assertNotIn("DO_NOT_COPY", json.dumps(result))

    def test_unsupported_execution_format_fails_instead_of_empty_success(self):
        with self.assertRaises(ValueError):
            scan.analyze_workflows([workflow()], [{"wrong": "shape"}])


class Handler(BaseHTTPRequestHandler):
    requests = []
    status = 200
    cursor = False
    malformed = False

    def log_message(self, *args):
        pass

    def do_GET(self):
        self.requests.append((self.command, self.path, self.headers.get("X-N8N-API-KEY")))
        payload = workflow() if "/workflows/" in self.path else {"data": [execution()], "nextCursor": "next" if self.cursor else None}
        body = b"bad json" if self.malformed else json.dumps(payload).encode()
        self.send_response(self.status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body)


class CollectionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.url = f"http://127.0.0.1:{cls.server.server_port}"

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join()

    def setUp(self):
        Handler.requests.clear()
        Handler.status, Handler.cursor, Handler.malformed = 200, False, False

    def config(self):
        return {"mode": "api", "instance": self.url, "workflow_id": "wf1", "key_env": "SCAN_TEST_KEY", "limit": 10}

    def test_api_is_get_only_bounded_and_key_does_not_enter_report(self):
        with patch.dict(os.environ, {"SCAN_TEST_KEY": "private-api-secret"}):
            report = scan.collect(self.config())
        self.assertEqual(report["usage"]["total_tokens"], 15)
        self.assertEqual(len(Handler.requests), 2)
        self.assertTrue(all(method == "GET" for method, _, _ in Handler.requests))
        self.assertIn("limit=10", Handler.requests[1][1])
        self.assertTrue(all(key == "private-api-secret" for _, _, key in Handler.requests))
        self.assertNotIn("private-api-secret", json.dumps(report))

    def test_pagination_is_explicit_and_access_errors_are_not_retried(self):
        with patch.dict(os.environ, {"SCAN_TEST_KEY": "private-api-secret"}):
            Handler.cursor = True
            self.assertFalse(scan.collect(self.config())["coverage"]["complete"])
            for status in [401, 403, 429, 302]:
                Handler.requests.clear()
                Handler.status = status
                with self.assertRaises(ValueError):
                    scan.collect(self.config())
                self.assertEqual(len(Handler.requests), 1)

    def test_malformed_api_json_is_an_error(self):
        Handler.malformed = True
        with patch.dict(os.environ, {"SCAN_TEST_KEY": "private-api-secret"}):
            with self.assertRaises(ValueError):
                scan.collect(self.config())

    def test_public_url_guards(self):
        for url in ["http://example.com", "https://127.0.0.1", "https://user:pass@example.com", "https://example.com/?token=secret"]:
            with self.assertRaises(ValueError):
                scan.fetch(url, public=True)
        with patch.object(scan, "MAX_BODY", 10):
            with self.assertRaises(ValueError):
                scan.fetch(self.url)

    def test_public_content_is_evidence_not_buying_intent(self):
        with patch.object(scan, "fetch", return_value=(b"<title>Example Agency</title><p>We build n8n AI automation.</p>", "text/html")):
            report = scan.collect({"mode": "prospects", "urls": ["https://example.com"]})
        self.assertEqual(report["prospects"][0]["buying_intent"], "unknown")
        self.assertIn("n8n", report["prospects"][0]["matched_terms"])
        self.assertIsNone(report["customer_savings"])


if __name__ == "__main__":
    unittest.main()
