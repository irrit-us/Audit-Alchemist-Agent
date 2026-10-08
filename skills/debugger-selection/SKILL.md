---
name: debugger-selection
description: Choose a debugger when the language, runtime, or platform needs guidance.
---

# Debugger selection

Use the project's working debugger configuration when it fits the problem. Otherwise choose a default below after checking the actual runtime, compiler, OS, and installed tool version. These preferences favor maintained language-aware tools, standard tooling, and reproducible local evidence. File extensions alone do not identify the runtime: TypeScript may run in a browser, Kotlin may target the JVM or native code, and Python may fail inside a native extension.

| Language | Preferred starting point | Consult next |
| --- | --- | --- |
| Python | pdb for local inspection; debugpy for an existing IDE/DAP workflow | references/dynamic.md |
| JavaScript | Node Inspector for Node; browser DevTools for browser code | references/javascript.md; node-inspector for the bundled controller |
| TypeScript | The JavaScript runtime's debugger with matching source maps | references/javascript.md |
| C | GDB on GNU/Linux; LLDB on macOS; Visual Studio for MSVC | references/native.md; gdb-debugging for capture |
| C++ | GDB/LLDB by toolchain; Visual Studio for MSVC/PDB builds | references/native.md |
| Rust | rust-gdb or rust-lldb with symbols; a PDB-capable debugger for MSVC | references/native.md |
| Go | Delve (dlv), especially for goroutines and Go values | references/go-shell.md |
| Java | Existing IntelliJ IDEA/Eclipse debugger; JDK jdb for terminal use | references/managed.md |
| Kotlin | IntelliJ IDEA or Android Studio for JVM/Android; LLDB for native | references/managed.md |
| C# | Visual Studio or VS Code C# debugger; dotnet-dump for managed dumps | references/managed.md |
| Swift | Xcode debugger or the matching Swift toolchain's LLDB | references/native.md |
| Ruby | The maintained debug gem and rdbg | references/dynamic.md |
| PHP | Xdebug with a compatible IDE/DBGp client | references/dynamic.md |
| Dart | Dart/Flutter DevTools or the IDE debugger connected to the VM service | references/managed.md |
| R | Built-in debugonce/browser/traceback, optionally through RStudio | references/dynamic.md |
| Bash | bash -n, then scoped bash -x tracing; bashdb when installed and compatible | references/go-shell.md |
| Solidity | Forge test traces and Foundry debugging helpers | Load foundry-debugging |

Load only the relevant reference with load_skill using this skill name and the exact resource path. The linked execution skills are separately available by name. This guide provides recommendations; it does not bundle the listed third-party debuggers or create adapters for them.

For automated work, prefer a bounded crash capture, a command script that exits, or a controller that starts, inspects, and closes its target in one Bash call. Interactive examples in the references require an IDE, a terminal, or the existing tmux-debugging workflow. Starting a paused server without connecting a client produces no diagnostic evidence. Bash has no interactive stdin, and background descendants may be cleaned up at the end of the call.

Preserve build IDs, symbols, source maps, runtime version, and the reproducing input. Missing locals or unbound breakpoints often indicate a build/mapping mismatch; check that before changing tools. Reduce optimization only in a reproduction build and recheck behavior under original settings when it matters. Use a profiler for performance questions and a sanitizer for memory/race detection when appropriate; a breakpoint debugger answers a different question.

Keep local debugger endpoints on loopback, and use watch expressions deliberately because evaluation can execute program code. Record the target outcome separately from the debugger's exit status. If a preferred tool is unavailable, choose an installed compatible fallback or report that dependency rather than claiming it ran.
