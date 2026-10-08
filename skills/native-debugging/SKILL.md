---
name: native-debugging
description: Analyze native memory corruption and address calculations with GDB/pwndbg.
---

# Native debugging

Check command -v gdb and the binary's architecture, symbols, and build flags. Use gdb-debugging for the bundled GDB capture script; pwndbg is optional. Prefer one bounded batch invocation through Bash for reproducible diagnostics, for example gdb -q -batch -ex run -ex bt -ex 'info registers' --args ./poc. Preserve stderr and the exit status. Load references/pwndbg.md only when pwndbg is installed and its helpers would simplify inspection.

At a crash, establish the faulting instruction, backtrace, relevant registers, mapped address ranges, and input bytes controlling the operation. Distinguish the first invalid operation from a later allocator failure. For PIE and shared libraries, derive addresses from the actual mapping for that run. Do not hard-code an address from a different process.

For heap issues, determine the allocator and version before choosing allocator-specific commands. Inspect the smallest relevant allocation and adjacent metadata; avoid giant memory dumps. A debugger run may change timing and layout, so reproduce the essential behavior outside it when possible.

Use tmux-debugging only when interactive steps are necessary. Keep the debugger and target within the tool deadline. Report observations separately from inferred exploitability.

Adapted from 0RAYS/codex-auditor pwndbg-skill; see ../LICENSE.codex-auditor and ../UPSTREAM.md in the source distribution.
