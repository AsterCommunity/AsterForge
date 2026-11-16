import importlib.util
from pathlib import Path
import unittest


spec = importlib.util.spec_from_file_location(
    "check_results", Path(__file__).with_name("check-results.py")
)
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class GateTests(unittest.TestCase):
    def setUp(self):
        self.needs = {
            "changes": {
                "result": "success",
                "outputs": {flag: "false" for flag in gate.SUITES.values()},
            },
            **{job: {"result": "skipped"} for job in gate.SUITES},
        }

    def test_documentation_only_skips_are_accepted(self):
        self.assertEqual(gate.failures(self.needs), [])

    def test_selected_suite_must_succeed(self):
        self.needs["changes"]["outputs"]["has_panic"] = "true"
        self.assertTrue(gate.failures(self.needs))
        self.needs["panic-contract"]["result"] = "success"
        self.assertEqual(gate.failures(self.needs), [])

    def test_failed_cancelled_or_missing_jobs_fail(self):
        for state in ["failure", "cancelled", None]:
            with self.subTest(state=state):
                self.needs["quality"]["result"] = state
                self.assertTrue(gate.failures(self.needs))

    def test_failed_or_missing_selection_cannot_pass(self):
        self.needs["changes"]["result"] = "failure"
        self.assertTrue(gate.failures(self.needs))
        self.needs["changes"]["result"] = "success"
        del self.needs["changes"]["outputs"]["has_rust"]
        self.assertTrue(gate.failures(self.needs))
