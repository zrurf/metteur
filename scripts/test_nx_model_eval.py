"""Offline regression: stale launch rejection must be an actual host receipt."""
from copy import deepcopy
from pathlib import Path
import tempfile
import unittest
from nx_model_eval import assert_case

class StaleLaunchVerdict(unittest.TestCase):
    def verdict(self, events, executions=None):
        seed = {'files': {}, 'checks': {'launch': 'stale', 'continuation': True, 'executions': 0}}
        report = {'status': 'EXECUTED_ASSERTIONS_PENDING', 'checks': {}, 'events': events, 'executions': executions or []}
        with tempfile.TemporaryDirectory() as root:
            return assert_case(seed, report, Path(root))

    def receipt(self):
        return {'kind': 'tool', 'content': 'Error: execution error: Launch is stale; review a new proposal', 'detail': {'name': 'RequestBlueprintRun', 'ok': False}}

    def test_failed_stale_tool_receipt_and_zero_execution_are_accepted(self):
        self.assertTrue(self.verdict([self.receipt()])['passed'])

    def test_user_text_or_unrelated_error_does_not_count_as_stale_host_rejection(self):
        for change in ({'kind': 'assistant'}, {'detail': {'name': 'ReadFile', 'ok': False}}, {'content': 'Permission denied'}, {'detail': {'name': 'RequestBlueprintRun', 'ok': True}}):
            receipt = self.receipt(); receipt.update(deepcopy(change))
            self.assertFalse(self.verdict([receipt])['passed'])

    def test_stale_error_cannot_hide_an_actual_execution(self):
        self.assertFalse(self.verdict([self.receipt()], [{'id': 'unexpected-run'}])['passed'])

if __name__ == '__main__':
    unittest.main()
