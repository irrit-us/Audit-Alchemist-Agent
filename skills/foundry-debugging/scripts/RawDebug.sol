// SPDX-License-Identifier: MIT
pragma solidity >=0.6.0 <0.9.0;

// Import-free diagnostics for local Forge tests. Never a production dependency.
library RawDebug {
    address internal constant VM = 0x7109709ECfa91a80626fF3989D68f67F5b1DD12D;
    address internal constant CONSOLE = 0x000000000000000000636F6e736F6c652e6c6f67;

    function requireVm() internal view {
        (bool ok, bytes memory data) = VM.staticcall(
            abi.encodeWithSignature("load(address,bytes32)", address(this), bytes32(0))
        );
        require(ok && data.length == 32, "RawDebug: no compatible cheatcode runtime");
    }

    function callVm(bytes memory payload) internal returns (bytes memory) {
        requireVm();
        (bool ok, bytes memory data) = VM.call(payload);
        if (!ok) {
            // Preserve the runtime's unknown-selector or other error.
            assembly { revert(add(data, 32), mload(data)) }
        }
        return data;
    }

    function logString(string memory value) internal view {
        sendConsole(abi.encodeWithSignature("log(string)", value));
    }

    function logUint(uint256 value) internal view {
        sendConsole(abi.encodeWithSignature("log(uint256)", value));
    }

    function logBytes(bytes memory value) internal view {
        sendConsole(abi.encodeWithSignature("log(bytes)", value));
    }

    function sendConsole(bytes memory payload) private view {
        (bool ok, ) = CONSOLE.staticcall(payload);
        require(ok, "RawDebug: console call reverted");
        // A successful call is not proof of output. Verify the Forge logs.
    }
}
