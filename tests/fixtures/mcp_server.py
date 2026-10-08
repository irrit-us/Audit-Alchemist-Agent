"""Local stdio fixture only; no third-party dependencies or external service."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

sys.stdin.reconfigure(encoding="utf-8")
sys.stdout.reconfigure(encoding="utf-8")
mode, log = sys.argv[1:3]
log = Path(log)


def record(value):
    with log.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(value) + "\n")


def send(value):
    print(json.dumps(value), flush=True)


record({"started": True, "cwd": os.getcwd()})
if mode in ("descendant", "cancel_init"):
    subprocess.Popen([sys.executable, "-c", "import time,pathlib,sys;time.sleep(2);pathlib.Path(sys.argv[1]).write_text('leaked')", str(log) + ".leaked"],
                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

for line in sys.stdin:
    request = json.loads(line)
    record(request)
    method = request.get("method")
    if method == "notifications/initialized":
        continue
    if "method" not in request:  # reply to server-initiated ping/unsupported request
        continue
    if method == "initialize":
        if mode in ("hang_init", "cancel_init"):
            time.sleep(30)
        result = {"protocolVersion": "bad" if mode == "bad_version" else "2025-11-25",
                  "capabilities": {"tools": {}}, "serverInfo": {"name": "fixture", "version": "1"},
                  "instructions": "SERVER_INSTRUCTIONS_MUST_NOT_ENTER_PROMPT"}
    elif method == "tools/list":
        tool = {"name": "echo", "description": "Echo a test value", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}
        result = {"tools": [tool, tool] if mode == "duplicate" else [tool]}
        if mode == "pages" and not request.get("params", {}).get("cursor"):
            result = {"tools": [], "nextCursor": "second"}
        if mode == "cursor_loop":
            result = {"tools": [], "nextCursor": "same"}
    elif method == "tools/call":
        if mode in ("hang_call", "descendant"):
            time.sleep(30)
        if mode == "oversized":
            print("x" * 1200000, flush=True)
            continue
        if mode == "malformed":
            print("not json", flush=True)
            continue
        sys.stderr.write("diagnostic-only\n" * 10000)
        sys.stderr.flush()
        send({"jsonrpc": "2.0", "method": "notifications/message", "params": {"data": "note"}})
        send({"jsonrpc": "2.0", "id": "ping", "method": "ping"})
        send({"jsonrpc": "2.0", "id": "sampling", "method": "sampling/createMessage"})
        result = {"content": [{"type": "text", "text": request["params"]["arguments"].get("text", "echo")}],
                  "structuredContent": {"mapped_env": os.environ.get("FIXTURE_MAPPED")},
                  "isError": mode == "tool_error"}
    else:
        raise AssertionError(method)
    if mode == "bad_result" and method == "tools/call":
        result = {"content": "not an array"}
    send({"jsonrpc": "2.0", "id": -1 if mode == "wrong_id" else request["id"], "result": result})
record({"closed": True})
