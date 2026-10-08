// SPDX-License-Identifier: MIT
pragma solidity >=0.8.13 <0.9.0;

import {Test} from "forge-std/Test.sol";
import {console} from "forge-std/console.sol";

contract TypedDebugCapabilityTest is Test {
    function testTypedDebugCapability() public {
        address account = address(0xBEEF);
        vm.deal(account, 123);
        assertEq(account.balance, 123);
        assertEq(vm.load(address(this), bytes32(uint256(123))), bytes32(0));
        console.log("typed-debug-compatible");
        console.log(account.balance);
    }
}
