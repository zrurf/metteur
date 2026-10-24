"""Offline budget safety and held-out seed integrity checks; no API requests."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from nx_eval_fixtures import tasks
from nx_model_support import BudgetError, Ledger, atomic_json, nano_cost, usage_from_body

class BudgetSafety(unittest.TestCase):
    def test_request_limit_survives_settlement_reopen_and_cannot_be_raised(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'fresh-ledger.json'
            ledger = Ledger(path, 1, max_calls=1)
            ident = ledger.reserve('deepseek/deepseek-v4.1-flash', b'{}', 512, 'current-candidate')
            ledger.finish(ident, state='settled', charged_upper_nano_usd=0)
            with self.assertRaises(BudgetError):
                Ledger(path, 1).reserve('deepseek/deepseek-v4.1-flash', b'{}', 512, 'second')
            with self.assertRaises(BudgetError):
                Ledger(path, 1, max_calls=2)
            self.assertEqual(len(ledger.read()['calls']), 1)
    def test_dead_request_owner_becomes_unknown_without_releasing_cost(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'ledger.json'
            script = 'from nx_model_support import Ledger; import os,sys; Ledger(sys.argv[1],5).reserve("deepseek/deepseek-v4.1-flash",b"x",512,"crash"); os._exit(0)'
            subprocess.run([sys.executable, '-c', script, str(path)], cwd=Path(__file__).parent, check=True, timeout=15)
            ledger = Ledger(path, 5)
            reserved = ledger.read()['calls'][0]['reserved_nano_usd']
            with self.assertRaises(BudgetError):
                ledger.reserve('deepseek/deepseek-v4.1-flash', b'{}', 512, 'new')
            call = ledger.read()['calls'][0]
            self.assertEqual(call['state'], 'unknown')
            self.assertEqual(call['reserved_nano_usd'], reserved)
            self.assertEqual(len(ledger.read()['calls']), 1)

    def test_recycled_pid_cannot_own_a_reservation(self):
        with tempfile.TemporaryDirectory() as folder:
            ledger = Ledger(Path(folder) / 'ledger.json', 5)
            ledger.reserve('deepseek/deepseek-v4.1-flash', b'{}', 512, 'first')
            data = ledger.read(); data['calls'][0]['owner_identity'] = 'different-process-creation'
            atomic_json(ledger.path, data)
            with self.assertRaises(BudgetError):
                ledger.reserve('deepseek/deepseek-v4.1-flash', b'{}', 512, 'recycled')
            self.assertEqual(ledger.read()['calls'][0]['state'], 'unknown')

    def test_live_owner_allows_bounded_concurrent_requests(self):
        with tempfile.TemporaryDirectory() as folder:
            ledger = Ledger(Path(folder) / 'ledger.json', 5)
            for label in ['parent', 'child']:
                ledger.reserve('deepseek/deepseek-v4.1-flash', b'{}', 512, label)
            self.assertEqual([c['state'] for c in ledger.read()['calls']], ['reserved', 'reserved'])

    def test_parallel_processes_cannot_overspend_or_reset_cap(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'ledger.json'
            Ledger(path, 0.005)
            script = 'from nx_model_support import Ledger; import sys; l=Ledger(sys.argv[1],0.005); l.reserve("deepseek/deepseek-v4.1-flash",b"x",2048,"race")'
            processes = [subprocess.Popen([sys.executable, '-c', script, str(path)], cwd=Path(__file__).parent, stdout=subprocess.PIPE, stderr=subprocess.PIPE) for _ in range(2)]
            codes = []
            for process in processes:
                process.communicate(timeout=15)
                codes.append(process.returncode)
            self.assertEqual(sorted(codes), [0, 1])
            self.assertEqual(len(Ledger(path, 0.005).read()['calls']), 1)
            with self.assertRaises(BudgetError):
                Ledger(path, 5)

    def test_unknown_result_blocks_replay_and_keeps_reservation(self):
        with tempfile.TemporaryDirectory() as folder:
            ledger = Ledger(Path(folder) / 'ledger.json', 5)
            ident = ledger.reserve('deepseek/deepseek-v4.1-flash', b'{}', 512, 'first')
            ledger.finish(ident, state='unknown')
            with self.assertRaises(BudgetError):
                ledger.reserve('deepseek/deepseek-v4.1-flash', b'{}', 512, 'replay')
            self.assertGreater(ledger.read()['calls'][0]['reserved_nano_usd'], 0)

    def test_reported_usage_settles_without_inventing_cache_discount(self):
        with tempfile.TemporaryDirectory() as folder:
            ledger = Ledger(Path(folder) / 'ledger.json', 5)
            ident = ledger.reserve('deepseek/deepseek-v4-pro', b'{}', 512, 'first')
            cost = nano_cost(100, 50, 'deepseek/deepseek-v4-pro')
            ledger.finish(ident, state='settled', charged_upper_nano_usd=cost)
            self.assertEqual(cost, 330000)
            self.assertEqual(ledger.read()['calls'][0]['charged_upper_nano_usd'], cost)

    def test_stream_missing_usage_is_unknown_not_zero(self):
        raw = b'data: {"choices":[{"delta":{"content":"x"}}]}\n\ndata: [DONE]\n\n'
        usage, _ = usage_from_body(raw, True)
        self.assertIsNone(usage)

    def test_all_eighteen_frozen_seeds_have_objective_assertions(self):
        values = tasks()
        self.assertEqual(len(values), 18)
        self.assertEqual(sum(x['split'] == 'development' for x in values), 6)
        self.assertEqual(sum(x['split'] == 'held_out' for x in values), 12)
        self.assertEqual(len({x['id'] for x in values}), 18)
        for value in values:
            expected = value['seed_digest']
            content = {k: v for k, v in value.items() if k != 'seed_digest'}
            self.assertEqual(hashlib.sha256(json.dumps(content, sort_keys=True, ensure_ascii=False, separators=(',', ':')).encode()).hexdigest(), expected)
            self.assertTrue(value['checks'])
            self.assertTrue(all(not Path(name).is_absolute() and '..' not in Path(name).parts for name in value['files']))

if __name__ == '__main__':
    unittest.main()
