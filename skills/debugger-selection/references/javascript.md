# JavaScript and TypeScript

## JavaScript

For Node processes, prefer Node Inspector. Load node-inspector for the bundled external controller, breakpoint capture, exception handling, and expression evaluation. For an existing IDE session, use that IDE's Node debug configuration and the same launch arguments/environment as the reproduction. A bare `node --inspect-brk=127.0.0.1:9229 app.js` waits for a client; it is not a complete unattended diagnostic command.

For browser code, use the browser's DevTools Sources debugger, especially exception, event-handler, and network-related breakpoints. Reproduce the actual page state and select the correct frame/worker. DOM and browser APIs are not present in ordinary Node, so moving a browser failure into Node may change the bug. Use the browser profiler for a performance question after identifying a reproducible interaction.

Sources: [Node Inspector](https://nodejs.org/api/inspector.html), [Chrome DevTools debugging](https://developer.chrome.com/docs/devtools/javascript).

## TypeScript

Choose the debugger for the runtime executing the generated JavaScript. Use VS Code's JavaScript debugger or browser DevTools with source maps enabled in the project build. Verify the map and generated file belong to the same build, and check output paths if a breakpoint remains unbound. For bundled apps, inspect whether dependencies are inlined or mapped to a different source path.

The bundled node-inspector script accepts runtime file locations and does not resolve source maps itself. Supply a generated JavaScript location to that helper, or use a source-map-aware IDE for TypeScript locations. A test runner, loader, or child worker can execute a different entrypoint from the visible .ts file; inspect the launch configuration before assuming a breakpoint was skipped.

Source: [VS Code TypeScript debugging](https://code.visualstudio.com/docs/typescript/typescript-debugging).
