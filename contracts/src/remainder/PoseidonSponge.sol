// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title PoseidonSponge
/// @notice Poseidon hash sponge for Fiat-Shamir transcript in Remainder proofs
/// @dev Parameters: t=3, rate=2, full_rounds=8, partial_rounds=57 over BN254 Fq
///
/// The sponge state consists of 3 field elements. Data is absorbed in pairs
/// (rate=2), and squeezed one element at a time.
///
/// This implementation must exactly match Remainder's PoseidonSponge in Rust
/// to produce identical Fiat-Shamir challenges.
library PoseidonSponge {
    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @notice BN254 base field modulus (Fq)
    uint256 internal constant FQ_MODULUS =
        21888242871839275222246405745257275088696311157297823662689037894645226208583;

    /// @notice Number of state elements
    uint256 internal constant T = 3;

    /// @notice Rate (elements absorbed per permutation)
    uint256 internal constant RATE = 2;

    /// @notice Full rounds (applied at start and end)
    uint256 internal constant FULL_ROUNDS = 8;

    /// @notice Partial rounds (applied in the middle)
    uint256 internal constant PARTIAL_ROUNDS = 57;

    /// @notice Total rounds
    uint256 internal constant TOTAL_ROUNDS = 65; // 8 + 57

    // ========================================================================
    // TYPES
    // ========================================================================

    struct Sponge {
        uint256[3] state;
        uint256 absorb_pos; // Current position in rate buffer (0 or 1)
        bool squeezed; // Whether we've squeezed since last absorb
    }

    // ========================================================================
    // SPONGE OPERATIONS
    // ========================================================================

    /// @notice Initialize a new sponge with zero state
    function init() internal pure returns (Sponge memory) {
        uint256[3] memory state;
        return Sponge({state: state, absorb_pos: 0, squeezed: false});
    }

    /// @notice Absorb a single field element into the sponge
    /// @param self The sponge state
    /// @param value The Fq element to absorb
    function absorb(Sponge memory self, uint256 value) internal pure {
        require(value < FQ_MODULUS, "PoseidonSponge: value >= Fq modulus");

        self.state[self.absorb_pos] = addmod(self.state[self.absorb_pos], value, FQ_MODULUS);
        self.absorb_pos++;
        self.squeezed = false;

        if (self.absorb_pos == RATE) {
            permute(self);
            self.absorb_pos = 0;
        }
    }

    /// @notice Absorb multiple field elements
    /// @param self The sponge state
    /// @param values Array of Fq elements
    function absorbMany(Sponge memory self, uint256[] memory values) internal pure {
        for (uint256 i = 0; i < values.length; i++) {
            absorb(self, values[i]);
        }
    }

    /// @notice Absorb a G1 affine point as (x, y) coordinates
    /// @param self The sponge state
    /// @param x The x-coordinate (Fq element)
    /// @param y The y-coordinate (Fq element)
    function absorbPoint(Sponge memory self, uint256 x, uint256 y) internal pure {
        absorb(self, x);
        absorb(self, y);
    }

    /// @notice Squeeze a single field element from the sponge
    /// @param self The sponge state
    /// @return The squeezed Fq element (used as Fiat-Shamir challenge)
    function squeeze(Sponge memory self) internal pure returns (uint256) {
        if (!self.squeezed) {
            // Pad remaining rate elements and permute
            if (self.absorb_pos > 0) {
                permute(self);
                self.absorb_pos = 0;
            }
            self.squeezed = true;
        }

        uint256 output = self.state[0];
        // Permute for next squeeze
        permute(self);
        return output;
    }

    // ========================================================================
    // POSEIDON PERMUTATION
    // ========================================================================

    /// @notice Apply the Poseidon permutation to the sponge state
    /// @dev Follows the Poseidon specification:
    ///   1. First FULL_ROUNDS/2 full rounds (S-box on all state elements)
    ///   2. PARTIAL_ROUNDS partial rounds (S-box on first element only)
    ///   3. Last FULL_ROUNDS/2 full rounds
    function permute(Sponge memory self) internal pure {
        uint256 roundIdx = 0;

        // First half of full rounds
        for (uint256 i = 0; i < FULL_ROUNDS / 2; i++) {
            addRoundConstants(self, roundIdx);
            fullSBox(self);
            mixLayer(self);
            roundIdx++;
        }

        // Partial rounds
        for (uint256 i = 0; i < PARTIAL_ROUNDS; i++) {
            addRoundConstants(self, roundIdx);
            partialSBox(self);
            mixLayer(self);
            roundIdx++;
        }

        // Second half of full rounds
        for (uint256 i = 0; i < FULL_ROUNDS / 2; i++) {
            addRoundConstants(self, roundIdx);
            fullSBox(self);
            mixLayer(self);
            roundIdx++;
        }
    }

    /// @notice Apply S-box (x^5) to all state elements (full round)
    function fullSBox(Sponge memory self) internal pure {
        for (uint256 i = 0; i < T; i++) {
            self.state[i] = sbox(self.state[i]);
        }
    }

    /// @notice Apply S-box (x^5) to first state element only (partial round)
    function partialSBox(Sponge memory self) internal pure {
        self.state[0] = sbox(self.state[0]);
    }

    /// @notice Compute x^5 mod Fq (the S-box)
    function sbox(uint256 x) internal pure returns (uint256) {
        uint256 x2 = mulmod(x, x, FQ_MODULUS);
        uint256 x4 = mulmod(x2, x2, FQ_MODULUS);
        return mulmod(x4, x, FQ_MODULUS);
    }

    /// @notice Mix layer using the MDS matrix
    /// @dev For t=3, uses the circulant matrix [[2,1,1],[1,2,1],[1,1,2]]
    ///      which provides optimal diffusion
    function mixLayer(Sponge memory self) internal pure {
        uint256 s0 = self.state[0];
        uint256 s1 = self.state[1];
        uint256 s2 = self.state[2];

        // MDS matrix multiplication for t=3
        // M = [[2,1,1],[1,2,1],[1,1,2]]
        self.state[0] = addmod(addmod(mulmod(2, s0, FQ_MODULUS), s1, FQ_MODULUS), s2, FQ_MODULUS);
        self.state[1] = addmod(addmod(s0, mulmod(2, s1, FQ_MODULUS), FQ_MODULUS), s2, FQ_MODULUS);
        self.state[2] = addmod(addmod(s0, s1, FQ_MODULUS), mulmod(2, s2, FQ_MODULUS), FQ_MODULUS);
    }

    /// @notice Add round constants to state
    /// @dev Round constants are derived from the Grain LFSR as specified
    ///      in the Poseidon paper. For t=3, 3 constants per round.
    ///
    ///      NOTE: These are placeholder constants. Production deployment
    ///      must use the exact constants from Remainder_CE's Poseidon
    ///      configuration to ensure transcript compatibility.
    function addRoundConstants(Sponge memory self, uint256 roundIdx) internal pure {
        // Round constants are generated deterministically from:
        //   seed = keccak256("Remainder_Poseidon_BN254_t3_r2_f8_p57")
        //
        // For each round r, constant c_i:
        //   rc[r][i] = keccak256(seed || r || i) mod FQ_MODULUS
        //
        // In production, these should be precomputed and stored as
        // immutable constants for gas efficiency. Here we use a
        // deterministic derivation for correctness verification.

        bytes32 seed = keccak256("Remainder_Poseidon_BN254_t3_r2_f8_p57");

        for (uint256 i = 0; i < T; i++) {
            uint256 rc = uint256(keccak256(abi.encodePacked(seed, roundIdx, i))) % FQ_MODULUS;
            self.state[i] = addmod(self.state[i], rc, FQ_MODULUS);
        }
    }
}
