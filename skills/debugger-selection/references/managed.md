# Java, Kotlin, C#, and Dart

## Java

Prefer the project's IntelliJ IDEA or Eclipse debugger for ordinary JVM inspection; `jdb` is the JDK's terminal fallback. A small terminal reproduction can use `javac -g Main.java`, then `jdb -classpath . Main`. This is interactive: set a breakpoint such as `stop in Main.main`, run, inspect, and exit. Preserve the real classpath/module path and JDK version for application tests.

For a JDWP attach workflow, use the existing IDE configuration, bind the target locally, and connect the client while the target is alive. A suspended JVM waiting for a client is not a hung application. Class reloading and optimized frames can make source state diverge from the loaded class.

Source: [JDK jdb](https://docs.oracle.com/en/java/javase/25/docs/specs/man/jdb.html).

## Kotlin

Prefer IntelliJ IDEA for JVM code and Android Studio for Android apps. Select a Kotlin-capable debugger view for coroutines; a thread stack alone may not represent the suspended coroutine's logical stack. Use the project's Gradle test/app launch configuration to preserve the classpath and generated classes. The JDK jdb fallback can inspect JVM bytecode execution, but lacks the IDE's Kotlin/coroutine presentation.

Kotlin/Native and Kotlin/JS require the target runtime's debugger: LLDB/native symbols or JavaScript tooling/source maps respectively. Do not route them to JDWP because the repository contains .kt files.

Source: [Kotlin coroutine/Flow debugging](https://kotlinlang.org/docs/debug-flow-with-idea.html).

## C#

Prefer Visual Studio on Windows or VS Code's C# debugger for a supported .NET project. Use its existing launch/test configuration and matching assemblies/PDBs; `dotnet build -c Debug` prepares a conventional debug build. Confirm the selected runtime, process, and source checksum if breakpoints do not bind.

For a managed hang or post-mortem investigation, an installed `dotnet-dump` can collect and analyze a dump with SOS commands. It is not a live stepping debugger and does not replace a native debugger for mixed native stacks. Match runtime and architecture when analyzing dumps; stop after collecting the specific evidence needed.

Sources: [VS Code C# debugging](https://code.visualstudio.com/docs/csharp/debugging), [dotnet-dump](https://learn.microsoft.com/en-us/dotnet/core/diagnostics/dotnet-dump).

## Dart

Prefer Dart/Flutter DevTools or the IDE debugger connected to the Dart VM service. For a Dart command-line reproduction, `dart run --observe bin/main.dart` exposes a debugging service and changes pause behavior; attach the client to the printed local URI. For Flutter, use the project's debug launch configuration and DevTools debugger.

Select the correct isolate and await context. Flutter release builds have different debugging availability, and Dart compiled to JavaScript requires browser tooling and maps. Keep a paused VM and its client within the same supervised session.

Sources: [dart run](https://dart.dev/tools/dart-run), [Dart/Flutter DevTools debugger](https://docs.flutter.dev/tools/devtools/debugger).
