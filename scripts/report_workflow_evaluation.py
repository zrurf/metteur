"""Publish only aggregate evidence; credentials, request bodies and ledger identities stay local."""
import argparse
import json
from pathlib import Path

def read(path):return json.loads(path.read_text(encoding='utf-8'))

def report(root):
    trials=[('model-evaluation-first','model-runtime-audit.json'),
            ('model-delivery-validation','model-delivery-validation-audit.json'),
            ('model-bug-final-validation','model-bug-final-validation-audit.json'),
            ('model-job-fix-supervised','model-job-fix-supervised-audit.json')]
    ledger=read(root/'model-evaluation-first/ledger.json')
    result={'schema':1,'budget_usd':3,'request_cap':ledger['max_calls'],'paid_requests':len(ledger['calls']),
            'charged_upper_usd':sum(c.get('charged_upper_nano_usd',c['reserved_nano_usd']) for c in ledger['calls'])/1e9,
            'unsettled_requests':sum(c['state']!='settled' for c in ledger['calls']),
            'usage':{},'trials':[]}
    for output,field,detail in [('input_tokens','prompt_tokens',None),('output_tokens','completion_tokens',None),
                              ('cached_input_tokens','cached_tokens','prompt_tokens_details'),
                              ('reasoning_tokens','reasoning_tokens','completion_tokens_details')]:
        values=[(c.get('usage') or {}).get(detail,{}) if detail else (c.get('usage') or {}) for c in ledger['calls']]
        result['usage'][output]={'reported_total':sum(v.get(field,0) or 0 for v in values),'coverage':sum(field in v for v in values),'requests':len(values)}
    for name,audit_file in trials:
        path=root/name
        frozen=read(path/'frozen.json');audit=read(root/audit_file);summary=read(path/'summary.json')
        trial={'name':name,'frozen_digest':frozen['digest'],'source_digest':read(path/'source.json')['digest'],
               'implementation_iterations':frozen.get('implementation_iterations',8),'cells':[]}
        for cell in summary['cells']:
            checked=next(a for a in audit if a['task']==cell['task'] and a['mode']==cell['mode'])
            label=cell['task']+'-'+cell['mode']
            if name=='model-delivery-validation':label+='-validation'
            elif name!='model-evaluation-first':label+=':'+name
            calls=[c for c in ledger['calls'] if c['label'].get('cell')==label]
            assert len(calls)==cell['model_requests'],(name,label,len(calls),cell['model_requests'])
            trial['cells'].append({**cell,**checked,'artifact_passed':cell['passed'],
                'complete_delivery':cell['passed'] and checked['runtime_completed'] and not checked['unexpected_files'] and checked['blueprint_matches_frozen_run'] is not False,
                'charged_upper_usd':sum(c['charged_upper_nano_usd'] for c in calls)/1e9,
                'automated_approval_count':read(path/(cell['task']+'-'+cell['mode'])/'driver.json').get('approvals',0)})
        result['trials'].append(trial)
    return result

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('root',type=Path);p.add_argument('output',type=Path);args=p.parse_args()
    data=report(args.root);args.output.write_text(json.dumps(data,indent=2),encoding='utf-8')
    print(json.dumps({k:v for k,v in data.items() if k!='trials'}))
