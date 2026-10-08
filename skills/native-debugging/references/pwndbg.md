# Focused pwndbg inspection

Check help for installed command syntax; versions differ. Begin with context regs disasm stack, then narrow to regs rax rip, stack, or telescope ADDRESS COUNT. Use vmmap to establish mappings and xinfo ADDRESS to interpret a suspicious pointer. Search only a relevant mapping for the bytes of interest.

For relocatable targets, pwndbg's $rebase(offset) resolves an executable-relative address and $base("libc") locates a matching module base. Confirm the mapping before using either. breakrva can set an executable-relative breakpoint when available; use standard GDB break with a verified address otherwise.

Use contextwatch only for expressions needed on repeated stops. Prefer a few targeted breakpoint captures over stepping the entire program. Use allocator-specific helpers only after identifying the allocator; generic heap command output is not evidence of a particular exploit technique.

Fallback without pwndbg: bt, info registers, info proc mappings (where supported), x/16gx ADDRESS, and disassemble. Never assume an optional helper succeeded without inspecting its output.
