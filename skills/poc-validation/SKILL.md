---
name: poc-validation
description: Build controlled local PoCs and diagnose failed or timed-out reproductions.
---

# PoC validation

Identify the exact claim to test and a control case. Inspect build instructions and installed dependencies first. Prefer the project's existing test runner or a minimal harness that exercises the real vulnerable path. Keep fixtures in a clearly named scratch directory; read existing files before changing them and preserve unrelated work.

Write the smallest input that crosses the suspected boundary. Run it with Bash and a deliberate timeout. Record the command, relevant build flags, exit status, and decisive output. Use both positive and negative controls where feasible. If output truncates, rerun a narrower command or redirect verbose diagnostics to a file and read the relevant section.

A timeout, compilation error, or missing dependency does not confirm the vulnerability. Read the error, change one cause at a time, and stop unchanged retries. A sanitizer finding establishes its reported memory violation, not automatically an exploit primitive or end-to-end impact. Do not describe a PoC as executed unless a tool result records the execution.

Use debugger-selection when choosing a runtime-appropriate debugger. Use foundry-debugging for Solidity tests, node-inspector for JavaScript breakpoints, pwntools-debugging for binary I/O, native-debugging for crash state, or tmux-debugging when an installed terminal debugger needs controlled interactive input. If execution cannot settle the claim, state the limitation and rely only on the code evidence actually established.
