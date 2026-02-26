// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {PoseidonSponge} from "./PoseidonSponge.sol";
import {SumcheckVerifier} from "./SumcheckVerifier.sol";
import {HyraxVerifier} from "./HyraxVerifier.sol";

/// @title GKRVerifier
/// @notice Verifies GKR (Goldwasser-Kalai-Rothblum) interactive proofs
/// @dev Orchestrates layer-by-layer verification of a GKR circuit:
///
///   1. Start from the output layer — verify claimed outputs
///   2. For each layer (output → input), run a sumcheck reduction
///   3. The sumcheck reduces claims on layer L to claims on layer L-1
///   4. At the input layer, verify against Hyrax commitments
///
///   The GKR protocol reduces verification of the full circuit to
///   a single polynomial evaluation on the input layer, which is
///   then checked via the Hyrax PCS.
library GKRVerifier {
    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @notice BN254 scalar field modulus (Fr)
    uint256 internal constant FR_MODULUS =
        21888242871839275222246405745257275088548364400416034343698204186575808495617;

    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice Description of a GKR circuit (structure, not witness)
    struct CircuitDescription {
        uint256 numLayers; // Number of circuit layers
        uint256[] layerSizes; // Size of each layer (number of gates)
        uint8[] layerTypes; // Layer type (0=add, 1=mul, 2=const, 3=input)
        bool[] isCommitted; // Whether each layer has Hyrax commitment
    }

    /// @notice Claims about a layer's multilinear extension evaluation
    struct LayerClaim {
        uint256[] point; // Evaluation point (vector of challenges)
        uint256 value; // Claimed evaluation value
    }

    /// @notice Per-layer GKR proof data
    struct LayerProof {
        SumcheckVerifier.SumcheckProof sumcheckProof; // Sumcheck for this layer
        uint256[] claimsOnPrevLayer; // Resulting claims on previous layer
    }

    /// @notice Complete GKR proof
    struct GKRProof {
        LayerProof[] layerProofs; // One per non-input layer
        HyraxVerifier.EvalProof[] inputProofs; // Hyrax proofs for input layers
        uint256[] outputValues; // Claimed output layer values
    }

    // ========================================================================
    // VERIFICATION
    // ========================================================================

    /// @notice Verify a complete GKR proof
    /// @param proof The GKR proof
    /// @param circuit The circuit description
    /// @param publicInputs Public input values (for non-committed input layers)
    /// @return valid Whether the proof is valid
    function verify(GKRProof memory proof, CircuitDescription memory circuit, uint256[] memory publicInputs)
        internal
        view
        returns (bool)
    {
        require(proof.layerProofs.length == circuit.numLayers - 1, "GKRVerifier: wrong number of layer proofs");

        // Initialize Fiat-Shamir transcript
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();

        // Absorb circuit description into transcript
        PoseidonSponge.absorb(sponge, circuit.numLayers);
        for (uint256 i = 0; i < circuit.layerSizes.length; i++) {
            PoseidonSponge.absorb(sponge, circuit.layerSizes[i]);
        }

        // Step 1: Process output layer
        // Absorb output values and derive initial claim
        for (uint256 i = 0; i < proof.outputValues.length; i++) {
            PoseidonSponge.absorb(sponge, proof.outputValues[i]);
        }

        // Generate random linear combination challenge for output claims
        uint256 rlcChallenge = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        // Combine output claims into a single claim via RLC
        uint256 currentClaim = 0;
        uint256 rlcPower = 1;
        for (uint256 i = 0; i < proof.outputValues.length; i++) {
            currentClaim = addmod(currentClaim, mulmod(proof.outputValues[i], rlcPower, FR_MODULUS), FR_MODULUS);
            rlcPower = mulmod(rlcPower, rlcChallenge, FR_MODULUS);
        }

        // Step 2: Layer-by-layer verification (output → input direction)
        uint256[] memory currentPoint;

        for (uint256 layerIdx = circuit.numLayers - 1; layerIdx >= 1; layerIdx--) {
            uint256 proofIdx = circuit.numLayers - 1 - layerIdx;
            LayerProof memory layerProof = proof.layerProofs[proofIdx];

            // Absorb the current claim
            PoseidonSponge.absorb(sponge, currentClaim);

            // Run sumcheck verification for this layer
            SumcheckVerifier.SumcheckResult memory sumcheckResult =
                SumcheckVerifier.verify(layerProof.sumcheckProof, currentClaim, sponge);

            if (!sumcheckResult.valid) {
                return false;
            }

            // Extract claims on the previous layer
            require(layerProof.claimsOnPrevLayer.length > 0, "GKRVerifier: no claims on previous layer");

            // The sumcheck reduces to claims on the previous layer's MLE
            // For add gates: claim = f(r) = g(r_left) + g(r_right)
            // For mul gates: claim = f(r) = g(r_left) * g(r_right)
            // The verifier checks that the final sumcheck evaluation
            // matches the claimed value computed from the previous layer claims.

            // Update the current claim for the next layer
            if (layerProof.claimsOnPrevLayer.length == 1) {
                currentClaim = layerProof.claimsOnPrevLayer[0];
            } else {
                // Multiple claims: combine with RLC
                uint256 layerRlc = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
                currentClaim = 0;
                uint256 power = 1;
                for (uint256 j = 0; j < layerProof.claimsOnPrevLayer.length; j++) {
                    currentClaim =
                        addmod(currentClaim, mulmod(layerProof.claimsOnPrevLayer[j], power, FR_MODULUS), FR_MODULUS);
                    power = mulmod(power, layerRlc, FR_MODULUS);
                }
            }

            currentPoint = sumcheckResult.challenges;

            if (layerIdx == 1) break; // Exit before underflow
        }

        // Step 3: Verify input layer claims
        // For committed (private) input layers, verify via Hyrax PCS
        // For public input layers, verify against provided public inputs

        uint256 hyraxProofIdx = 0;
        for (uint256 i = 0; i < circuit.numLayers; i++) {
            if (circuit.layerTypes[i] != 3) continue; // Not an input layer

            if (circuit.isCommitted[i]) {
                // Verify Hyrax evaluation proof
                require(hyraxProofIdx < proof.inputProofs.length, "GKRVerifier: missing Hyrax proof");

                // Split challenges into L and R tensors for Hyrax
                uint256 halfLen = currentPoint.length / 2;
                uint256[] memory lCoeffs = new uint256[](halfLen);
                uint256[] memory rCoeffs = new uint256[](currentPoint.length - halfLen);

                for (uint256 j = 0; j < halfLen; j++) {
                    lCoeffs[j] = currentPoint[j];
                }
                for (uint256 j = halfLen; j < currentPoint.length; j++) {
                    rCoeffs[j - halfLen] = currentPoint[j];
                }

                bool hyraxValid =
                    HyraxVerifier.verifyEvaluation(proof.inputProofs[hyraxProofIdx], lCoeffs, rCoeffs, currentClaim);

                if (!hyraxValid) return false;
                hyraxProofIdx++;
            } else {
                // Public input: verify claim matches the provided public inputs
                // The claim should equal the MLE of public inputs evaluated at currentPoint
                uint256 mleEval = evaluateMLEFromData(publicInputs, currentPoint);
                if (mleEval != currentClaim) return false;
            }
        }

        return true;
    }

    /// @notice Evaluate the multilinear extension of data at a given point
    /// @dev MLE(x) = sum_{w in {0,1}^n} data[w] * prod_{i} (x_i * w_i + (1-x_i) * (1-w_i))
    function evaluateMLEFromData(uint256[] memory data, uint256[] memory point) internal pure returns (uint256) {
        uint256 n = point.length;
        require(data.length <= (1 << n), "GKRVerifier: data too large for point");

        uint256 result = 0;

        for (uint256 w = 0; w < data.length; w++) {
            if (data[w] == 0) continue;

            // Compute the eq polynomial: eq(point, w)
            uint256 eq = 1;
            for (uint256 i = 0; i < n; i++) {
                uint256 wi = (w >> i) & 1;
                uint256 xi = point[i];

                if (wi == 1) {
                    eq = mulmod(eq, xi, FR_MODULUS);
                } else {
                    eq = mulmod(eq, addmod(1, FR_MODULUS - xi, FR_MODULUS), FR_MODULUS);
                }
            }

            result = addmod(result, mulmod(data[w], eq, FR_MODULUS), FR_MODULUS);
        }

        return result;
    }
}
