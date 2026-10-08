"""Isolated stdio MCP fixture; no external network or inherited credentials."""
import json
import os
import pathlib
import subprocess
import sys
import time

label, mode, pid_file = sys.argv[1:]
child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(120)"], env={})
pathlib.Path(pid_file).write_text(f"{os.getpid()} {child.pid}")
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if "id" not in request:
        continue
    if mode == "hang":
        time.sleep(120)
    if method == "initialize":
        result = {"protocolVersion": request["params"]["protocolVersion"], "capabilities": {"tools": {}, "resources": {}}, "serverInfo": {"name": "r08-test", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "read_value", "description": "Isolated test output", "inputSchema": {"type": "object", "properties": {}}}]}
    elif method == "tools/call":
        if mode == "disconnect":
            sys.exit(0)
        result = {"content": [{"type": "text", "text": label + ":" + os.environ.get("R08_SELECTED", "absent") + ":" + str("HOME" in os.environ)}]}
    elif method == "resources/list":
        result = {"resources": []}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unsupported"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
