set pagination off
set confirm off
set print elements 64
set print repeats 8
python
import gdb
import json

result = {"outcome": "debugger_error", "exit_code": None, "signal": None}

def diagnostic(command):
    try:
        text = gdb.execute(command, to_string=True)
        if len(text) > 12000:
            text = text[:6000] + "\n[diagnostic truncated]\n" + text[-6000:]
        gdb.write(text)
    except gdb.error as error:
        gdb.write(json.dumps({"diagnostic": command, "error": str(error)}) + "\n")

def stopped(event):
    result["outcome"] = "stopped"
    if isinstance(event, gdb.SignalEvent):
        result["outcome"] = "signal"
        result["signal"] = event.stop_signal
    diagnostic("bt 20")
    diagnostic("info registers")
    diagnostic("x/8i $pc")

def exited(event):
    result["outcome"] = "exited"
    result["exit_code"] = getattr(event, "exit_code", None)

gdb.events.stop.connect(stopped)
gdb.events.exited.connect(exited)
try:
    gdb.execute("run")
except gdb.error as error:
    result["error"] = str(error)
    result["outcome"] = "debugger_error"
gdb.write(json.dumps(result) + "\n")
failed = result["outcome"] in ("signal", "debugger_error") or (
    result["outcome"] == "exited" and result["exit_code"] != 0
)
gdb.execute("quit " + ("1" if failed else "0"))
end
