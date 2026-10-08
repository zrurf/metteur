"""Local framed LSP fixture; no network or external provider access."""
import json
import os
import subprocess
import sys
import time

label, mode, pid_file = sys.argv[1:]
child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(120)"])
with open(pid_file, "w") as file:
    file.write(f"{os.getpid()} {child.pid}")

def send(value):
    payload = json.dumps(value).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(payload)}\r\n\r\n".encode() + payload)
    sys.stdout.buffer.flush()

while True:
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line == b"\r\n":
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1])
    request = json.loads(sys.stdin.buffer.read(length))
    method = request.get("method")
    params = request.get("params", {})
    if method == "initialize":
        if mode == "hang_init":
            time.sleep(120)
        if mode == "oversize":
            send({"jsonrpc": "2.0", "id": request["id"], "result": "x" * (1024 * 1024)})
            continue
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"capabilities": {"textDocumentSync": 1}}})
    elif method in ("textDocument/didOpen", "textDocument/didChange"):
        document = params["textDocument"]
        if method == "textDocument/didOpen" and document.get("languageId") not in ("fixture", "user"):
            sys.exit(3)
        text = document.get("text", params.get("contentChanges", [{}])[0].get("text", ""))
        if mode == "crash":
            sys.exit(0)
        if mode == "silent":
            continue
        diagnostics = []
        if "BAD" in text:
            diagnostics = [{"severity": 1, "message": label, "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}}}]
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": document["uri"], "version": document["version"], "diagnostics": diagnostics}})
        # Unopened documents and stale versions must not contaminate the cache.
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": "file:///unrelated", "diagnostics": diagnostics}})
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": document["uri"], "version": document["version"] - 1, "diagnostics": [{"severity": 1, "message": "stale"}]}})
    elif method == "textDocument/hover":
        if mode == "hang_hover":
            time.sleep(120)
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"contents": f"{label}:{os.environ.get('R09_TOKEN', 'absent')}:{'HOME' in os.environ}"}})
    elif method == "textDocument/definition":
        send({"jsonrpc": "2.0", "id": request["id"], "result": [{"uri": params["textDocument"]["uri"], "range": {"start": {"line": 2, "character": 3}, "end": {"line": 2, "character": 4}}}]})
