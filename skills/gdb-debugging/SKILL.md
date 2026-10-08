---
name: gdb-debugging
description: Capture native crashes, breakpoints, registers, and exit status with a GDB script.
---

# GDB debugging

Check gdb --version and whether it supports Python (gdb -q -nx -batch -ex 'python print(1)'). Build the reproduction with debug symbols when possible. Preserve the original optimization level if changing it hides the bug.

Load scripts/capture.gdb with save_to such as .audit-debug/capture.gdb after creating that directory. Run through Bash:

    gdb -q -nx -batch -x .audit-debug/capture.gdb --args ./poc input.bin

For a breakpoint snapshot, insert -ex 'break vulnerable_function' before -x. The script runs until the first stop, emits bounded stack/register diagnostics, and records inferior exit or stop status as JSON. A crash or debugger failure exits nonzero; an intentional breakpoint exits zero with a stopped outcome. This is a single-stop capture, not an interactive controller. Use explicit GDB commands or tmux-debugging when stepping is needed.

The default -nx excludes initialization files. Omit it only when deliberately using installed trusted extensions such as pwndbg; native-debugging has a focused pwndbg reference. Determine architecture and actual mappings before interpreting registers or PIE addresses. gdb's command success is distinct from the inferior's outcome; inspect the structured result and backtrace.

This script requires GDB Python support. Without it, use bounded plain commands (run, bt, info registers) and inspect their errors instead of treating the debugger exit code as the program exit code. Keep the whole debugger run inside the Bash timeout; do not leave a process waiting for stdin.

Reference: [GDB modes](https://sourceware.org/gdb/current/onlinedocs/gdb.html/Mode-Options.html).
