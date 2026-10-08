# C, C++, Rust, and Swift

## C and C++

Prefer GDB for GNU/Linux GCC builds, LLDB for Apple/Clang environments, and Visual Studio's native debugger for Windows MSVC/PDB builds. Follow a working project setup when it already supplies compatible symbols and visualizers. For a reproduction, use the build system's debug configuration; a small GCC/Clang example is `cc -g -Og repro.c -o repro`. Keep release symbols when optimization itself triggers the issue.

For automated GDB evidence, load gdb-debugging and its capture script. In a terminal, `gdb --args ./repro input` or `lldb -- ./repro input` starts an interactive session. Set a relevant breakpoint, run, inspect stack/locals/registers, then exit. Missing symbols and optimized-out variables are not proof of corruption. Use native-debugging for memory interpretation and pwntools-debugging for exact binary input. ASan/UBSan are useful complementary detectors; their diagnostics do not establish exploitability by themselves.

Sources: [GDB](https://sourceware.org/gdb/current/onlinedocs/gdb.html/Mode-Options.html), [LLDB tutorial](https://lldb.llvm.org/use/tutorial.html), [Visual Studio C++ debugging](https://learn.microsoft.com/en-us/visualstudio/debugger/getting-started-with-the-debugger-cpp).

## Rust

Prefer `rust-gdb` on a compatible GNU toolchain or `rust-lldb` with LLDB; rustup provides these wrappers, while the underlying debugger must be installed. Build with `cargo build` and select the actual package binary under target/debug. In a terminal, `rust-gdb --args target/debug/app` or `rust-lldb -- target/debug/app` preserves the language-specific setup. For MSVC targets, use a debugger that understands the generated PDB files rather than assuming GNU debug formats.

Start a panic investigation with a backtrace from the reproducing test; use the debugger for locals, native faults, or FFI state. Async tasks are not necessarily one-to-one with OS threads. Pretty-printer failures and unavailable variables can come from toolchain or optimization mismatches.

Source: [rustup debugger proxies](https://rust-lang.github.io/rustup/concepts/proxies.html).

## Swift

Prefer Xcode's debugger for Apple application targets and the Swift toolchain's LLDB for command-line or server binaries. For a Swift package, build a debug product and open its actual executable in LLDB; the official Swift VS Code integration also supplies launch configurations. Use a debugger built for the selected Swift toolchain: a generic distribution LLDB may lack Swift expression support. Match dSYM/debug symbols to the executable and loaded frameworks before interpreting missing frames.

Sources: [Swift REPL and debugger](https://www.swift.org/documentation/lldb/), [Swift editor debugging](https://docs.swift.org/latest/documentation/userdocs/debugging/).
