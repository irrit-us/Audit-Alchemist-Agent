# Compiler and runtime compatibility

There are three independent compatibility boundaries: Solidity syntax/import pragmas, the forge-std ABI interfaces compiled into a test, and selectors implemented by the installed Forge runtime. First reproduce the failure and identify which boundary it crosses. Pinning a compatible library/compiler is useful when allowed; diagnostic helpers should not silently upgrade project dependencies.

Cheatcodes live at 0x7109709ECfa91a80626fF3989D68f67F5b1DD12D. Both a typed Vm call and a low-level ABI call reach this same address. Direct calls remove the need to import a particular Vm.sol, but an unknown selector still fails. RawDebug bubbles revert data. Its requireVm probe also checks a known return shape, because calling an empty address outside Forge can return success with empty bytes.

Console output uses the separate address 0x000000000000000000636F6e736F6c652e6c6f67. It accepts static calls encoded with signatures such as log(string), log(uint256), and log(bytes). RawDebug calls this address without importing a compiler-incompatible console library. It uses a view helper instead of a compiler-dependent assembly cast to pure. View helpers cannot be called from pure Solidity functions; adjust only the reproduction harness as appropriate.

Run Forge with at least -vv to see logs, or -vvvv for call traces. Console calls outside a compatible local runtime may silently do nothing. Read observed output before asserting that a value was emitted. For ancient compilers outside 0.6–0.8, use a helper written and tested for that compiler rather than changing the pragma blindly.

Authoritative references: [cheatcodes](https://getfoundry.sh/forge/cheatcodes), [console implementation](https://github.com/foundry-rs/forge-std/blob/master/src/console.sol), and the installed forge test --help.
