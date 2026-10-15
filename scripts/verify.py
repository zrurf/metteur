"""Reproducible gates with immutable first-run logs and real exit codes."""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

# Windows consoles may use a legacy code page while pnpm emits UTF-8 glyphs.
sys.stdout.reconfigure(encoding='utf-8', errors='replace')
sys.stderr.reconfigure(encoding='utf-8', errors='replace')

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--scope', choices=['checks', 'all', 'rust', 'web', 'live'], default='checks')
parser.add_argument('--evidence-dir', type=Path)
parser.add_argument('--temp-dir', type=Path)
parser.add_argument('--target-dir', type=Path)
parser.add_argument('--diagnostic', action='store_true', help='Label a diagnostic run; never counts as a first-run gate')
args = parser.parse_args()
repo = Path(__file__).resolve().parents[1]
web = repo / 'modules' / 'webcore'
parent = (args.evidence_dir or repo / '.tmp' / 'validation').resolve()
parent.mkdir(parents=True, exist_ok=True)
evidence = Path(tempfile.mkdtemp(prefix='diagnostic-' if args.diagnostic else 'first-', dir=parent))
env = os.environ.copy()
temp = (args.temp_dir or evidence / 'tmp').resolve()
temp.mkdir(parents=True, exist_ok=True)
env.update(TMPDIR=str(temp), TMP=str(temp), TEMP=str(temp), CARGO_BUILD_JOBS='1',
           CI='1', VITE_MOCK='0', VITE_GW_URL='', PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD='1',
           METTEUR_VALIDATION_DIR=str(evidence), E2E_OUTPUT_DIR=str(evidence / 'browser'))
if args.target_dir:
    env['CARGO_TARGET_DIR'] = str(args.target_dir.resolve())
else:
    env['CARGO_TARGET_DIR'] = str(Path(env.get('CARGO_TARGET_DIR', repo / 'target')).resolve())
dist = evidence / 'web'
env['METTEUR_WEB_DIST'] = str(dist if args.scope in ['all', 'web'] else Path(env.get('METTEUR_WEB_DIST', web / 'dist')).resolve())
if args.scope == 'all':
    suffix = '.exe' if sys.platform == 'win32' else ''
    for variable, name in [('METTEUR_DAEMON_BIN', 'metteurd'), ('METTEUR_WEB_BIN', 'metteur-web')]:
        env[variable] = str(Path(env['CARGO_TARGET_DIR']) / 'debug' / (name + suffix))
pnpm = [shutil.which('corepack') or 'corepack', 'pnpm']

def output(command, cwd=repo):
    try:
        result = subprocess.run(command, cwd=cwd, env=env, capture_output=True, text=True,
                                encoding='utf-8', errors='replace', check=False)
        return {'exit_code': result.returncode, 'output': result.stdout.strip() + result.stderr.strip()}
    except OSError as error:
        return {'exit_code': 127, 'output': str(error)}

def source_digest():
    paths = subprocess.check_output(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=repo).split(b'\0')
    digest = hashlib.sha256()
    for raw in sorted(set(paths)):
        if not raw:
            continue
        path = repo / os.fsdecode(raw)
        if path.is_file():
            digest.update(raw + b'\0' + hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()

version_commands = {'python': [sys.executable, '--version']}
if args.scope in ['all', 'rust']:
    version_commands.update(rust=['rustc', '-Vv'], cargo=['cargo', '-V'])
if args.scope in ['checks', 'all', 'web', 'live']:
    version_commands.update(node=['node', '--version'], pnpm=pnpm + ['--version'])

report = {
    'schema_version': 1, 'scope': args.scope, 'diagnostic': args.diagnostic,
    'platform': sys.platform, 'head': output(['git', 'rev-parse', 'HEAD']),
    'worktree': output(['git', 'status', '--short']), 'source_sha256': source_digest(),
    'started_at_utc': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
    'versions': {name: output(command, web if name == 'pnpm' else repo)
                 for name, command in version_commands.items()},
    'locks': {name: hashlib.sha256((repo / name).read_bytes()).hexdigest() for name in [
        'Cargo.lock', 'modules/webcore/pnpm-lock.yaml']},
    'test_threads': env.get('RUST_TEST_THREADS', 'default concurrency'), 'retries': 0,
    'temp_dir': str(temp), 'target_dir': env.get('CARGO_TARGET_DIR', str(repo / 'target')), 'steps': [],
}

def save():
    (evidence / 'result.json').write_text(json.dumps(report, indent=2), encoding='utf-8')

steps = []
if args.scope in ['all', 'rust']:
    steps += [(stage, ['cargo', stage, '--locked', '-j', '1', '--workspace'] + (['--all-targets'] if stage == 'clippy' else []), repo)
              for stage in ['build', 'test', 'clippy']]
if args.scope in ['checks', 'all', 'web']:
    steps += [('install', pnpm + ['install', '--frozen-lockfile'], web),
              ('typecheck', pnpm + ['typecheck'], web), ('lint', pnpm + ['lint'], web),
              ('live-guards', ['node', '--test', 'scripts/live-identity.test.mjs'], web)]
if args.scope in ['all', 'web']:
    steps += [('web-build', pnpm + ['build', '--outDir', str(dist)], web),
              ('e2e', pnpm + ['test:e2e', '--workers=1', '--retries=0'], web)]
if args.scope in ['all', 'live']:
    steps += [('live', pnpm + ['test:live'], web)]

print(f'Validation evidence: {evidence}', flush=True)
save()
if env.get('RUST_TEST_THREADS') and not args.diagnostic:
    report['exit_code'] = 2
    report['preflight_failure'] = 'RUST_TEST_THREADS is set; remove it for the concurrent gate, or label this --diagnostic'
    save()
    print(report['preflight_failure'], file=sys.stderr)
    sys.exit(2)
expected = {'python': r'^Python 3\.(11|12)\.'}
if args.scope in ['all', 'rust']:
    expected.update(rust=r'^rustc 1\.99\.0 ', cargo=r'^cargo 1\.99\.0 ')
if args.scope in ['checks', 'all', 'web', 'live']:
    expected.update(node=r'^v24\.13\.1$', pnpm=r'^11\.24\.0$')
for name, pattern in expected.items():
    version = report['versions'][name]
    if version['exit_code'] or not re.search(pattern, version['output'], re.MULTILINE):
        report['exit_code'] = 2
        report['preflight_failure'] = f'{name} does not match the documented toolchain: {version}'
        save()
        print(report['preflight_failure'], file=sys.stderr)
        sys.exit(2)
for name, command, cwd in steps:
    started = time.monotonic()
    print(f'Running {name}', flush=True)
    with (evidence / f'{name}.log').open('w', encoding='utf-8') as log:
        try:
            process = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding='utf-8', errors='replace')
            for line in process.stdout:
                log.write(line)
                print(line, end='', flush=True)
            code = process.wait()
        except OSError as error:
            log.write(str(error))
            code = 127
    report['steps'].append({'name': name, 'command': command, 'cwd': str(cwd),
                            'exit_code': code, 'seconds': round(time.monotonic() - started, 3)})
    save()
    if code:
        report['exit_code'] = code
        save()
        print(f'FAILED {name} ({code}); subsequent steps were not run. See {evidence}', file=sys.stderr)
        sys.exit(code)
report['exit_code'] = 0
if source_digest() != report['source_sha256']:
    report['exit_code'] = 1
    save()
    sys.exit('Source changed during validation; this run cannot certify the resulting worktree')
save()
print('Diagnostic completed' if args.diagnostic else 'Selected validation gates passed')
