"""Run the frozen 12-cell stage within the authorized USD 3 shared ledger.

Each cell has a fresh process, database and repository. The relay rejects a
thirteenth request in a cell; unknown usage stops the shared ledger. No retries.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import threading
import time
import uuid
from nx_model_eval import Relay, RelayHandler, config_text, port, wait_listener, source_digest, REPO
from nx_model_support import Ledger, atomic_json, connection, BudgetError
from workflow_eval_fixtures import freeze, grade


def phase_ledger(path, request_cap, extension_reason=None):
    if not 144 <= request_cap <= 240:
        raise BudgetError('This phase allows at most 240 requests within the unchanged USD 3 budget')
    ledger = Ledger(path, 3, imported_nano_usd=0)
    with ledger.lock():
        data = ledger.read()
        old = data.get('max_calls')
        if old is None and not data['calls']:
            data['max_calls'] = request_cap
        elif old != request_cap:
            if not extension_reason or old > request_cap or any(c['state'] != 'settled' for c in data['calls']):
                raise BudgetError('An explicit audited extension with fully settled history is required')
            data.setdefault('request_cap_changes', []).append({
                'from': old, 'to': request_cap, 'reason': extension_reason,
                'calls_preserved': len(data['calls']), 'prior_sha256': hashlib.sha256(ledger.path.read_bytes()).hexdigest(),
                'authorization': 'User authorized autonomous real-model evaluation within USD 3; only the assistant-selected request cap changes.',
                'time': time.time(),
            })
            data['max_calls'] = request_cap
        atomic_json(ledger.path, data)
    return Ledger(path, 3, imported_nano_usd=0, max_calls=request_cap)


class WorkflowHandler(RelayHandler):
    def do_POST(self):
        # Keep the per-cell check and ledger reservation in one serialized transport operation.
        with self.server.transport_lock:
            super().do_POST()


class WorkflowRelay(Relay):
    def __init__(self, config, ledger):
        super().__init__(config, ledger)
        self.transport_lock=threading.Lock()
        self.RequestHandlerClass=WorkflowHandler

    def transform(self, body):
        calls=[c for c in self.ledger.read()['calls'] if c['label'].get('cell')==self.tag['cell']]
        if len(calls)>=12:
            raise BudgetError('The frozen cell reached twelve model requests')
        return body


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--credential-config',type=Path,required=True)
    parser.add_argument('--target',type=Path,required=True)
    parser.add_argument('--node',type=Path,default=Path('D:/Tocoding/Node/node.exe'))
    parser.add_argument('--existing-ledger',type=Path)
    parser.add_argument('--validate',nargs='+',choices=['fix_bug','add_tests','refactor','add_feature'])
    parser.add_argument('--validation-mode',choices=['plan_execute','plan_execute_supervised'],default='plan_execute')
    parser.add_argument('--request-cap',type=int,default=144)
    parser.add_argument('--cap-extension-reason')
    args=parser.parse_args()
    args.output.mkdir(parents=True,exist_ok=False)
    if args.validate and (not args.existing_ledger or not args.existing_ledger.is_file()):
        raise ValueError('Targeted validation must retain the original phase ledger')
    ledger=phase_ledger(args.existing_ledger or args.output/'ledger.json',args.request_cap,args.cap_extension_reason)
    bundle=freeze(args.output/'frozen.json',10 if args.validate else 8,args.validate,args.validation_mode,args.request_cap)
    atomic_json(args.output/'source.json',{'digest':source_digest(),'pricing_verified':'2026-10-24 Asia/Shanghai',
        'implementation_iterations':bundle['implementation_iterations'],
        'validation_only':bool(args.validate),
        'source':'https://commandcode.ai/models/deepseek-v4-1-flash',
        'accounting':'Conservative peak rates: input USD0.30/M, output USD1.20/M; no assumed cache discounts',
        'human_preparation':'Repository tasks, templates and grader were prepared by the coding assistant before the trial; this engineering time is outside measured per-cell wall time, not claimed zero.',
        'conditions':f"Same flash model, {bundle['implementation_iterations']} implementation iterations, 1024 output tokens, identical repository bytes and tool permissions; supervised mode adds one real assisted oversight checkpoint (2 review iterations, 512 output tokens). Total per-cell cap 12 requests. Counterbalanced order; no forced cache reuse. Exact public and added test commands are allowed; other controls denied. A failed oversight gate is explicitly continued without changing its failed conclusion or retrying. Approval counts are recorded; automated fixture decisions are not claimed human work."})
    relay=WorkflowRelay(connection(args.credential_config,'commandcode-flash'),ledger)
    threading.Thread(target=relay.serve_forever,daemon=True).start()
    summary={'cells':[],'budget_usd':3,'frozen_digest':bundle['digest'],'paid_calls':0}
    binaries=args.output/'bin';binaries.mkdir()
    for name in ['metteurd.exe','metteur-web.exe']:
        shutil.copy2(args.target/'debug'/name,binaries/name)
    try:
        for task_id,mode in bundle['order']:
            spec=next(t for t in bundle['tasks'] if t['id']==task_id)
            cell=task_id+'-'+mode;root=args.output/cell;root.mkdir()
            workspace=root/'workspace';workspace.mkdir();(workspace/'blueprints').mkdir()
            for name,body in spec['files'].items():(workspace/name).write_text(body,encoding='utf-8')
            # Only the user-visible task, never the independent grader/oracle, enters the driver.
            atomic_json(root/'task.json',{k:spec[k] for k in ['id','goal','allowed']})
            dp,wp=port(),port();relay.tag={'cell':cell+(':'+args.output.name if args.validate else ''),'id':task_id,'mode':mode,'variant':'candidate'}
            config=config_text(relay,'flash')
            start=config.index('whitelist = ');end=config.index('\n',start)
            config=config[:start]+'whitelist = ["py -3.12 check_public.py", "py -3.12 -m unittest test_added"]'+config[end:]
            config+='\n[oversight]\nmode = "'+('assisted' if mode.endswith('supervised') else 'off')+'"\nmodel = "flash"\nmax_review_iterations = 2\nmax_output_tokens = 512\nrun_token_budget = 8192\nreview_timeout_ms = 60000\n'
            (root/'config.toml').write_text(config,encoding='utf-8')
            ready={'baseURL':f'http://127.0.0.1:{wp}','workspace':workspace.as_posix(),'mode':mode,'max_iterations':bundle['implementation_iterations'],'instance_id':str(uuid.uuid5(uuid.NAMESPACE_URL,bundle['digest']+cell))}
            atomic_json(root/'ready.json',ready)
            before=len(ledger.read()['calls']);started=time.monotonic();children=[];handles=[]
            try:
                for name,exe,flags in [('daemon','metteurd.exe',['--config',str(root/'config.toml'),'--data-dir',str(root/'data'),'--listen-addr',f'127.0.0.1:{dp}']),
                                       ('web','metteur-web.exe',['--daemon-addr',f'127.0.0.1:{dp}','--listen-addr',f'127.0.0.1:{wp}','--static-dir',str(REPO/'modules/webcore/dist')])]:
                    handle=(root/(name+'.log')).open('wb');handles.append(handle)
                    child=subprocess.Popen([str(binaries/exe),*flags],cwd=REPO,stdout=handle,stderr=subprocess.STDOUT,creationflags=subprocess.CREATE_NO_WINDOW)
                    children.append(child);wait_listener(child,dp if name=='daemon' else wp)
                completed=subprocess.run([str(args.node),str(REPO/'modules/webcore/scripts/workflow-eval-case.mjs'),str(root/'ready.json'),str(root/'task.json'),str(root/'driver.json')],cwd=REPO/'modules/webcore',capture_output=True,timeout=300)
                (root/'driver.log').write_bytes(completed.stdout+completed.stderr)
                host_manifest=root/'driver.json.host-files.json'
                result=grade(spec,workspace,json.loads(host_manifest.read_text()) if host_manifest.exists() else {})
                result.update(mode=mode,driver_exit=completed.returncode,wall_seconds=round(time.monotonic()-started,2),model_requests=len(ledger.read()['calls'])-before)
            except Exception as error:
                result={'task':task_id,'mode':mode,'passed':False,'error':type(error).__name__}
            finally:
                for child in reversed(children):
                    if child.poll() is None:child.terminate()
                for child in children:
                    try:child.wait(timeout=10)
                    except subprocess.TimeoutExpired:child.kill();child.wait()
                for handle in handles:handle.close()
            atomic_json(root/'grade.json',result);summary['cells'].append(result)
            summary['paid_calls']=len(ledger.read()['calls']);atomic_json(args.output/'summary.json',summary)
            print(json.dumps(result),flush=True)
            if any(c['state']=='unknown' for c in ledger.read()['calls']):break
    finally:
        relay.shutdown();relay.server_close()
        atomic_json(args.output/'summary.json',summary)


if __name__=='__main__':main()
