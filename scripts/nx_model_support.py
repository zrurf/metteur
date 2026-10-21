"""Bounded operator-owned model transport with a durable, shared cost ledger.

Only configuration paths enter the command line. Credentials and request/response
content never enter ordinary receipts. No automatic retries or redirects.
"""
from contextlib import contextmanager
from datetime import datetime, timezone
from decimal import Decimal, ROUND_CEILING
import hashlib
import hmac
import json
import os
from pathlib import Path
import secrets
import time
import tomllib
import urllib.error
import urllib.request
from urllib.parse import urlsplit
import uuid

RATES = {
    'deepseek/deepseek-v4.1-flash': {'input': '0.30', 'output': '1.20'},
    'deepseek/deepseek-v4-pro': {'input': '1.32', 'output': '3.96'},
}
RATE_DATE = '2026-10-21 Asia/Shanghai'
RATE_SOURCES = ['https://commandcode.ai/models/deepseek-v4-1-flash', 'https://commandcode.ai/models/deepseek-v4-pro']

def atomic_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + '.' + uuid.uuid4().hex + '.tmp')
    with temporary.open('xb') as stream:
        stream.write((json.dumps(value, ensure_ascii=False, indent=2) + '\n').encode('utf-8'))
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)

def nano_cost(input_tokens, output_tokens, model):
    rates = RATES[model]
    value = (Decimal(input_tokens) * Decimal(rates['input']) + Decimal(output_tokens) * Decimal(rates['output'])) * 1000
    return int(value.to_integral_value(rounding=ROUND_CEILING))

def process_identity(pid):
    """Live process creation identity, so a recycled PID never owns a receipt."""
    try:
        if not isinstance(pid, int) or pid <= 0:
            return None
        if os.name == 'nt':
            import ctypes
            from ctypes import wintypes
            kernel = ctypes.WinDLL('kernel32', use_last_error=True)
            kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
            kernel.OpenProcess.restype = wintypes.HANDLE
            kernel.CloseHandle.argtypes = [wintypes.HANDLE]
            kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
            kernel.WaitForSingleObject.restype = wintypes.DWORD
            kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
            handle = kernel.OpenProcess(0x00101000, False, pid)
            if not handle:
                return None
            try:
                if kernel.WaitForSingleObject(handle, 0) != 258:
                    return None
                times = [wintypes.FILETIME() for _ in range(4)]
                if not kernel.GetProcessTimes(handle, *(ctypes.byref(value) for value in times)):
                    return None
                created = (times[0].dwHighDateTime << 32) | times[0].dwLowDateTime
                return f'windows:{created}'
            finally:
                kernel.CloseHandle(handle)
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        if fields[0] in ('Z', 'X'):
            return None
        boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
        return f'linux:{boot}:{fields[19]}'
    except (OSError, ValueError, IndexError):
        return None


class BudgetError(RuntimeError):
    pass

class Ledger:
    def __init__(self, path, limit_usd, imported_nano_usd=0):
        self.path = Path(path).resolve()
        self.limit = int(Decimal(str(limit_usd)) * 1_000_000_000)
        if self.limit <= 0:
            raise BudgetError('A positive explicit budget is required')
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.lock():
            if not self.path.exists():
                atomic_json(self.path, {'schema_version': 1, 'limit_nano_usd': self.limit, 'imported_estimated_nano_usd': imported_nano_usd, 'calls': [], 'fingerprint_epoch': secrets.token_hex(16)})
            if self.read()['limit_nano_usd'] != self.limit:
                raise BudgetError('Existing ledger limit differs; never silently reset or raise it')

    @contextmanager
    def lock(self):
        # Kernel lock survives process failure without a stale-lock bypass.
        with self.path.with_suffix(self.path.suffix + '.lock').open('a+b') as stream:
            stream.seek(0, os.SEEK_END)
            if stream.tell() == 0:
                stream.write(b'0')
                stream.flush()
            stream.seek(0)
            if os.name == 'nt':
                import msvcrt
                msvcrt.locking(stream.fileno(), msvcrt.LK_LOCK, 1)
                try:
                    yield
                finally:
                    stream.seek(0)
                    msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl
                fcntl.flock(stream, fcntl.LOCK_EX)
                try:
                    yield
                finally:
                    fcntl.flock(stream, fcntl.LOCK_UN)

    def read(self):
        return json.loads(self.path.read_text(encoding='utf-8'))

    def reserve(self, model, raw, max_tokens, label):
        # A byte fallback is deliberately conservative, with extra protocol overhead.
        reservation = nano_cost(len(raw) + 8192, max_tokens, model)
        with self.lock():
            data = self.read()
            orphaned = False
            for call in data['calls']:
                if call['state'] == 'reserved' and (not call.get('owner_identity') or call['owner_identity'] != process_identity(call.get('owner_pid'))):
                    call.update(state='unknown', error_type='reservation_owner_lost')
                    orphaned = True
            if orphaned:
                atomic_json(self.path, data)
            if any(c['state'] == 'unknown' for c in data['calls']):
                raise BudgetError('An unknown outcome requires reconciliation before another paid call')
            committed = data['imported_estimated_nano_usd'] + sum(c.get('charged_upper_nano_usd', c['reserved_nano_usd']) for c in data['calls'])
            if committed + reservation > self.limit:
                raise BudgetError('Insufficient remaining authorized budget')
            owner_identity = process_identity(os.getpid())
            if owner_identity is None:
                raise BudgetError('Cannot verify the paid request owner process')
            ident = str(uuid.uuid4())
            call = {'id': ident, 'model': model, 'label': label, 'state': 'reserved', 'owner_pid': os.getpid(), 'owner_identity': owner_identity, 'started_utc': datetime.now(timezone.utc).isoformat(), 'request_bytes': len(raw), 'max_output_tokens': max_tokens, 'reserved_nano_usd': reservation, 'request_fingerprint': hmac.new(bytes.fromhex(data['fingerprint_epoch']), raw, hashlib.sha256).hexdigest()}
            data['calls'].append(call)
            atomic_json(self.path, data)
            return ident

    def finish(self, ident, **fields):
        with self.lock():
            data = self.read()
            call = next(c for c in data['calls'] if c['id'] == ident)
            call.update(fields)
            atomic_json(self.path, data)

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None

