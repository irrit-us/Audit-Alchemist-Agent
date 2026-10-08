---
name: foundry-debugging
description: Debug Solidity with Forge traces and version-compatible VM/console helpers.
---

# Foundry debugging

Inspect forge --version, foundry.toml, remappings, the selected solc version, and the installed forge-std interfaces. Forge, solc, and forge-std have independent version numbers: matching means the project compiles and the runtime implements the requested selectors, not equal version strings.

Use the existing Test.vm interface and console helpers when compatible. Load scripts/TypedDebug.t.sol as a small capability test for deal/load and console output. It requires the project's forge-std remapping and a compatible compiler. Keep experiments in tests; use forge test --match-contract NAME --match-test NAME -vvvv for focused traces. Use --debug for an interactive single test only when a terminal is available.

For an incompatible Solidity pragma, broken forge-std imports, or interface drift, load scripts/RawDebug.sol. It has no forge-std dependency and supports Solidity 0.6–0.8. Call RawDebug.requireVm() to verify the local cheatcode runtime, RawDebug.callVm(abi.encodeWithSignature(...)) for supported state-changing cheatcodes, and RawDebug.logString/logUint/logBytes for direct diagnostic output. Read references/compatibility.md for address semantics and limitations. A direct call cannot implement a selector missing from the runtime.

To save a bundled file, call load_skill with this name, the exact resource, and save_to such as test/RawDebug.sol (parent must exist; existing files are refused). The response gives its path without copying the source into context. Import it into a separate reproduction test, then execute through Bash with an appropriate timeout. scripts/forge_trace.py runs one selected test and records Forge version without guessing a compiler or rewriting configuration.

Compare traces and actual emitted values with the hypothesis. Avoid claiming successful logging from the low-level call's success bit alone. Never interpret local cheatcode behavior as behavior available on a deployed chain.
