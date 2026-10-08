---
name: node-inspector
description: Debug local Node.js breakpoints and exceptions with an Inspector controller.
---

# Node Inspector

Use scripts/inspect.mjs for an external controller so pausing the target does not pause the debugger itself. It needs a Node build with global WebSocket support (Node 22.4+). Check node --version. Save the script with load_skill save_to, then run it through Bash:

    node .audit-debug/inspect.mjs --break src/app.js:42 --expression 'request.path' --timeout-ms 10000 -- src/app.js argument

The target starts paused with an ephemeral Inspector port bound to 127.0.0.1. The controller enables debugging, installs an optional 1-based source breakpoint, captures uncaught exceptions and debugger statements, evaluates the optional expression in the top paused frame, then resumes. Entry pause is resumed without evaluating the expression. A supplied expression can have side effects: use an expression relevant to the investigation. Compiled/transpiled files require their runtime locations; this helper does not resolve source maps.

Target output remains on stdout. Diagnostic JSON records and target stderr go to stderr. Inspect pause reason, source locations, evaluation exceptionDetails, and the final child exit status. The script closes its WebSocket when Node waits for debugger disconnection and kills its own target on a deadline. Run the complete lifecycle in one Bash call for descendant cleanup.

It launches a fresh target and does not attach to an existing server. For an authorized existing target use a suitable CDP client and its verified endpoint. Do not expose the Inspector port beyond loopback. Reference: [Node Inspector API](https://nodejs.org/api/inspector.html), including the warning about same-thread breakpoints.
