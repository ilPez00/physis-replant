import importlib.util
import json
import unittest

from adapters import autogen_tool, crewai_tool, langchain_tools, request_payload


class FakeClient:
    def run(self, request):
        return {"status": "miss", "request": request}


class AdapterTests(unittest.TestCase):
    def test_request_payload_validates_and_defaults(self):
        self.assertEqual(
            request_payload("p"), {"prompt": "p", "input": {}, "context": {}}
        )
        with self.assertRaises(ValueError):
            request_payload("  ")

    def test_autogen_adapter_returns_a_documented_callable(self):
        tool = autogen_tool(FakeClient())
        receipt = json.loads(tool("increment once"))
        self.assertEqual(receipt["status"], "miss")
        self.assertIn("Look up", tool.__doc__)

    def test_framework_adapters_fail_with_an_install_hint_when_absent(self):
        for module, adapter in (("langchain_core", langchain_tools), ("crewai", crewai_tool)):
            if importlib.util.find_spec(module) is not None:
                continue
            with self.assertRaises(ImportError) as caught:
                adapter(FakeClient())
            self.assertIn("pip install", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
