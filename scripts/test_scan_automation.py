import copy
import json
import os
import pathlib
import subprocess
import sys
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

    def test_provider_response_identity_is_counted_once_across_items_and_executions(self):
        ex = execution()
        items = ex["data"]["resultData"]["runData"]["Model"][0]["data"]["main"][0]
        items[0]["json"]["id"] = "chatcmpl-same-provider-response"
        items.append(copy.deepcopy(items[0]))
        other = copy.deepcopy(ex)
        other["id"] = "ex2"
        usage = scan.analyze_workflows([workflow()], [ex, other])["usage"]
        self.assertEqual(usage["total_tokens"], 15)
        self.assertEqual(usage["observed_model_responses"], 1)

    def test_repeated_response_without_identity_is_partial_not_double_counted(self):
        ex = execution()
        items = ex["data"]["resultData"]["runData"]["Model"][0]["data"]["main"][0]
        items.append(copy.deepcopy(items[0]))
        usage = scan.analyze_workflows([workflow()], [ex])["usage"]
        self.assertEqual(usage["status"], "partial")
        self.assertIsNone(usage["total_tokens"])
        self.assertEqual(usage["known_tokens"]["total_tokens"], 15)

    def test_conflicting_provider_response_identity_fails(self):
        ex = execution()
        items = ex["data"]["resultData"]["runData"]["Model"][0]["data"]["main"][0]
        items[0]["json"]["id"] = "chatcmpl-conflict"
        items.append(copy.deepcopy(items[0]))
        items[1]["json"]["usage"] = {"prompt_tokens": 11, "completion_tokens": 5, "total_tokens": 16}
        with self.assertRaises(ValueError):
            scan.analyze_workflows([workflow()], [ex])

    def test_unattributable_historical_node_marks_coverage_partial(self):
        ex = execution()
        runs = ex["data"]["resultData"]["runData"]
        runs["Old Model"] = copy.deepcopy(runs["Model"])
        usage = scan.analyze_workflows([workflow()], [ex])["usage"]
        self.assertEqual(usage["status"], "partial")
        self.assertIsNone(usage["total_tokens"])
        self.assertGreater(usage["unmeasured_responses"], 0)

    def test_malformed_nested_execution_data_fails(self):
        variants = [{}, "", [None], [{"data": []}], [{"data": {"main": {}}}],
                    [{"data": {"main": [""]}}], [{"data": {"main": [[None]]}}],
                    [{"data": {"main": [[{"json": []}]]}}]]
        for invalid in variants:
            with self.subTest(invalid=invalid):
                ex = execution()
                ex["data"]["resultData"]["runData"]["Model"] = invalid
                with self.assertRaises(ValueError):
                    scan.analyze_workflows([workflow()], [ex])

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
    echo_key = False
    truncated = False

    def log_message(self, *args):
        pass

    def do_GET(self):
        self.requests.append((self.command, self.path, self.headers.get("X-N8N-API-KEY")))
        payload = workflow() if "/workflows/" in self.path else {"data": [execution()], "nextCursor": "next" if self.cursor else None}
        if self.echo_key and "/workflows/" in self.path:
            payload["name"] = self.headers.get("X-N8N-API-KEY")
        body = b"bad json" if self.malformed else json.dumps(payload).encode()
        self.send_response(self.status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body) + (10 if self.truncated else 0)))
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
        Handler.status, Handler.cursor, Handler.malformed, Handler.echo_key = 200, False, False, False
        Handler.truncated = False

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

    def test_truncated_http_body_cannot_produce_an_audit(self):
        Handler.truncated = True
        with patch.dict(os.environ, {"SCAN_TEST_KEY": "private-api-secret"}):
            with self.assertRaises(ValueError):
                scan.collect(self.config())

    def test_cleartext_api_hostname_is_refused_before_connection(self):
        answer = [(2, 1, 6, "", ("127.0.0.1", 80))]
        with patch.object(scan.socket, "getaddrinfo", return_value=answer), patch.object(scan.http.client, "HTTPConnection") as connection:
            response = connection.return_value.getresponse.return_value
            response.status, response.length = 200, 0
            response.read.return_value = b"{}"
            with self.assertRaises(ValueError):
                scan.fetch("http://private-now-public-later.example", {"X-N8N-API-KEY": "private-api-secret"})
            connection.assert_not_called()

    def test_long_echoed_credential_is_redacted_before_label_truncation(self):
        Handler.echo_key = True
        secret = "unique-private-credential" * 15
        with patch.dict(os.environ, {"SCAN_TEST_KEY": secret}):
            report = scan.collect(self.config())
        self.assertNotIn("unique-private-credential", json.dumps(report))

    def test_explicit_wrong_workflow_execution_is_rejected(self):
        ex = execution()
        ex["workflowId"] = "other-workflow"
        with self.assertRaises(ValueError):
            scan.analyze_workflows([workflow()], [ex])

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

    def test_public_non_text_and_rebound_private_dns_are_refused(self):
        with patch.object(scan, "fetch", return_value=(b"binary", "application/pdf")):
            with self.assertRaises(ValueError):
                scan.collect({"mode": "prospects", "urls": ["https://example.com"]})
        answer = [(2, 1, 6, "", ("93.184.216.34", 443)), (2, 1, 6, "", ("127.0.0.1", 443))]
        with patch.object(scan.socket, "getaddrinfo", return_value=answer):
            with self.assertRaises(ValueError):
                scan.fetch("https://example.com", public=True)


class ReceiptTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.workflow = self.root / "workflow.json"
        self.workflow.write_text(json.dumps(workflow()))
        self.executions = self.root / "executions.json"
        self.executions.write_text(json.dumps([execution()]))
        self.out = self.root / "audit"
        self.script = pathlib.Path(scan.__file__).resolve()
        self.binary = pathlib.Path(os.environ.get("REPLANT_BIN", self.script.parents[1] / "target/debug/replant"))
        self.assertTrue(self.binary.is_file(), "build Replant first or set REPLANT_BIN")

    def run_scan(self, *extra):
        result = subprocess.run([sys.executable, str(self.script), "files", "--workflow", str(self.workflow),
            "--executions", str(self.executions), "--out", str(self.out), "--replant", str(self.binary), *extra],
            capture_output=True, text=True, timeout=30)
        self.assertTrue(result.stdout.strip(), result.stderr)
        return result, json.loads(result.stdout)

    def test_real_record_then_replay_reexecutes_and_binds_artifact(self):
        result, first = self.run_scan()
        self.assertEqual(result.returncode, 0, first)
        self.assertEqual(first["receipt"]["operation"], "record")
        original = json.loads((self.out / "report.json").read_text())
        result, second = self.run_scan()
        self.assertEqual(result.returncode, 0, second)
        self.assertEqual(second["receipt"]["operation"], "replay")
        current = json.loads((self.out / "report.json").read_text())
        self.assertNotEqual(original["challenge"], current["challenge"])
        self.assertEqual(second["artifact_sha256"], scan.digest((self.out / "report.json").read_bytes()))
        self.assertEqual(current["findings"]["usage"]["total_tokens"], 15)
        self.assertIsNone(current["findings"]["customer_savings"])
        self.assertEqual(len(list((self.out / "receipts").glob("*.json"))), 2)
        for path in self.out.rglob("*.json"):
            self.assertNotIn("DO_NOT_COPY", path.read_text(), str(path))

    def test_changed_source_records_new_evidence_and_stale_check_fails(self):
        _, first = self.run_scan()
        wf = workflow()
        wf["name"] = "Changed"
        self.workflow.write_text(json.dumps(wf))
        result, second = self.run_scan()
        self.assertEqual(result.returncode, 0, second)
        self.assertEqual(second["receipt"]["operation"], "record")
        self.assertNotEqual(first["receipt"]["key"], second["receipt"]["key"])
        challenge = json.loads((self.out / "attempt.json").read_text())
        challenge["nonce"] = "new-attempt"
        (self.out / "attempt.json").write_text(json.dumps(challenge))
        cfg = json.loads((self.out / "config.json").read_text())
        result = subprocess.run([sys.executable, str(self.script), "_check", str(self.out)],
            input=json.dumps({"config_sha256": scan.digest(cfg)}), capture_output=True, text=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(json.loads(result.stdout)["ok"])

    def test_failed_scan_blocks_automatic_retry_and_does_not_reference_old_artifact(self):
        self.run_scan()
        self.workflow.write_text("invalid json")
        result, failed = self.run_scan()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(failed["receipt"]["status"], "failed")
        self.assertIsNone(failed["artifact_sha256"])
        result, blocked = self.run_scan()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(blocked["receipt"]["status"], "blocked")

    def test_unrelated_output_directory_is_not_overwritten(self):
        self.out.mkdir()
        artifact = self.out / "report.json"
        artifact.write_text("user work")
        result, _ = self.run_scan()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(artifact.read_text(), "user work")

    def test_invalid_authenticated_url_is_not_saved_to_config(self):
        result = subprocess.run([sys.executable, str(self.script), "api", "--instance-url", "https://user:DO_NOT_SAVE@example.com",
            "--workflow-id", "wf1", "--out", str(self.out), "--replant", str(self.binary)], capture_output=True, text=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.out / "config.json").exists())
        self.assertNotIn("DO_NOT_SAVE", result.stdout)

    def test_check_refuses_wrong_source_binding_even_with_updated_checksum(self):
        self.run_scan()
        path = self.out / "report.json"
        report = json.loads(path.read_text())
        report["findings"]["sources"][0]["sha256"] = "a" * 64
        path.write_text(json.dumps(report))
        (self.out / "report.md").write_text(scan.markdown(report))
        (self.out / "report.sha256").write_text(scan.digest(path.read_bytes()))
        cfg = json.loads((self.out / "config.json").read_text())
        with self.assertRaises(ValueError):
            scan.worker("_check", self.out, {"config_sha256": scan.digest(cfg)})

    def test_check_refuses_inconsistent_usage_even_with_updated_checksum(self):
        self.run_scan()
        path = self.out / "report.json"
        report = json.loads(path.read_text())
        report["findings"]["usage"]["total_tokens"] = 999
        path.write_text(json.dumps(report))
        (self.out / "report.md").write_text(scan.markdown(report))
        (self.out / "report.sha256").write_text(scan.digest(path.read_bytes()))
        cfg = json.loads((self.out / "config.json").read_text())
        with self.assertRaises(ValueError):
            scan.worker("_check", self.out, {"config_sha256": scan.digest(cfg)})

    def test_api_mode_creates_real_scan_receipt_without_persisting_key(self):
        Handler.status, Handler.cursor, Handler.malformed, Handler.echo_key = 200, False, False, True
        Handler.truncated = False
        Handler.requests.clear()
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            result = subprocess.run([sys.executable, str(self.script), "api", "--instance-url", f"http://127.0.0.1:{server.server_port}",
                "--workflow-id", "wf1", "--key-env", "SCAN_TEST_KEY", "--out", str(self.out), "--replant", str(self.binary)],
                env=dict(os.environ, SCAN_TEST_KEY="private-api-secret"), capture_output=True, text=True, timeout=30)
            bundle = json.loads(result.stdout)
            self.assertEqual(result.returncode, 0, bundle)
            self.assertEqual(bundle["receipt"]["status"], "succeeded")
            self.assertEqual(len(Handler.requests), 2)
            for path in self.out.rglob("*.json"):
                self.assertNotIn("private-api-secret", path.read_text())
        finally:
            server.shutdown()
            server.server_close()
            thread.join()

    def test_concurrent_calls_are_serialized(self):
        command = [sys.executable, str(self.script), "files", "--workflow", str(self.workflow),
                   "--out", str(self.out), "--replant", str(self.binary)]
        self.run_scan()  # Establish directory ownership before concurrent callers.
        processes = [subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) for _ in range(2)]
        results = [p.communicate(timeout=30) for p in processes]
        self.assertTrue(all(p.returncode == 0 for p in processes), results)
        receipts = [json.loads(out)["receipt"] for out, _ in results]
        self.assertEqual({r["operation"] for r in receipts}, {"record", "replay"})
        self.assertEqual(len({r["id"] for r in receipts}), 2)


if __name__ == "__main__":
    unittest.main()
