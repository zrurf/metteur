#!/usr/bin/env python3
"""Aggregate exported content-free llm.observation audit rows. Never calls models."""
import argparse,json,statistics
from collections import defaultdict
from pathlib import Path

def summarize(rows):
    calls={}
    for row in rows:
        op=row.get('operation')
        if op not in ('llm.observation.begin','llm.observation.end'): continue
        d=row.get('detail',{});identity=(str(d.get('run_id')),str(d.get('call_id')))
        calls.setdefault(identity,{})['begin' if op.endswith('begin') else 'end']=d
    groups=defaultdict(list)
    for call in calls.values():
        b,e=call.get('begin',{}),call.get('end',{})
        r=b.get('request',{})
        key=tuple(str(b.get(k,e.get(k,'unknown'))) for k in ('provider','model','role'))+(str(r.get('endpoint_digest','unknown')),str(r.get('key_id','unknown')),str(r.get('cache_condition','unknown')))
        groups[key].append(call)
    result=[]
    for key,items in sorted(groups.items()):
        known_input=known_read=read_calls=known_total=total_calls=write_calls=written=0
        latency=[];ttft=[];costs=[];currencies=set();completed=0
        for item in items:
            e=item.get('end',{});u=e.get('usage') or {}
            inp=(u.get('normalized_input') or {}).get('value')
            read=(u.get('cache_read') or {}).get('value')
            write=(u.get('cache_write') or {}).get('value')
            if isinstance(inp,int):known_total+=inp;total_calls+=1
            if isinstance(inp,int) and isinstance(read,int) and inp>=read>=0 and u.get('cache_counter_consistent',False):
                known_input+=inp;known_read+=read;read_calls+=1
            if isinstance(write,int):written+=write;write_calls+=1
            if isinstance(e.get('elapsed_ms'),(int,float)):latency.append(e['elapsed_ms'])
            if isinstance(e.get('ttft_ms'),(int,float)):ttft.append(e['ttft_ms'])
            c=e.get('cost')
            if isinstance(c,dict) and isinstance(c.get('micros'),int):
                costs.append(c['micros']);currencies.add(c.get('currency'))
            completed+=int(e.get('outcome')=='response')
        count=len(items)
        result.append(dict(zip(('provider','model','role','endpoint_digest','fingerprint_epoch','declared_cache_condition'),key),calls=count,responses=completed,
            known_input_calls=total_calls,known_input_tokens=known_total if total_calls else None,
            cache_read_coverage_calls=read_calls,cache_read_coverage_fraction=read_calls/count,
            cache_read_tokens=known_read if read_calls else None,cache_read_input_denominator=known_input if read_calls else None,
            cache_hit_ratio=known_read/known_input if known_input else None,
            cache_write_coverage_calls=write_calls,cache_write_tokens=written if write_calls else None,
            latency_median_ms=statistics.median(latency) if latency else None,
            ttft_median_ms=statistics.median(ttft) if ttft else None,ttft_known_calls=len(ttft),
            known_cost_micros=sum(costs) if costs and len(currencies)==1 else None,
            cost_complete=len(costs)==count and len(currencies)==1,
            currency=next(iter(currencies)) if len(currencies)==1 else None))
    return {'schema_version':1,'source':'host_observation_export','groups':result,
            'limits':['Unknown values remain null. Known cost is a partial sum unless cost_complete.',
                      'Cold/warm labels never prove provider cache reuse.',
                      'Fingerprints from different epochs cannot establish equal content.',
                      'No correctness, safety or speed improvement is inferred from usage alone.']}

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input',required=True,type=Path)
    parser.add_argument('--output',required=True,type=Path)
    args=parser.parse_args()
    if args.input.stat().st_size>32*1024*1024:parser.error('Use a bounded per-experiment audit export (at most 32 MiB).')
    rows=json.loads(args.input.read_text())
    if not isinstance(rows,list):parser.error('Expected an audit-entry JSON array.')
    with args.output.open('x') as out:json.dump(summarize(rows),out,ensure_ascii=False,indent=2)
if __name__=='__main__':main()
