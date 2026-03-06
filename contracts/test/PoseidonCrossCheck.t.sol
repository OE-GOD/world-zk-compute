// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/remainder/PoseidonSponge.sol";

/// @notice Cross-check Poseidon outputs against the Rust Stylus implementation.
/// Run with: forge test --match-contract PoseidonCrossCheckTest -vv
contract PoseidonCrossCheckTest is Test {
    /// Test 1: squeeze with no absorb (just padding + permute from init state)
    function test_squeeze_no_absorb() public {
        PoseidonSponge.Sponge memory s = PoseidonSponge.init();
        uint256 result = PoseidonSponge.squeeze(s);
        emit log_named_uint("squeeze_no_absorb", result);
    }

    /// Test 2: absorb(1), squeeze
    function test_absorb_1_squeeze() public {
        PoseidonSponge.Sponge memory s = PoseidonSponge.init();
        PoseidonSponge.absorb(s, 1);
        uint256 result = PoseidonSponge.squeeze(s);
        emit log_named_uint("absorb_1_squeeze", result);
    }

    /// Test 3: absorb(42), squeeze
    function test_absorb_42_squeeze() public {
        PoseidonSponge.Sponge memory s = PoseidonSponge.init();
        PoseidonSponge.absorb(s, 42);
        uint256 result = PoseidonSponge.squeeze(s);
        emit log_named_uint("absorb_42_squeeze", result);
    }

    /// Test 4: absorb(1), absorb(2), squeeze (triggers permute at rate boundary)
    function test_absorb_1_2_squeeze() public {
        PoseidonSponge.Sponge memory s = PoseidonSponge.init();
        PoseidonSponge.absorb(s, 1);
        PoseidonSponge.absorb(s, 2);
        uint256 result = PoseidonSponge.squeeze(s);
        emit log_named_uint("absorb_1_2_squeeze", result);
    }

    /// Test 5: absorb(1), absorb(2), absorb(3), squeeze (rate overflow → permute, then more absorb)
    function test_absorb_1_2_3_squeeze() public {
        PoseidonSponge.Sponge memory s = PoseidonSponge.init();
        PoseidonSponge.absorb(s, 1);
        PoseidonSponge.absorb(s, 2);
        PoseidonSponge.absorb(s, 3);
        uint256 result = PoseidonSponge.squeeze(s);
        emit log_named_uint("absorb_1_2_3_squeeze", result);
    }

    /// Test 6: squeeze, then squeeze again (double squeeze)
    function test_double_squeeze() public {
        PoseidonSponge.Sponge memory s = PoseidonSponge.init();
        PoseidonSponge.absorb(s, 99);
        uint256 result1 = PoseidonSponge.squeeze(s);
        uint256 result2 = PoseidonSponge.squeeze(s);
        emit log_named_uint("double_squeeze_1", result1);
        emit log_named_uint("double_squeeze_2", result2);
    }

    /// Test 7: absorb a large field element
    function test_absorb_large() public {
        PoseidonSponge.Sponge memory s = PoseidonSponge.init();
        // Use Fq modulus - 1 as input
        uint256 large = 21888242871839275222246405745257275088696311157297823662689037894645226208582;
        PoseidonSponge.absorb(s, large);
        uint256 result = PoseidonSponge.squeeze(s);
        emit log_named_uint("absorb_large_squeeze", result);
    }

    /// Test 8: absorb pair then squeeze
    function test_absorb_pair_squeeze() public {
        PoseidonSponge.Sponge memory s = PoseidonSponge.init();
        PoseidonSponge.absorbPair(s, 100, 200);
        uint256 result = PoseidonSponge.squeeze(s);
        emit log_named_uint("absorb_pair_100_200_squeeze", result);
    }
}
