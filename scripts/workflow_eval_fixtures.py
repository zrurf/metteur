"""Frozen repository tasks and independent graders for workflow delivery.

The grader is outside the model workspace. No model response is a verdict.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def tasks():
    interval = '''def merge_ranges(ranges):
    ordered = sorted(ranges)
    result = []
    for start, end in ordered:
        if start > end:
            raise ValueError("reversed range")
        if result and start < result[-1][1]:
            result[-1] = (result[-1][0], max(end, result[-1][1]))
        else:
            result.append((start, end))
    return result
'''
    parser = '''def parse_labels(text):
    if not isinstance(text, str):
        raise TypeError("text required")
    result = []
    for part in text.split(","):
        label = part.strip()
        if label and label not in result:
            result.append(label)
    return result
'''
    grouped = '''def summarize(rows):
    names = []
    for row in rows:
        if row[0] not in names:
            names.append(row[0])
    result = []
    for name in names:
        count = 0
        total = 0
        for key, value in rows:
            if name == key:
                count += 1
                total += value
        result.append((name, count, total))
    return result
'''
    feature = '''def paginate(items, page, size):
    raise NotImplementedError("pagination")
'''
    public = {
        'fix_bug': 'from app import merge_ranges\nassert merge_ranges([(1, 2), (2, 4)]) == [(1, 4)]\n',
        'add_tests': 'from app import parse_labels\nassert parse_labels("one,two") == ["one", "two"]\n',
        'refactor': 'from app import summarize\nassert summarize([("a", 2), ("b", 1), ("a", 3)]) == [("a", 2, 5), ("b", 1, 1)]\n',
        'add_feature': 'import app\nassert callable(app.paginate)\n',
    }
    definitions = [
        ('fix_bug', interval,
         'Fix merge_ranges so overlapping or touching closed ranges merge, including chains, zero-width and negative ranges. Preserve sorted tuple output, empty input, ValueError on reversed ranges and caller input. Add regression tests.',
         'from app import merge_ranges as f\nassert f([])==[]\nassert f([(5,8),(1,3),(3,5)])==[(1,8)]\nassert f([(-3,-1),(-1,0),(2,2)])==[(-3,0),(2,2)]\nx=[(4,5),(1,2)]; assert f(x)==[(1,2),(4,5)] and x==[(4,5),(1,2)]\ntry: f([(3,1)])\nexcept ValueError: pass\nelse: raise AssertionError("reversed range accepted")\n',
         interval.replace('start < result', 'start <= result')),
        ('add_tests', parser,
         'Add test_added.py using unittest for parse_labels. Cover blank/whitespace, trimming, empty comma segments, duplicate removal in first-seen order, case sensitivity and non-string TypeError. Keep app.py unchanged. Tests must detect removal of trimming and duplicate filtering.',
         'from app import parse_labels as f\nassert f(" , a, a,B,b,, ")==["a","B","b"]\nassert f("")==[]\ntry: f(None)\nexcept TypeError: pass\nelse: raise AssertionError("type check missing")\n', parser),
        ('refactor', grouped,
         'Refactor summarize from repeated scanning to a single aggregation pass. Preserve function signature, first-seen key order, tuple rows, counts, sums and no input mutation; support a generator of input rows. Add regression tests; use the standard library only.',
         'from app import summarize as f\nassert f([])==[]\nx=[("b",-2),("a",0),("b",5)]; before=list(x)\nassert f(x)==[("b",2,3),("a",1,0)] and x==before\nassert f(iter(x))==[("b",2,3),("a",1,0)]\n',
         'def summarize(rows):\n    groups = {}\n    for key, value in rows:\n        count, total = groups.get(key, (0, 0))\n        groups[key] = (count + 1, total + value)\n    return [(key, count, total) for key, (count, total) in groups.items()]\n'),
        ('add_feature', feature,
         'Implement paginate(items,page,size) with one-based pages. Return a new list slice, retain item order and caller list. Empty or beyond-last pages return []. page and size must be positive integers; reject booleans, floats, nonpositive and nonintegers with ValueError. Add normal, boundary, invalid-input and regression tests.',
         'from app import paginate as f\nx=list(range(7)); assert f(x,1,3)==[0,1,2] and f(x,3,3)==[6]\nassert f(x,4,3)==[] and f([],1,2)==[] and x==list(range(7))\nassert f(x,1,10) is not x\nfor p,s in [(0,2),(1,0),(-1,2),(1,2.5),(True,2),(1,False),("1",2)]:\n try: f(x,p,s)\n except ValueError: pass\n else: raise AssertionError((p,s))\n',
         'def paginate(items, page, size):\n    if type(page) is not int or type(size) is not int or page < 1 or size < 1:\n        raise ValueError("positive integers required")\n    return list(items[(page - 1) * size:page * size])\n'),
    ]
    return [dict(id=kind, goal=goal, files={'app.py': source, 'check_public.py': public[kind], 'README.md': goal+'\n'},
                 allowed=['app.py', 'test_added.py'] if kind != 'add_tests' else ['test_added.py'],
                 grader=grader, oracle=oracle) for kind, source, goal, grader, oracle in definitions]


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def grade(spec, workspace, host_files=None):
    workspace = Path(workspace)
    before = spec['files']
    after = {p.relative_to(workspace).as_posix(): p.read_text(encoding='utf-8')
             for p in workspace.rglob('*') if p.is_file() and not any(x in p.relative_to(workspace).parts for x in ('.metteur', '__pycache__', '.git'))}
    for name, content in (host_files or {}).items():
        if after.get(name) == content:
            after.pop(name)
    changes = sorted(p for p in set(before) | set(after) if before.get(p) != after.get(p))
    checks = {'scope': all(p in spec['allowed'] for p in changes), 'actual_change': bool(changes)}
    with tempfile.TemporaryDirectory(prefix='workflow-independent-grader-') as directory:
        root = Path(directory)
        for name, content in after.items():
            target = root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content, encoding='utf-8')
        (root / 'independent_check.py').write_text(spec['grader'], encoding='utf-8')
        def run(args):
            try:
                result = subprocess.run([sys.executable, *args], cwd=root, capture_output=True, timeout=15,
                                        env={**os.environ, 'PYTHONDONTWRITEBYTECODE': '1'})
                return result.returncode == 0 and (args[:2] != ['-m', 'unittest'] or b'Ran 0 tests' not in result.stderr)
            except subprocess.TimeoutExpired:
                return False
        checks['independent_behavior'] = run(['independent_check.py'])
        checks['public_regression'] = run(['check_public.py'])
        checks['added_tests_present'] = bool(after.get('test_added.py', '').strip())
        checks['added_tests_pass'] = checks['added_tests_present'] and run(['-m', 'unittest', 'test_added'])
        if spec['id'] == 'add_tests':
            for name, old, new in [('trim', 'part.strip()', 'part'), ('duplicates', 'if label and label not in result:', 'if label:')]:
                (root / 'app.py').write_text(before['app.py'].replace(old, new), encoding='utf-8')
                # Avoid stale bytecode when equally sized mutants execute within one timestamp tick.
                checks['detect_'+name] = not run(['-B', '-m', 'unittest', 'test_added'])
    return {'task': spec['id'], 'changes': changes, 'checks': checks, 'passed': all(checks.values())}


def freeze(destination, iterations=8, selected=None, validation_mode='plan_execute', request_cap=144):
    destination = Path(destination)
    bundle = {'schema': 1, 'model_alias': 'flash', 'budget_usd': 3, 'max_requests': request_cap, 'implementation_iterations': iterations,
              'modes': ['react', 'plan_execute', 'plan_execute_supervised'], 'tasks': tasks(),
              'order': [(t['id'], mode) for index,t in enumerate(tasks()) for mode in
                        (['react','plan_execute','plan_execute_supervised'][index%3:]+['react','plan_execute','plan_execute_supervised'][:index%3])]}
    if selected:
        bundle['order'] = [(task,mode) for task,mode in bundle['order'] if task in selected and mode == validation_mode]
        bundle['purpose'] = 'Targeted delivery validation after first-pass failures; excluded from the original matched comparison.'
    bundle['digest'] = digest(bundle)
    with destination.open('x', encoding='utf-8') as stream:
        json.dump(bundle, stream, indent=2)
    return bundle


if __name__ == '__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=Path)
    args=parser.parse_args()
    print(json.dumps({'digest':freeze(args.destination)['digest'], 'paid_calls':0}))
