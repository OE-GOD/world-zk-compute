// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {PoseidonSponge} from "./PoseidonSponge.sol";
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
    // ERRORS
    // ========================================================================

    error WrongNumberOfLayerProofs();
    error NoOutputClaimCommitments();
    error CommittedSumcheckFailed();
    error ProofOfProductFailed();
    error NoCommitments();
    error SubtractNeedsTwoCommitments();
    error NotEnoughCommitmentsForPoP();
    error NoCommitmentsForClaim();
    error NoBindingsForReferencingLayer();
    error CommittedInputVerificationFailed();
    error PublicInputCommitmentMismatch();
    error Log2OfZero();
    error DataTooLargeForPoint();

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
        HyraxVerifier.G1Point[] outputClaimCommitments; // Prover's Pedersen commitments to output claims
    }

    // ========================================================================
    // VERIFICATION
    // ========================================================================

    /// @notice Verify a complete GKR proof with committed sumcheck
    /// @param proof The GKR proof
    /// @param circuit The circuit description
    /// @param publicInputs Public input values (= output claims for the circuit)
    /// @param gens Pedersen generators for PODP/PoP verification
    /// @param sponge Pre-initialized Fiat-Shamir transcript (initial absorbs already done by caller)
    /// @return valid Whether the proof is valid
    function verify(
        GKRProof memory proof,
        CircuitDescription memory circuit,
        uint256[] memory publicInputs,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge
    ) internal view returns (bool) {
        if (proof.layerProofs.length != circuit.numLayers - 1) revert WrongNumberOfLayerProofs();

        // Step 1: Squeeze output challenge — becomes claim_point for first layer
        uint256 outputChallenge = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        // Use the prover's Pedersen commitment to the output claim (includes blinding factor).
        if (proof.outputClaimCommitments.length == 0) revert NoOutputClaimCommitments();
        HyraxVerifier.G1Point memory currentClaimCommitment = proof.outputClaimCommitments[0];

        // Absorb claim commitment and squeeze claim aggregation coefficient
        PoseidonSponge.absorb(sponge, currentClaimCommitment.x);
        PoseidonSponge.absorb(sponge, currentClaimCommitment.y);
        uint256 randomCoeff = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        // Build initial claimPoint array (single element for single output claim)
        uint256[] memory claimPoint = new uint256[](1);
        claimPoint[0] = outputChallenge;

        // Step 2: Layer-by-layer committed sumcheck verification
        // Track bindings from each layer for input claim point derivation
        uint256[][] memory allBindings = new uint256[][](circuit.numLayers);
        uint256[] memory currentBindings;
        (currentBindings, currentClaimCommitment) =
            _verifyLayers(proof, circuit, gens, sponge, currentClaimCommitment, claimPoint, randomCoeff, allBindings);

        // Step 3: Verify input layer claims using tracked bindings
        return _verifyInputLayers(proof, circuit, publicInputs, gens, sponge, allBindings, currentClaimCommitment);
    }

    /// @notice Verify all non-input layers via committed sumcheck
    function _verifyLayers(
        GKRProof memory proof,
        CircuitDescription memory circuit,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge,
        HyraxVerifier.G1Point memory currentClaimCommitment,
        uint256[] memory claimPoint,
        uint256 randomCoeff,
        uint256[][] memory allBindings
    ) private view returns (uint256[] memory currentBindings, HyraxVerifier.G1Point memory claimOut) {
        claimOut = currentClaimCommitment;

        for (uint256 layerIdx = circuit.numLayers - 1; layerIdx >= 1; layerIdx--) {
            uint256 proofIdx = circuit.numLayers - 1 - layerIdx;

            (currentBindings, claimOut) = _verifyOneLayer(
                proof.layerProofs[proofIdx], circuit.layerTypes[layerIdx], gens, sponge, claimPoint, randomCoeff
            );

            // Save bindings for this layer (used later for input claim derivation)
            allBindings[layerIdx] = currentBindings;

            if (layerIdx == 1) break;

            // Update claimPoint for next layer: previous layer's bindings become next layer's claim point
            claimPoint = currentBindings;

            // Squeeze claim aggregation coefficient between layers
            randomCoeff = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
        }
    }

    /// @notice Verify a single committed layer (extracted to avoid stack-too-deep)
    function _verifyOneLayer(
        CommittedLayerProof memory layerProof,
        uint8 layerType,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge,
        uint256[] memory claimPoint,
        uint256 randomCoeff
    ) private view returns (uint256[] memory bindings, HyraxVerifier.G1Point memory nextClaim) {
        // Absorb sumcheck messages and derive bindings
        bindings = _absorbMessagesAndDeriveBindings(layerProof.sumcheckProof.messages, sponge);

        // Absorb post-sumcheck commitments
        for (uint256 j = 0; j < layerProof.commitments.length; j++) {
            PoseidonSponge.absorb(sponge, layerProof.commitments[j].x);
            PoseidonSponge.absorb(sponge, layerProof.commitments[j].y);
        }

        // Compute rlc_beta = beta(bindings, claimPoint) * randomCoeff
        uint256 rlcBeta = _computeRlcBeta(bindings, claimPoint, randomCoeff);

        // Compute oracle_eval scaled by rlc_beta
        HyraxVerifier.G1Point memory oracleEval = _computeOracleEval(layerProof.commitments, layerType, rlcBeta);

        // Degree: type 0 (add/subtract) = 2, type 1 (multiply) = 3
        uint256 degree = (layerType == 1) ? 3 : 2;

        bool scValid =
            CommittedSumcheckVerifier.verify(layerProof.sumcheckProof, oracleEval, degree, bindings, gens, sponge);
        if (!scValid) revert CommittedSumcheckFailed();

        // Verify ProofOfProduct
        if (!_verifyProducts(layerProof, gens, sponge)) revert ProofOfProductFailed();

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

    /// @notice Compute rlc_beta = beta(bindings, claimPoint) * randomCoeff
    /// @dev beta(r, c) = prod_i (r_i * c_i + (1 - r_i) * (1 - c_i))
    function _computeRlcBeta(uint256[] memory bindings, uint256[] memory claimPoint, uint256 randomCoeff)
        private
        pure
        returns (uint256)
    {
        uint256 n = bindings.length < claimPoint.length ? bindings.length : claimPoint.length;
        uint256 beta = 1;
        for (uint256 i = 0; i < n; i++) {
            // term = r_i * c_i + (1 - r_i) * (1 - c_i)
            uint256 rc = mulmod(bindings[i], claimPoint[i], FR_MODULUS);
            uint256 oneMinusR = addmod(1, FR_MODULUS - bindings[i], FR_MODULUS);
            uint256 oneMinusC = addmod(1, FR_MODULUS - claimPoint[i], FR_MODULUS);
            uint256 term = addmod(rc, mulmod(oneMinusR, oneMinusC, FR_MODULUS), FR_MODULUS);
            beta = mulmod(beta, term, FR_MODULUS);
        }
        return mulmod(beta, randomCoeff, FR_MODULUS);
    }

    /// @notice Compute oracle_eval from PostSumcheckLayer commitments
    /// @dev oracle_eval = sum(product_result * coefficient)
    ///      For subtract gate (type 0): coefficients are [rlcBeta, -rlcBeta]
    ///        oracle_eval = rlcBeta * commitment[0] - rlcBeta * commitment[1]
    ///      For multiply gate (type 1): coefficient is [rlcBeta]
    ///        oracle_eval = rlcBeta * commitment[last]
    function _computeOracleEval(HyraxVerifier.G1Point[] memory commitments, uint8 layerType, uint256 rlcBeta)
        private
        view
        returns (HyraxVerifier.G1Point memory)
    {
        if (commitments.length == 0) revert NoCommitments();

        if (layerType == 1) {
            // Multiply gate: result = last commitment (final Composite), scaled by rlcBeta
            return HyraxVerifier.scalarMul(commitments[commitments.length - 1], rlcBeta);
        } else {
            // Add/subtract gate: 2 products with coefficients [rlcBeta, -rlcBeta]
            // oracle_eval = rlcBeta * commitment[0] - rlcBeta * commitment[1]
            if (commitments.length < 2) revert SubtractNeedsTwoCommitments();
            HyraxVerifier.G1Point memory pos = HyraxVerifier.scalarMul(commitments[0], rlcBeta);
            HyraxVerifier.G1Point memory neg = HyraxVerifier.scalarMul(commitments[1], FR_MODULUS - rlcBeta);
            return HyraxVerifier.ecAdd(pos, neg);
        }
    }

    /// @notice Verify ProofOfProduct entries for a layer
    /// @dev Product triples are extracted via sliding windows(3) over all
    ///      intermediates across all products:
    ///      For 2-factor product: intermediates = [Atom(a), Atom(b), Composite(ab)]
    ///        1 triple: (commitment[0], commitment[1], commitment[2])
    ///      For 3-factor product: intermediates = [Atom(a), Atom(b), Composite(ab), Atom(c), Composite(abc)]
    ///        3 triples via windows(3): (0,1,2), (1,2,3), (2,3,4)
    function _verifyProducts(
        CommittedLayerProof memory layerProof,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge
    ) private view returns (bool) {
        if (layerProof.pops.length == 0) return true;

        // PoP triples come from sliding window of size 3 over all commitment indices
        // For each product, intermediates are flattened: [Atom₁, Atom₂, Composite₁₂, ...]
        // windows(3) yields triples: (com[i], com[i+1], com[i+2])
        uint256 commitIdx = 0;
        for (uint256 i = 0; i < layerProof.pops.length; i++) {
            if (commitIdx + 2 >= layerProof.commitments.length) revert NotEnoughCommitmentsForPoP();

            bool popValid = HyraxVerifier.verifyProofOfProduct(
                layerProof.pops[i],
                layerProof.commitments[commitIdx], // com_x
                layerProof.commitments[commitIdx + 1], // com_y
                layerProof.commitments[commitIdx + 2], // com_z = x * y
                gens,
                sponge
            );
            if (!popValid) return false;

            commitIdx += 1; // Sliding window: advance by 1
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
        if (commitments.length == 0) revert NoCommitmentsForClaim();
        return commitments[0];
    }

    // ========================================================================
    // INPUT LAYER VERIFICATION
    // ========================================================================

    /// @notice Verify input layer claims
    /// @dev For committed inputs: derive claim points from the circuit structure.
    ///      The multiply layer creates 2 Atoms referencing the input layer at different offsets.
    ///      For a multiply layer of size M referencing an input layer of size N:
    ///        - Atom for first factor: point = (0, binding) [in (N/M)-split space]
    ///        - Atom for second factor: point = (1, binding) [in (N/M)-split space]
    ///      For public inputs: use the subtract layer's bindings as the claim point.
    function _verifyInputLayers(
        GKRProof memory proof,
        CircuitDescription memory circuit,
        uint256[] memory publicInputs,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge,
        uint256[][] memory allBindings,
        HyraxVerifier.G1Point memory currentClaimCommitment
    ) private view returns (bool) {
        uint256 hyraxProofIdx = 0;
        for (uint256 i = 0; i < circuit.numLayers; i++) {
            if (circuit.layerTypes[i] != 3) continue; // Not an input layer

            if (circuit.isCommitted[i]) {
                // For committed input: derive claim points from the multiply layer that references it
                // The multiply layer (layer i+1) creates claims at points based on shred offsets
                if (i + 1 >= circuit.numLayers || allBindings[i + 1].length == 0) {
                    revert NoBindingsForReferencingLayer();
                }
                uint256[] memory mulBindings = allBindings[i + 1];

                // Build claim points: for a 2-shred input (a, b), the multiply layer
                // creates 2 claims at points (0, r) and (1, r) where r = multiply binding
                uint256[][] memory claimPoints = _deriveInputClaimPoints(mulBindings, circuit.layerSizes[i]);

                if (!_verifyCommittedInputMultiClaim(proof.inputProofs[hyraxProofIdx], claimPoints, sponge, gens)) {
                    revert CommittedInputVerificationFailed();
                }
                hyraxProofIdx++;
            } else {
                // Public input: claim point = bindings from the layer that references it
                // Find the first non-input layer above this one
                uint256[] memory pubBindings = _findReferringBindings(i, circuit, allBindings);
                uint256 mleEval = evaluateMLEFromData(publicInputs, pubBindings);
                HyraxVerifier.G1Point memory expectedCom = HyraxVerifier.scalarMul(gens.scalarGen, mleEval);
                if (!HyraxVerifier.isEqual(expectedCom, currentClaimCommitment)) {
                    revert PublicInputCommitmentMismatch();
                }
            }
        }

        return true;
    }

    /// @notice Find the bindings from the layer that references input layer i
    function _findReferringBindings(
        uint256 inputLayerIdx,
        CircuitDescription memory circuit,
        uint256[][] memory allBindings
    ) private pure returns (uint256[] memory) {
        // Find first computation layer above the input layer
        for (uint256 k = inputLayerIdx + 1; k < circuit.numLayers; k++) {
            if (circuit.layerTypes[k] != 3 && allBindings[k].length > 0) {
                return allBindings[k];
            }
        }
        // Fallback: use the highest layer's bindings
        return allBindings[circuit.numLayers - 1];
    }

    /// @notice Derive claim points for a committed input layer from the referencing layer's bindings
    /// @dev For a multiply layer with binding r referencing a 2-shred input of size 2^n:
    ///      Shred a occupies positions 0..M-1, shred b occupies positions M..2M-1
    ///      After sumcheck binding r, claims are at points (0, r) and (1, r)
    ///      where the first variable selects the shred and r selects within the shred
    function _deriveInputClaimPoints(
        uint256[] memory mulBindings,
        uint256 /* inputSize */
    )
        private
        pure
        returns (uint256[][] memory claimPoints)
    {
        // For 2-shred input: 2 claim points
        // Each has (shred_selector, multiply_bindings...)
        claimPoints = new uint256[][](2);

        // Claim 0: shred a at offset 0 → first variable = 0
        claimPoints[0] = new uint256[](mulBindings.length + 1);
        claimPoints[0][0] = 0;
        for (uint256 j = 0; j < mulBindings.length; j++) {
            claimPoints[0][j + 1] = mulBindings[j];
        }

        // Claim 1: shred b at offset inputSize/2 → first variable = 1 (fully bound)
        claimPoints[1] = new uint256[](mulBindings.length + 1);
        claimPoints[1][0] = FR_MODULUS; // This represents "1" but we need FR field element 1
        // Actually FR_MODULUS mod FR_MODULUS = 0. We need 1.
        claimPoints[1][0] = 1;
        for (uint256 j = 0; j < mulBindings.length; j++) {
            claimPoints[1][j + 1] = mulBindings[j];
        }
    }

    /// @notice Verify a committed input layer with multiple claims via Hyrax
    /// @dev Computes L_tensor as RLC of individual L-tensors, R_tensor from shared R-half.
    ///      Transcript flow: squeeze RLC coeffs → absorb comEval → absorb PODP → verify.
    function _verifyCommittedInputMultiClaim(
        HyraxVerifier.EvalProof memory inputProof,
        uint256[][] memory claimPoints,
        PoseidonSponge.Sponge memory sponge,
        HyraxVerifier.PedersenGens memory gens
    ) private view returns (bool) {
        (uint256[] memory lCoeffs, uint256[] memory rCoeffs) =
            _computeMultiClaimTensors(inputProof.commitmentRows.length, claimPoints, sponge);

        // Absorb commitment to evaluation (transcript step before PODP)
        PoseidonSponge.absorb(sponge, inputProof.comEval.x);
        PoseidonSponge.absorb(sponge, inputProof.comEval.y);

        uint256 podpChallenge = _absorbInputPODP(inputProof, sponge);

        return HyraxVerifier.verifyEvaluation(inputProof, lCoeffs, rCoeffs, 0, podpChallenge, gens);
    }

    /// @notice Compute L and R tensors for multi-claim input verification
    function _computeMultiClaimTensors(
        uint256 numRows,
        uint256[][] memory claimPoints,
        PoseidonSponge.Sponge memory sponge
    ) private pure returns (uint256[] memory lCoeffs, uint256[] memory rCoeffs) {
        uint256 n = claimPoints[0].length;
        uint256 lHalfLen = _log2(numRows > 0 ? numRows : 1);
        uint256 rHalfLen = n - lHalfLen;

        // Squeeze ALL RLC coefficients (Rust: get_scalar_field_challenges squeezes N times)
        uint256[] memory rlcCoeffs = new uint256[](claimPoints.length);
        for (uint256 i = 0; i < claimPoints.length; i++) {
            rlcCoeffs[i] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
        }

        // L_tensor = RLC of individual L-tensors
        lCoeffs = _computeRlcTensor(claimPoints, lHalfLen, rlcCoeffs);

        // R_tensor from shared R-half (same for all claims)
        uint256[] memory rVars = new uint256[](rHalfLen);
        for (uint256 j = 0; j < rHalfLen; j++) {
            rVars[j] = claimPoints[0][lHalfLen + j];
        }
        rCoeffs = _initializeTensor(rVars);
    }

    /// @notice Compute RLC of tensor products for L-halves of multiple claim points
    function _computeRlcTensor(uint256[][] memory claimPoints, uint256 lHalfLen, uint256[] memory rlcCoeffs)
        private
        pure
        returns (uint256[] memory result)
    {
        uint256 tensorLen = uint256(1) << lHalfLen;
        result = new uint256[](tensorLen);

        for (uint256 c = 0; c < claimPoints.length; c++) {
            // Extract L-half variables
            uint256[] memory lVars = new uint256[](lHalfLen);
            for (uint256 j = 0; j < lHalfLen; j++) {
                lVars[j] = claimPoints[c][j];
            }

            // Compute tensor and add with RLC coefficient
            uint256[] memory tensor = _initializeTensor(lVars);
            for (uint256 j = 0; j < tensorLen; j++) {
                result[j] = addmod(result[j], mulmod(tensor[j], rlcCoeffs[c], FR_MODULUS), FR_MODULUS);
            }
        }
    }

    /// @notice Absorb input proof PODP data and return challenge
    function _absorbInputPODP(HyraxVerifier.EvalProof memory inputProof, PoseidonSponge.Sponge memory sponge)
        private
        pure
        returns (uint256)
    {
        PoseidonSponge.absorb(sponge, inputProof.podp.commitD.x);
        PoseidonSponge.absorb(sponge, inputProof.podp.commitD.y);
        PoseidonSponge.absorb(sponge, inputProof.podp.commitDDotA.x);
        PoseidonSponge.absorb(sponge, inputProof.podp.commitDDotA.y);
        uint256 podpChallenge = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        for (uint256 j = 0; j < inputProof.podp.zVector.length; j++) {
            PoseidonSponge.absorb(sponge, inputProof.podp.zVector[j]);
        }
        PoseidonSponge.absorb(sponge, inputProof.podp.zDelta);
        PoseidonSponge.absorb(sponge, inputProof.podp.zBeta);

        return podpChallenge;
    }

    /// @notice Compute tensor product of challenge coordinates
    /// @dev Matches Rust's initialize_tensor: iterates in reverse order.
    ///      tensor([]) = [1]
    ///      tensor([r]) = [(1-r), r]
    ///      tensor([r1, r2]) = [(1-r1)(1-r2), r1(1-r2), (1-r1)r2, r1*r2]
    function _initializeTensor(uint256[] memory coords) private pure returns (uint256[] memory table) {
        uint256 size = uint256(1) << coords.length;
        table = new uint256[](size);
        table[0] = 1;

        if (coords.length == 0) return table;

        uint256 len = 1;
        // Iterate in reverse order (matching Rust's .iter().rev())
        for (uint256 idx = coords.length; idx > 0; idx--) {
            uint256 r = coords[idx - 1];
            uint256 oneMinusR = addmod(1, FR_MODULUS - r, FR_MODULUS);

            // Double the table: lower half *= (1-r), upper half = lower * r
            for (uint256 i = 0; i < len; i++) {
                table[i + len] = mulmod(table[i], r, FR_MODULUS);
                table[i] = mulmod(table[i], oneMinusR, FR_MODULUS);
            }
            len *= 2;
        }
    }

    /// @notice Compute floor(log2(x)) for x > 0
    function _log2(uint256 x) private pure returns (uint256 result) {
        if (x == 0) revert Log2OfZero();
        result = 0;
        uint256 v = x;
        while (v > 1) {
            v >>= 1;
            result++;
        }
    }

    // ========================================================================
    // MLE EVALUATION
    // ========================================================================

    /// @notice Evaluate the multilinear extension of data at a given point
    /// @dev MLE(x) = sum_{w in {0,1}^n} data[w] * prod_{i} (x_i * w_i + (1-x_i) * (1-w_i))
    function evaluateMLEFromData(uint256[] memory data, uint256[] memory point) internal pure returns (uint256) {
        uint256 n = point.length;
        if (data.length > (uint256(1) << n)) revert DataTooLargeForPoint();

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
