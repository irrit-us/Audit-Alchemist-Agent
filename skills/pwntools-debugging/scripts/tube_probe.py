#!/usr/bin/env python3
"""Send one byte payload to a local process; no interactive stdin or remote target."""
import argparse
import json
import math
from pathlib import Path
import sys
import time


def receive(tube, deadline, limit, delimiter=None):
    data = bytearray()
    while len(data) < limit:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return bytes(data), "timeout"
        try:
            chunk = tube.recv(min(4096, limit - len(data)), timeout=remaining)
        except EOFError:
            return bytes(data), "eof"
        if not chunk:
            return bytes(data), "timeout"
        data.extend(chunk)
        if delimiter and delimiter in data:
            return bytes(data), "matched"
    return bytes(data), "output_limit"


def probe(tube, payload, prompt, newline, timeout, limit):
    deadline = time.monotonic() + timeout
    prefix = b""
    if prompt:
        prefix, state = receive(tube, deadline, limit, prompt)
        if state != "matched":
            return {"state": "prompt_" + state, "sent": False, "output_hex": prefix.hex()}
    if len(prefix) >= limit:
        return {"state": "output_limit", "sent": False, "output_hex": prefix.hex()}
    tube.send(payload + (b"\n" if newline else b""))
    tube.shutdown("send")
    output, state = receive(tube, deadline, limit - len(prefix))
    return {"state": state, "sent": True, "output_hex": (prefix + output).hex()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--payload-hex")
    group.add_argument("--payload-file", type=Path)
    parser.add_argument("--expect-hex")
    parser.add_argument("--newline", action="store_true")
    parser.add_argument("--timeout", type=float, default=3)
    parser.add_argument("--max-bytes", type=int, default=8192)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command or not math.isfinite(args.timeout) or not 0 < args.timeout <= 120:
        parser.error("provide a command after -- and a timeout in (0, 120]")
    if not 1 <= args.max_bytes <= 32768:
        parser.error("--max-bytes must be 1..32768")
    try:
        if args.payload_file:
            with args.payload_file.open("rb") as stream:
                payload = stream.read(1048577)
        else:
            payload = bytes.fromhex(args.payload_hex)
        if len(payload) > 1048576:
            parser.error("payload exceeds 1 MiB")
        prompt = bytes.fromhex(args.expect_hex) if args.expect_hex else None
        if prompt and len(prompt) > args.max_bytes:
            parser.error("prompt exceeds receive limit")
    except (ValueError, OSError) as error:
        parser.error(str(error))
    try:
        from pwnlib.context import context
        from pwnlib.tubes.process import process, PIPE, STDOUT
    except ImportError:
        parser.error("pwntools is unavailable in this interpreter; use an environment with pwnlib installed")
    context.log_level = "error"
    try:
        with process(command, stdin=PIPE, stdout=PIPE, stderr=STDOUT, timeout=args.timeout) as tube:
            started = time.monotonic()
            result = probe(tube, payload, prompt, args.newline, args.timeout, args.max_bytes)
            if result["state"] == "eof":
                tube.wait_for_close(timeout=max(0, args.timeout - (time.monotonic() - started)))
            result["exit_code"] = tube.poll(block=False)
            print(json.dumps(result))
            return 0 if result["state"] == "eof" and result["exit_code"] == 0 else 1
    except (EOFError, OSError) as error:
        print(json.dumps({"state": "io_error", "error": str(error)}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
