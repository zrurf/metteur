import concurrent.futures
import json
import threading
import time
import unittest
import tempfile
from pathlib import Path
import urllib.error
import urllib.request
from unittest.mock import patch
from workflow_model_eval import WorkflowRelay, phase_ledger
from nx_model_support import atomic_json, BudgetError


class PerCellCapTests(unittest.TestCase):
    def test_explicit_cap_extension_preserves_cost_and_call_history(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'ledger.json'
            ledger=phase_ledger(path,144)
            data=ledger.read()
            data['calls']=[{'state':'settled','id':'original','charged_upper_nano_usd':123}]
            atomic_json(path,data)
            with self.assertRaises(BudgetError):phase_ledger(path,240)
            extended=phase_ledger(path,240,'Retest verified job-delivery defect; complete missing supervisory coverage').read()
            self.assertEqual(extended['calls'],data['calls'])
            self.assertEqual(extended['fingerprint_epoch'],data['fingerprint_epoch'])
            self.assertEqual(extended['limit_nano_usd'],3_000_000_000)
            self.assertEqual(extended['request_cap_changes'][0]['from'],144)
            with self.assertRaises(BudgetError):phase_ledger(path,241,'not permitted')
            extended['calls'][0]['state']='unknown'
            atomic_json(path,extended)
            with self.assertRaises(BudgetError):phase_ledger(path,144,'cannot discard unknown')

    def test_concurrent_requests_cannot_both_consume_the_last_slot(self):
        calls=[{'label':{'cell':'test'}} for _ in range(11)]
        class Ledger:
            def read(self): return {'calls':calls}
        relay=WorkflowRelay({},Ledger())
        relay.tag={'cell':'test','mode':'plan_execute','variant':'candidate'}
        threading.Thread(target=relay.serve_forever,daemon=True).start()
        def paid(config,ledger,payload,label):
            time.sleep(.05)
            calls.append({'label':label})
            return b'{}',None,None
        def send():
            request=urllib.request.Request(f'http://127.0.0.1:{relay.server_port}/v1/chat/completions',
                data=json.dumps({'max_tokens':1}).encode(),headers={'Authorization':'Bearer '+relay.fake_key})
            try:
                with urllib.request.urlopen(request,timeout=5) as response:return response.status
            except urllib.error.HTTPError as error:return error.code
        try:
            with patch('nx_model_eval.paid_request',side_effect=paid) as transport, concurrent.futures.ThreadPoolExecutor(2) as pool:
                result=list(pool.map(lambda _:send(),range(2)))
                self.assertEqual(sorted(result),[200,502])
                self.assertEqual(transport.call_count,1)
                self.assertEqual(len(calls),12)
        finally:relay.shutdown();relay.server_close()

if __name__=='__main__':unittest.main()
