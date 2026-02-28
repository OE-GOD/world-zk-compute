// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {PoseidonSponge} from "./PoseidonSponge.sol";
import {SumcheckVerifier} from "./SumcheckVerifier.sol";
import {HyraxVerifier} from "./HyraxVerifier.sol";
import {CommittedSumcheckVerifier} from "./CommittedSumcheckVerifier.sol";

/// @title GKRVerifier
/// @notice Verifies GKR (Goldwasser-Kalai-Rothblum) interactive proofs
/// @dev Orchestrates layer-by-layer verification of a GKR circuit using
///      committed sumcheck (EC point messages + PODP) for each layer.
///
///   1. Start from the output layer — lift scalar claim to commitment
///   2. For each layer (output → input), verify committed sumcheck + ProofOfProduct
///   3. At the input layer, verify against Hyrax commitments
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

    /// @notice Per-layer GKR proof data (committed sumcheck version)
    struct CommittedLayerProof {
        CommittedSumcheckVerifier.CommittedSumcheckProof sumcheckProof;
        HyraxVerifier.G1Point[] commitments; // PostSumcheckLayer commitment values
        HyraxVerifier.ProofOfProduct[] pops; // Product relation proofs
    }

    /// @notice Complete GKR proof (committed version)
    struct GKRProof {
        CommittedLayerProof[] layerProofs; // One per non-input layer
        HyraxVerifier.EvalProof[] inputProofs; // Hyrax proofs for input layers
        uint256[] outputValues; // Claimed output layer values
    }

    // ========================================================================
    // VERIFICATION
    // ========================================================================

    /// @notice Verify a complete GKR proof with committed sumcheck
    /// @param proof The GKR proof
    /// @param circuit The circuit description
    /// @param publicInputs Public input values (for non-committed input layers)
    /// @param gens Pedersen generators for PODP/PoP verification
    /// @return valid Whether the proof is valid
    function verify(
        GKRProof memory proof,
        CircuitDescription memory circuit,
        uint256[] memory publicInputs,
        HyraxVerifier.PedersenGens memory gens
    ) internal view returns (bool) {
        require(proof.layerProofs.length == circuit.numLayers - 1, "GKRVerifier: wrong number of layer proofs");

        // Initialize Fiat-Shamir transcript
        PoseidonSponge.Sponge memory sponge = PoseidonSponge.init();

        // Absorb circuit description into transcript
        PoseidonSponge.absorb(sponge, circuit.numLayers);
        for (uint256 i = 0; i < circuit.layerSizes.length; i++) {
            PoseidonSponge.absorb(sponge, circuit.layerSizes[i]);
        }

        // Step 1: Process output layer — compute scalar claim and lift to commitment
        for (uint256 i = 0; i < proof.outputValues.length; i++) {
            PoseidonSponge.absorb(sponge, proof.outputValues[i]);
        }

        uint256 rlcChallenge = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        // Combine output claims into single scalar via RLC
        uint256 scalarClaim = 0;
        uint256 rlcPower = 1;
        for (uint256 i = 0; i < proof.outputValues.length; i++) {
            scalarClaim = addmod(scalarClaim, mulmod(proof.outputValues[i], rlcPower, FR_MODULUS), FR_MODULUS);
            rlcPower = mulmod(rlcPower, rlcChallenge, FR_MODULUS);
        }

        // Lift scalar claim to commitment: eval * g_scalar (blinding = 0)
        HyraxVerifier.G1Point memory currentClaimCommitment = HyraxVerifier.scalarMul(gens.scalarGen, scalarClaim);

        // Step 2: Layer-by-layer committed sumcheck verification
        uint256[] memory currentBindings;
        (currentBindings, currentClaimCommitment) = _verifyLayers(proof, circuit, gens, sponge, currentClaimCommitment);

        // Step 3: Verify input layer claims
        return _verifyInputLayers(proof, circuit, publicInputs, gens, sponge, currentBindings, currentClaimCommitment);
    }

    /// @notice Verify all non-input layers via committed sumcheck
    function _verifyLayers(
        GKRProof memory proof,
        CircuitDescription memory circuit,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge,
        HyraxVerifier.G1Point memory currentClaimCommitment
    ) private view returns (uint256[] memory currentBindings, HyraxVerifier.G1Point memory claimOut) {
        claimOut = currentClaimCommitment;

        for (uint256 layerIdx = circuit.numLayers - 1; layerIdx >= 1; layerIdx--) {
            uint256 proofIdx = circuit.numLayers - 1 - layerIdx;

            (currentBindings, claimOut) =
                _verifyOneLayer(proof.layerProofs[proofIdx], circuit.layerTypes[layerIdx], gens, sponge);

            if (layerIdx == 1) break;
        }
    }

    /// @notice Verify a single committed layer (extracted to avoid stack-too-deep)
    function _verifyOneLayer(
        CommittedLayerProof memory layerProof,
        uint8 layerType,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge
    ) private view returns (uint256[] memory bindings, HyraxVerifier.G1Point memory nextClaim) {
        // Absorb sumcheck messages and derive bindings
        bindings = _absorbMessagesAndDeriveBindings(layerProof.sumcheckProof.messages, sponge);

        // Absorb post-sumcheck commitments
        for (uint256 j = 0; j < layerProof.commitments.length; j++) {
            PoseidonSponge.absorb(sponge, layerProof.commitments[j].x);
            PoseidonSponge.absorb(sponge, layerProof.commitments[j].y);
        }

        // Compute oracle_eval and verify committed sumcheck
        HyraxVerifier.G1Point memory oracleEval = _computeOracleEval(layerProof.commitments, layerType);

        bool scValid = CommittedSumcheckVerifier.verify(layerProof.sumcheckProof, oracleEval, 2, bindings, gens, sponge);
        require(scValid, "GKRVerifier: committed sumcheck failed");

        // Verify ProofOfProduct
        require(_verifyProducts(layerProof, gens, sponge), "GKRVerifier: PoP failed");

        // Extract claim for next layer
        nextClaim = _extractClaimCommitment(layerProof.commitments);
    }

    // ========================================================================
    // LAYER HELPERS
    // ========================================================================

    /// @notice Absorb committed sumcheck messages and derive bindings
    /// @dev From layer.rs: absorb first message, then for each subsequent message
    ///      squeeze a binding challenge and absorb the next message.
    function _absorbMessagesAndDeriveBindings(
        HyraxVerifier.G1Point[] memory messages,
        PoseidonSponge.Sponge memory sponge
    ) private pure returns (uint256[] memory bindings) {
        uint256 n = messages.length;
        bindings = new uint256[](n);

        // Absorb first message
        if (n > 0) {
            PoseidonSponge.absorb(sponge, messages[0].x);
            PoseidonSponge.absorb(sponge, messages[0].y);
        }

        // For each subsequent message: squeeze binding, absorb message
        for (uint256 i = 1; i < n; i++) {
            bindings[i - 1] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
            PoseidonSponge.absorb(sponge, messages[i].x);
            PoseidonSponge.absorb(sponge, messages[i].y);
        }

        // Final binding
        if (n > 0) {
            bindings[n - 1] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
        }
    }

    /// @notice Compute oracle_eval from PostSumcheckLayer commitments
    /// @dev For multiply gates: oracle_eval = last commitment (product result, coeff=1)
    ///      For add gates: oracle_eval = sum of all commitments (each coeff=1)
    ///      This is a simplification — production needs full product descriptor.
    function _computeOracleEval(HyraxVerifier.G1Point[] memory commitments, uint8 layerType)
        private
        view
        returns (HyraxVerifier.G1Point memory)
    {
        require(commitments.length > 0, "GKRVerifier: no commitments");

        if (layerType == 1) {
            // Multiply gate: result is the last commitment (final Composite)
            return commitments[commitments.length - 1];
        } else {
            // Add gate or other: oracle_eval = sum of commitments
            HyraxVerifier.G1Point memory result = commitments[0];
            for (uint256 i = 1; i < commitments.length; i++) {
                result = HyraxVerifier.ecAdd(result, commitments[i]);
            }
            return result;
        }
    }

    /// @notice Verify ProofOfProduct entries for a layer
    /// @dev For multiply gate with k=2 factors:
    ///      Product triple = (commitments[1], commitments[2], commitments[3])
    ///      = (Composite(a), Atom(b), Composite(a*b))
    function _verifyProducts(
        CommittedLayerProof memory layerProof,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge
    ) private view returns (bool) {
        if (layerProof.pops.length == 0) return true;

        // For standard k=2 multiply gates:
        // intermediates = [Atom(a), Composite(a), Atom(b), Composite(ab)]
        // Triple: (Composite(a), Atom(b), Composite(ab)) = (commitments[1], commitments[2], commitments[3])
        //
        // For layers with multiple products, triples are at offsets:
        // product i starts at commitments[i * 2 * k_factors]
        // This is a simplification for k=2.
        uint256 commitIdx = 1;
        for (uint256 i = 0; i < layerProof.pops.length; i++) {
            require(commitIdx + 2 <= layerProof.commitments.length, "GKRVerifier: not enough commitments for PoP");

            bool popValid = HyraxVerifier.verifyProofOfProduct(
                layerProof.pops[i],
                layerProof.commitments[commitIdx], // com_x (Composite of partial product)
                layerProof.commitments[commitIdx + 1], // com_y (next Atom)
                layerProof.commitments[commitIdx + 2], // com_z (new Composite)
                gens,
                sponge
            );
            if (!popValid) return false;

            commitIdx += 2; // Move to next triple
        }
        return true;
    }

    /// @notice Extract claim commitment for the next layer
    /// @dev The first commitment (Atom[0]) represents the claim on the previous layer.
    function _extractClaimCommitment(HyraxVerifier.G1Point[] memory commitments)
        private
        pure
        returns (HyraxVerifier.G1Point memory)
    {
        require(commitments.length > 0, "GKRVerifier: no commitments for claim");
        return commitments[0];
    }

    // ========================================================================
    // INPUT LAYER VERIFICATION
    // ========================================================================

    /// @notice Verify input layer claims
    function _verifyInputLayers(
        GKRProof memory proof,
        CircuitDescription memory circuit,
        uint256[] memory publicInputs,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge,
        uint256[] memory currentBindings,
        HyraxVerifier.G1Point memory currentClaimCommitment
    ) private view returns (bool) {
        uint256 hyraxProofIdx = 0;
        for (uint256 i = 0; i < circuit.numLayers; i++) {
            if (circuit.layerTypes[i] != 3) continue; // Not an input layer

            if (circuit.isCommitted[i]) {
                if (!_verifyCommittedInput(
                        proof.inputProofs[hyraxProofIdx], currentBindings, currentClaimCommitment, sponge, gens
                    )) {
                    return false;
                }
                hyraxProofIdx++;
            } else {
                // Public input: verify claim matches the MLE of public inputs
                uint256 mleEval = evaluateMLEFromData(publicInputs, currentBindings);
                // Compute expected commitment: mleEval * g_scalar
                HyraxVerifier.G1Point memory expectedCom = HyraxVerifier.scalarMul(gens.scalarGen, mleEval);
                if (!HyraxVerifier.isEqual(expectedCom, currentClaimCommitment)) return false;
            }
        }

        return true;
    }

    /// @notice Verify a committed input layer via Hyrax+PODP
    function _verifyCommittedInput(
        HyraxVerifier.EvalProof memory inputProof,
        uint256[] memory currentBindings,
        HyraxVerifier.G1Point memory currentClaimCommitment,
        PoseidonSponge.Sponge memory sponge,
        HyraxVerifier.PedersenGens memory gens
    ) private view returns (bool) {
        // Split bindings into L and R tensors for Hyrax
        uint256 halfLen = currentBindings.length / 2;
        uint256[] memory lCoeffs = new uint256[](halfLen);
        uint256[] memory rCoeffs = new uint256[](currentBindings.length - halfLen);

        for (uint256 j = 0; j < halfLen; j++) {
            lCoeffs[j] = currentBindings[j];
        }
        for (uint256 j = halfLen; j < currentBindings.length; j++) {
            rCoeffs[j - halfLen] = currentBindings[j];
        }

        // Derive PODP challenge from transcript
        PoseidonSponge.absorb(sponge, inputProof.podp.commitD.x);
        PoseidonSponge.absorb(sponge, inputProof.podp.commitD.y);
        PoseidonSponge.absorb(sponge, inputProof.podp.commitDDotA.x);
        PoseidonSponge.absorb(sponge, inputProof.podp.commitDDotA.y);
        uint256 podpChallenge = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        // Absorb z_vector, z_delta, z_beta for transcript consistency
        for (uint256 j = 0; j < inputProof.podp.zVector.length; j++) {
            PoseidonSponge.absorb(sponge, inputProof.podp.zVector[j]);
        }
        PoseidonSponge.absorb(sponge, inputProof.podp.zDelta);
        PoseidonSponge.absorb(sponge, inputProof.podp.zBeta);

        return HyraxVerifier.verifyEvaluation(inputProof, lCoeffs, rCoeffs, 0, podpChallenge, gens);
    }

    // ========================================================================
    // MLE EVALUATION
    // ========================================================================

    /// @notice Evaluate the multilinear extension of data at a given point
    /// @dev MLE(x) = sum_{w in {0,1}^n} data[w] * prod_{i} (x_i * w_i + (1-x_i) * (1-w_i))
    function evaluateMLEFromData(uint256[] memory data, uint256[] memory point) internal pure returns (uint256) {
        uint256 n = point.length;
        require(data.length <= (1 << n), "GKRVerifier: data too large for point");

        uint256 result = 0;

        for (uint256 w = 0; w < data.length; w++) {
            if (data[w] == 0) continue;

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
