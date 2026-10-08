---
name: pwntools-debugging
description: Probe local binary process I/O with pwntools, exact bytes, and prompt checks.
---

# pwntools debugging

Check the chosen Python interpreter can import pwnlib; pwntools is most useful on Linux. Identify the target architecture before using ELF, packing helpers, cyclic offsets, or a corefile. Do not change context architecture based on an unrelated binary.

Save scripts/tube_probe.py with load_skill save_to. It launches only the specified local program, takes raw payload bytes from a hex string or file, optionally waits for exact prompt bytes, then sends and captures bounded output:

    python3 .audit-debug/tube_probe.py --expect-hex 3e20 --payload-hex 41414141 --newline --timeout 3 -- ./poc

Use --payload-file for binary fixtures. The helper closes stdin after sending and records output as hex without lossy decoding. Its receive deadline and byte cap distinguish EOF, timeout, and truncated output. A missing prompt stops before sending. This avoids sendlineafter's behavior of sending even when its receive timed out. The helper closes its process in all cases; run through Bash for process-tree cleanup.

For multi-step protocols, adapt the saved script with explicit per-step expectations and finite deadlines. Avoid interactive() in the harness, whose stdin is closed. For a crash, use gdb-debugging or inspect a verified local corefile; do not mistake a tube timeout, EOF, or killed process for successful exploitation.

References: [tube receive semantics](https://docs.pwntools.com/en/stable/tubes.html), [local process tubes](https://docs.pwntools.com/en/stable/tubes/processes.html).
