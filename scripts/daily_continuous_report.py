"""Summarize the existing observation; never drives UI or repeats acceptance."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import time


def read_json(path):
    # status.json is written in place by the already-running observer.
    for attempt in range(3):
        try:
            return json.loads(path.read_text(encoding='utf-8'))
        except json.JSONDecodeError:
            if attempt == 2:
                raise
            time.sleep(0.05)


def summarize(directory):
    status = read_json(directory / 'status.json')
    raw = (directory / 'samples.jsonl').read_text(encoding='utf-8')
    lines = raw.splitlines()
    if raw and not raw.endswith('\n'):
        lines = lines[:-1]
    samples = [json.loads(line) for line in lines if line.strip()]
    heaps, native = [[], []], {}
    for sample in samples:
        for index, heap in enumerate(sample['heap']):
            heaps[index].append(heap['usedSize'])
        for process in sample['processes']:
            native.setdefault(str(process['Id']), []).append(process['PrivateMemorySize64'])
    complete = (
        status['status'] == 'PASSED'
        and status.get('continuousWindowCompleted') is True
        and status['elapsedMs'] >= status['requestedMs'] >= 8 * 3600000
        and not status['errors']
        and not status['longSamplingGaps']
    )
    records = [sample['records'] for sample in samples]
    checks = {
        'samples_match_producer': len(samples) == status['samples'],
        'metadata_rows_bounded': bool(records) and min(records) == max(records) == 4,
        'two_js_heaps_within_frozen_guard': all(values and max(values) <= status['heapGuardBytes'][index] for index, values in enumerate(heaps)),
        'native_within_frozen_guards': bool(native) and all(max(values) <= status['nativePrivateGuards'][pid] for pid, values in native.items()),
    }
    state = 'READY_FOR_FINAL_REVIEW' if complete and all(checks.values()) else 'IN_PROGRESS' if status['status'] == 'RUNNING' else 'REQUIRES_REVIEW'
    return {
        'recordedUtc': datetime.now(timezone.utc).isoformat(),
        'state': state, 'observerStatus': status, 'checks': checks,
        'sampleCount': len(samples), 'metadataRowRange': [min(records), max(records)] if records else [],
        'postGcHeapMaxBytes': [max(values) if values else None for values in heaps],
        'nativePrivateMaxBytes': {pid: max(values) for pid, values in native.items()},
        'firstSampleUtc': samples[0]['at'] if samples else None,
        'lastSampleUtc': samples[-1]['at'] if samples else None,
        'scope': 'Original 19093 Web, unchanged native/input/event/storage implementation; UI 19103 accepted separately. Not relabelled as 8h on the new Web process.',
        'reuse': 'AC-19 fixed candidate performance: s4-performance-ui-r1. AC-28 storage bounds/zero business notifications: existing Windows/Linux recovery tests. No new business or model tests.',
        'boundary': 'Evidence only. Does not edit the repository, sign human IME/sleep results or mark the whole phase complete.',
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--observation', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--wait', action='store_true')
    parser.add_argument('--ready-marker', type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    print(json.dumps({'watching': str(args.observation), 'output': str(args.output)}), flush=True)
    while True:
        result = summarize(args.observation)
        observer = result['observerStatus']
        updated = datetime.fromisoformat(observer['updated'].replace('Z', '+00:00'))
        stale = (datetime.now(timezone.utc) - updated).total_seconds() > 180
        if stale and observer['status'] == 'RUNNING':
            result.update(state='REQUIRES_REVIEW', collectorReason='Observer status stale; no success inferred')
        temporary = args.output / 'continuous-result.tmp'
        temporary.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        temporary.replace(args.output / 'continuous-result.json')
        if result['state'] == 'READY_FOR_FINAL_REVIEW' and args.ready_marker:
            marker = args.ready_marker.resolve()
            if not marker.exists():
                with marker.open('x', encoding='utf-8') as stream:
                    stream.write('8小时连续观察证据已采齐，可以按清单进行真实睡眠/唤醒验收。\n')
                    stream.write('这不是人工项目或整个阶段的通过结论。\n')
                    stream.write('记录时间UTC：' + result['recordedUtc'] + '\n')
        if not args.wait or observer['status'] != 'RUNNING' or stale:
            print(json.dumps({'state': result['state'], 'samples': result['sampleCount']}), flush=True)
            return
        time.sleep(60)


if __name__ == '__main__':
    main()
