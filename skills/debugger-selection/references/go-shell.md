# Go and Bash

## Go

Prefer Delve for normal Go programs: it understands Go runtime structures and goroutines better than GDB. Check `go version` and `dlv version` for toolchain compatibility. In a terminal, `dlv debug ./cmd/app -- argument` launches a program; `dlv test ./pkg -- -test.run '^TestName$'` selects a test. Inspect the goroutine of interest, its stack, and relevant variables before following all threads.

For automation, use an existing DAP/Delve client or a bounded command script; a headless Delve server alone waits for a client. For a prebuilt executable, retain symbols and use dlv exec with the installed version's syntax. Delve test/debug builds may disable optimization; confirm behavior under original build settings if timing or optimization matters. GDB remains useful for specific cgo/native-runtime problems, but should not be the default for Go concurrency inspection.

Sources: [Go debugger guidance](https://go.dev/doc/gdb), [Delve commands](https://github.com/go-delve/delve/blob/master/Documentation/usage/README.md), [test selection](https://github.com/go-delve/delve/blob/master/Documentation/usage/dlv_test.md).

## Bash

Start with `bash -n script.sh` for syntax, then `bash -x script.sh argument` for a bounded trace of a reproducing execution. Xtrace is execution tracing, not a breakpoint debugger. Prefer it for expansion, branch, and command-order problems. Use an installed compatible bashdb only when stepping would provide additional evidence; inspect its help instead of assuming it matches the shell version.

Set PS4 to include source and line information when needed, and use BASH_XTRACEFD to separate traces from program stderr when supported. Traces contain expanded arguments and can reveal credentials; trace the smallest relevant section and inspect before sharing. Check pipeline status deliberately: introducing pipefail changes failure behavior, so do not silently change shell options while claiming an unchanged reproduction. A POSIX sh or zsh script requires its own interpreter rather than forcing Bash semantics.

Sources: [Bash set/xtrace](https://www.gnu.org/software/bash/manual/html_node/The-Set-Builtin.html), [Bash variables](https://www.gnu.org/software/bash/manual/html_node/Bash-Variables.html), [bashdb project](https://bashdb.sourceforge.net/).