def connection(config_path, alias=None):
    doc = tomllib.loads(Path(config_path).read_text(encoding='utf-8-sig'))
    alias = alias or doc['llm']['default_model']
    model = doc['llm']['models'][alias]
    p = urlsplit(model['api_endpoint'])
    if p.scheme != 'https' or p.hostname != 'api.commandcode.ai' or p.path.rstrip('/') != '/provider/v1' or p.username or p.password or p.query or p.fragment:
        raise ValueError('Expected the explicitly approved CommandCode HTTPS endpoint')
    if model.get('api_type') != 'openai-chat' or not model.get('api_key'):
        raise ValueError('Missing configured Chat Completions connection')
    return model

def usage_from_body(raw, streaming):
    if not streaming:
        result = json.loads(raw)
        return result.get('usage'), result
    usage = None
    for line in raw.splitlines():
        if line.startswith(b'data:'):
            value = line[5:].strip()
            if value and value != b'[DONE]':
                item = json.loads(value)
                if isinstance(item.get('usage'), dict):
                    usage = item['usage']
    return usage, None

def paid_request(config, ledger, payload, label, timeout=75):
    model = payload.get('model')
    if model not in RATES:
        raise ValueError('Model is outside the approved evaluation model allowlist')
    max_tokens = payload.get('max_tokens', payload.get('max_completion_tokens'))
    if not isinstance(max_tokens, int) or not 1 <= max_tokens <= 2048:
        raise ValueError('A bounded output limit is required')
    raw = json.dumps(payload, ensure_ascii=False, separators=(',', ':')).encode('utf-8')
    if len(raw) > 240_000:
        raise ValueError('Model request exceeds byte limit')
    ident = ledger.reserve(model, raw, max_tokens, label)
    start = time.monotonic()
    request = urllib.request.Request(config['api_endpoint'].rstrip('/') + '/chat/completions', data=raw, headers={'Authorization': 'Bearer ' + config['api_key'], 'Content-Type': 'application/json', 'User-Agent': 'Metteur-Local-Acceptance/1.0'})
    try:
        chunks = []
        ttft = None
        with urllib.request.build_opener(NoRedirect).open(request, timeout=timeout) as response:
            size = 0
            while chunk := response.read1(8192):
                size += len(chunk)
                if size > 4_000_000:
                    raise ValueError('Model response exceeds byte limit')
                if ttft is None and (b'content' in chunk or b'tool_calls' in chunk or b'reasoning_content' in chunk):
                    ttft = int((time.monotonic() - start) * 1000)
                chunks.append(chunk)
        response_raw = b''.join(chunks)
        usage, result = usage_from_body(response_raw, payload.get('stream', False))
        safe = {k: usage[k] for k in ('prompt_tokens', 'completion_tokens', 'total_tokens', 'prompt_cache_hit_tokens', 'prompt_cache_miss_tokens', 'prompt_tokens_details', 'completion_tokens_details') if k in (usage or {})}
        known = isinstance((usage or {}).get('prompt_tokens'), int) and isinstance((usage or {}).get('completion_tokens'), int)
        fields = {'state': 'settled' if known else 'unknown', 'elapsed_ms': int((time.monotonic() - start) * 1000), 'ttft_ms': ttft if payload.get('stream') else None, 'usage': safe, 'provider_reported_cost_usd': None}
        if known:
            if usage['prompt_tokens'] < 0 or usage['completion_tokens'] < 0:
                raise ValueError('Negative provider usage')
            fields['charged_upper_nano_usd'] = nano_cost(usage['prompt_tokens'], usage['completion_tokens'], model)
        ledger.finish(ident, **fields)
        return response_raw, result, ident
    except urllib.error.HTTPError as exc:
        # A rejected client request is not a successful generation. Server-side
        # failures can have an unknown effect and retain the reservation.
        fields = {'state': 'rejected' if 400 <= exc.code < 500 else 'unknown', 'http_status': exc.code}
        if fields['state'] == 'rejected':
            fields['charged_upper_nano_usd'] = 0
        ledger.finish(ident, **fields)
        raise RuntimeError(f'Provider HTTP {exc.code}; response omitted') from None
    except Exception as exc:
        ledger.finish(ident, state='unknown', error_type=type(exc).__name__)
        raise RuntimeError(f'Model outcome unknown ({type(exc).__name__}); automatic replay blocked') from None
