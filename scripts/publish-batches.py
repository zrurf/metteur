"""Preview or execute a frozen sequence of fast-forward pushes to the owner's fork."""
import argparse
import json
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--plan', required=True, type=Path)
mode = parser.add_mutually_exclusive_group()
mode.add_argument('--dry-run', action='store_true', help='Check push permission without changing refs')
mode.add_argument('--execute', action='store_true', help='Execute the reviewed manifest after owner acceptance')
args = parser.parse_args()
repo = Path(__file__).resolve().parents[1]
plan = json.loads(args.plan.read_text(encoding='utf-8-sig'))

def git(*values, allowed=(0,)):
    result = subprocess.run(['git', *values], cwd=repo, capture_output=True, text=True, encoding='utf-8', errors='replace', timeout=120)
    if result.returncode not in allowed:
        raise SystemExit(result.stderr.strip() or f'git exited {result.returncode}')
    return result.stdout.strip()

def require(condition, message):
    if not condition:
        raise SystemExit(message)

require(plan['fork_url'] == 'https://github.com/shizhende574-creator/metteur.git', 'Unexpected fork destination')
require(plan['upstream_url'] == 'https://github.com/zrurf/metteur.git', 'Unexpected upstream')
require(plan['target_branch'].startswith('codex/'), 'Expected codex/ branch')
require(git('remote', 'get-url', 'fork') == plan['fork_url'], 'Configured fork differs from frozen plan')
require(not git('status', '--porcelain'), 'Preserve and commit local changes before publication')
require(git('rev-parse', plan['candidate_branch']) == plan['expected_final_head'], 'Candidate advanced; regenerate the review manifest')
latest_main = git('ls-remote', plan['upstream_url'], 'refs/heads/main').split()[0]
require(latest_main == plan['upstream_main'], 'Upstream changed; review its diff before publishing')
batches = plan['batches']
previous = plan['common_base']
for batch in batches:
    git('merge-base', '--is-ancestor', previous, batch['head'])
    require(git('rev-parse', batch['head']) == batch['head'], 'A batch must use a full immutable commit SHA')
    previous = batch['head']
require(previous == plan['expected_final_head'], 'Last batch does not contain the exact reviewed candidate')
target = 'refs/heads/' + plan['target_branch']
remote = git('ls-remote', '--exit-code', 'fork', target, allowed=(0, 2))
remote_head = remote.split()[0] if remote else None
positions = {batch['head']: i + 1 for i, batch in enumerate(batches)}
require(remote_head is None or remote_head in positions, 'Fork branch changed outside this plan; no overwrite is allowed')
start = positions.get(remote_head, 0)
print(json.dumps({'mode': 'execute' if args.execute else 'dry-run' if args.dry_run else 'preview', 'fork': plan['fork_url'], 'target': target, 'completed_batches': start, 'remaining': batches[start:]}, ensure_ascii=False, indent=2), flush=True)
if args.dry_run:
    print(git('push', '--dry-run', '--porcelain', 'fork', f"{plan['expected_final_head']}:{target}"), flush=True)
elif args.execute:
    for batch in batches[start:]:
        print(f"Pushing batch {batch['number']}: {batch['title']}", flush=True)
        print(git('push', '--porcelain', 'fork', f"{batch['head']}:{target}"), flush=True)
        current = git('ls-remote', 'fork', target).split()[0]
        require(current == batch['head'], 'Remote acknowledgement differs; stop before the next batch')
    print('All frozen batches acknowledged. No PR was merged or made ready automatically.', flush=True)
