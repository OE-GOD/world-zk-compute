// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {PoseidonSponge} from "./PoseidonSponge.sol";
import {HyraxVerifier} from "./HyraxVerifier.sol";
import {GKRVerifier} from "./GKRVerifier.sol";

/// @title GKRHybridVerifier
/// @notice Hybrid GKR verifier that combines on-chain Poseidon transcript replay
///         with off-chain Groth16 proof for Fr arithmetic.
/// @dev Architecture:
///      1. Replay Poseidon transcript on-chain to derive Fiat-Shamir challenges
///      2. Verify Groth16 proof with challenges as public inputs (Fr arithmetic)
///      3. Use Groth16 outputs (Fr scalars) in on-chain EC equation checks
///
///      This eliminates expensive on-chain Fr computations (j_star, beta, inner products,
///      tensor, MLE eval) while keeping security-critical transcript replay and EC checks.
library GKRHybridVerifier {
    // ========================================================================
    // ERRORS
    // ========================================================================

    error NeedAtLeastOneLayerProof();
    error NoOutputClaims();
    error NoInputProofs();
    error LayerPODPFailed();
    error LayerPoPFailed();
    error InputHyraxFailed();
    error MleEvalMismatch();
    error PublicInputClaimMismatch();
    error PODPEq1Failed();
    error PODPEq2Failed();
    error NotEnoughCommitsForPoP();
    error InputPODPEq1Failed();
    error InputPODPEq2Failed();
    error NoCommitments();
    error SubtractNeedsTwoCommitments();

    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @notice BN254 scalar field modulus (Fr)
    uint256 internal constant FR_MODULUS =
        21888242871839275222246405745257275088548364400416034343698204186575808495617;

    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice Per-layer challenge container
    struct LayerChallenges {
        uint256[] bindings;
        uint256[] rhos;
        uint256[] gammas;
        uint256 podpChallenge;
        uint256 popChallenge; // 0 if layer has no PoPs
    }

    /// @notice All challenges derived from transcript replay
    struct TranscriptChallenges {
        uint256[] outputChallenges; // num_vars challenges squeezed after initial setup
        uint256 claimAggCoeff; // After absorb output claim commitment
        LayerChallenges[] layers; // One per computation layer
        uint256[] interLayerCoeffs; // N-1 values (between layers)
        uint256[] inputRlcCoeffs; // 2 RLC coefficients
        uint256 inputPodpChallenge; // Input PODP challenge
        uint256[] mleEvalPoint; // MLE evaluation point (from GKR claim propagation to input layer)
        uint256[] inputClaimPoint; // Input layer claim point (for Hyrax L/R tensor split)
    }

    /// @notice Groth16 public outputs (computed off-chain, verified by Groth16)
    struct Groth16Outputs {
        uint256[] rlcBeta; // One per computation layer
        uint256[] zDotJStar; // One per computation layer
        uint256[] lTensor; // Input Hyrax L-tensor: 2 * 2^floor(num_vars/2) elements
        uint256 zDotR; // Input PODP inner product
        uint256 mleEval; // Public input MLE evaluation
    }

    // ========================================================================
    // TRANSCRIPT REPLAY
    // ========================================================================

    /// @notice Replay the full Poseidon transcript and collect all Fiat-Shamir challenges
    /// @dev Performs ONLY absorb/squeeze operations (no Fr computation, no EC checks).
    ///      This keeps transcript state in sync for correct challenge derivation.
    /// @param proof The decoded GKR proof
    /// @param sponge Pre-initialized transcript (after _setupTranscript)
    /// @return challenges All derived challenge values
    function replayTranscriptAndCollectChallenges(
        GKRVerifier.GKRProof memory proof,
        PoseidonSponge.Sponge memory sponge
    ) internal pure returns (TranscriptChallenges memory challenges) {
        uint256 numLayers = proof.layerProofs.length;
        if (numLayers < 1) revert NeedAtLeastOneLayerProof();

        // Step 1: Squeeze output challenges (num_vars values)
        uint256 numVars = proof.layerProofs[0].sumcheckProof.messages.length;
        challenges.outputChallenges = _squeezeMultiple(sponge, numVars);

        // Step 2: Absorb output claim commitment, squeeze claim agg coefficient
        if (proof.outputClaimCommitments.length == 0) revert NoOutputClaims();
        PoseidonSponge.absorb(sponge, proof.outputClaimCommitments[0].x);
        PoseidonSponge.absorb(sponge, proof.outputClaimCommitments[0].y);
        challenges.claimAggCoeff = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        // Step 3: Loop over computation layers
        challenges.layers = new LayerChallenges[](numLayers);
        challenges.interLayerCoeffs = new uint256[](numLayers > 1 ? numLayers - 1 : 0);

        for (uint256 i = 0; i < numLayers; i++) {
            // Absorb sumcheck messages → derive bindings
            challenges.layers[i].bindings =
                _absorbMessagesAndDeriveBindings(proof.layerProofs[i].sumcheckProof.messages, sponge);

            // Absorb post-sumcheck commitments
            for (uint256 j = 0; j < proof.layerProofs[i].commitments.length; j++) {
                PoseidonSponge.absorb(sponge, proof.layerProofs[i].commitments[j].x);
                PoseidonSponge.absorb(sponge, proof.layerProofs[i].commitments[j].y);
            }

            // Squeeze rhos (n+1) and gammas (n)
            uint256 n = proof.layerProofs[i].sumcheckProof.messages.length;
            challenges.layers[i].rhos = _squeezeMultiple(sponge, n + 1);
            challenges.layers[i].gammas = _squeezeMultiple(sponge, n);

            // PODP transcript
            challenges.layers[i].podpChallenge = _absorbPODPAndSqueeze(proof.layerProofs[i].sumcheckProof.podp, sponge);

            // PoP transcript (if layer has product triples)
            for (uint256 p = 0; p < proof.layerProofs[i].pops.length; p++) {
                PoseidonSponge.absorb(sponge, proof.layerProofs[i].pops[p].alpha.x);
                PoseidonSponge.absorb(sponge, proof.layerProofs[i].pops[p].alpha.y);
                PoseidonSponge.absorb(sponge, proof.layerProofs[i].pops[p].beta.x);
                PoseidonSponge.absorb(sponge, proof.layerProofs[i].pops[p].beta.y);
                PoseidonSponge.absorb(sponge, proof.layerProofs[i].pops[p].delta.x);
                PoseidonSponge.absorb(sponge, proof.layerProofs[i].pops[p].delta.y);
                challenges.layers[i].popChallenge = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
                PoseidonSponge.absorb(sponge, proof.layerProofs[i].pops[p].z1);
                PoseidonSponge.absorb(sponge, proof.layerProofs[i].pops[p].z2);
                PoseidonSponge.absorb(sponge, proof.layerProofs[i].pops[p].z3);
                PoseidonSponge.absorb(sponge, proof.layerProofs[i].pops[p].z4);
                PoseidonSponge.absorb(sponge, proof.layerProofs[i].pops[p].z5);
            }

            // Inter-layer coefficient (except after last layer)
            if (i < numLayers - 1) {
                challenges.interLayerCoeffs[i] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
            }
        }

        // MLE eval point: the public input layer is fed by the first compute layer (closest
        // to output), so its claim point = layers[0].bindings. For a 2-layer circuit:
        // output → layer[0](diff=product-expected) → layer[1](product=a*b) → committed input
        // The public input 'expected' appears in layer[0], so its claim uses layer[0].bindings.
        challenges.mleEvalPoint = challenges.layers[0].bindings;
        // Input claim point: the committed input layer is fed by the last compute layer,
        // so its PODP evaluation point = layers[numLayers-1].bindings.
        challenges.inputClaimPoint = challenges.layers[numLayers - 1].bindings;

        // Step 4: Input layer — squeeze RLC coefficients
        challenges.inputRlcCoeffs = _squeezeMultiple(sponge, 2);

        // Absorb comEval
        if (proof.inputProofs.length == 0) revert NoInputProofs();
        PoseidonSponge.absorb(sponge, proof.inputProofs[0].comEval.x);
        PoseidonSponge.absorb(sponge, proof.inputProofs[0].comEval.y);

        // Input PODP transcript
        challenges.inputPodpChallenge = _absorbPODPAndSqueeze(proof.inputProofs[0].podp, sponge);
    }

    // ========================================================================
    // EC EQUATION CHECKS
    // ========================================================================

    /// @notice Verify all EC equations using Groth16 outputs as Fr scalars
    /// @param proof The decoded GKR proof
    /// @param challenges Transcript-derived challenges
    /// @param outputs Groth16 public outputs
    /// @param gens Pedersen generators
    /// @param circuit Circuit description (for layer type mapping)
    /// @return valid Whether all EC checks pass
    function verifyECChecks(
        GKRVerifier.GKRProof memory proof,
        TranscriptChallenges memory challenges,
        Groth16Outputs memory outputs,
        HyraxVerifier.PedersenGens memory gens,
        GKRVerifier.CircuitDescription memory circuit,
        uint256[] memory pubInputs
    ) internal view returns (bool) {
        uint256 numComputeLayers = proof.layerProofs.length;

        for (uint256 i = 0; i < numComputeLayers; i++) {
            // Map proof index to circuit layer type
            // proof.layerProofs[0] = output layer = circuit.layerTypes[numLayers-1]
            uint8 layerType = circuit.layerTypes[circuit.numLayers - 1 - i];

            if (!_verifyLayerPODP(
                    proof.layerProofs[i],
                    challenges.layers[i].rhos,
                    challenges.layers[i].gammas,
                    challenges.layers[i].podpChallenge,
                    outputs.rlcBeta[i],
                    outputs.zDotJStar[i],
                    layerType,
                    gens
                )) revert LayerPODPFailed();

            // PoP verification (if layer has product triples)
            if (proof.layerProofs[i].pops.length > 0) {
                if (!_verifyLayerPoPs(proof.layerProofs[i], challenges.layers[i].popChallenge, gens)) {
                    revert LayerPoPFailed();
                }
            }
        }

        // Input Hyrax verification (committed input layer)
        if (proof.inputProofs.length > 0) {
            if (!_verifyInputHyrax(proof.inputProofs[0], challenges.inputPodpChallenge, outputs, gens)) {
                revert InputHyraxFailed();
            }
        }

        // Public input claim verification (for non-committed input layers)
        if (_hasNonCommittedInputLayer(circuit) && pubInputs.length > 0) {
            // Defense-in-depth: verify on-chain MLE evaluation matches Groth16's mleEval.
            // The gnark tensor product reverses bit ordering vs Solidity's evaluateMLEFromData,
            // so we reverse the mleEvalPoint array before evaluating.
            uint256 bLen = challenges.mleEvalPoint.length;
            uint256[] memory revPoint = new uint256[](bLen);
            for (uint256 i = 0; i < bLen; i++) {
                revPoint[i] = challenges.mleEvalPoint[bLen - 1 - i];
            }
            uint256 onChainMle = GKRVerifier.evaluateMLEFromData(pubInputs, revPoint);
            if (outputs.mleEval != onChainMle) revert MleEvalMismatch();

            // Verify the MLE evaluation matches the final claim commitment
            HyraxVerifier.G1Point memory claimCom = proof.layerProofs[numComputeLayers - 1].commitments[0];
            HyraxVerifier.G1Point memory expectedCom = HyraxVerifier.scalarMul(gens.scalarGen, outputs.mleEval);
            if (!HyraxVerifier.isEqual(expectedCom, claimCom)) revert PublicInputClaimMismatch();
        }

        return true;
    }

    /// @notice Check if the circuit has any non-committed input layer
    function _hasNonCommittedInputLayer(GKRVerifier.CircuitDescription memory circuit) private pure returns (bool) {
        for (uint256 i = 0; i < circuit.numLayers; i++) {
            if (circuit.layerTypes[i] == 3 && !circuit.isCommitted[i]) {
                return true;
            }
        }
        return false;
    }

    // ========================================================================
    // LAYER PODP VERIFICATION
    // ========================================================================

    /// @notice Verify a layer's committed sumcheck PODP using Groth16 outputs
    /// @dev Replaces CommittedSumcheckVerifier.verify() — uses rlcBeta and zDotJStar
    ///      from Groth16 instead of computing them on-chain.
    function _verifyLayerPODP(
        GKRVerifier.CommittedLayerProof memory layerProof,
        uint256[] memory rhos,
        uint256[] memory gammas,
        uint256 podpChallenge,
        uint256 rlcBeta,
        uint256 zDotJStar,
        uint8 layerType,
        HyraxVerifier.PedersenGens memory gens
    ) private view returns (bool) {
        // Step 1: Compute alpha = MSM(messages, gammas) and oracleEval
        HyraxVerifier.G1Point memory alpha = HyraxVerifier.multiScalarMul(layerProof.sumcheckProof.messages, gammas);
        HyraxVerifier.G1Point memory oracleEval = _computeOracleEval(layerProof.commitments, layerType, rlcBeta);

        // Step 2: Compute dotProduct = sum * rhos[0] + oracleEval * (-rhos[n])
        uint256 n = layerProof.sumcheckProof.messages.length;
        HyraxVerifier.G1Point memory dotProduct = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(layerProof.sumcheckProof.sum, rhos[0]),
            HyraxVerifier.scalarMul(oracleEval, FR_MODULUS - rhos[n])
        );

        // Step 3: PODP checks (split to avoid stack-too-deep)
        return _verifyLayerPODPChecks(layerProof.sumcheckProof.podp, alpha, dotProduct, podpChallenge, zDotJStar, gens);
    }

    /// @notice PODP equation checks for a layer (extracted to avoid stack-too-deep)
    function _verifyLayerPODPChecks(
        HyraxVerifier.PODPProof memory podp,
        HyraxVerifier.G1Point memory alpha,
        HyraxVerifier.G1Point memory dotProduct,
        uint256 podpChallenge,
        uint256 zDotJStar,
        HyraxVerifier.PedersenGens memory gens
    ) private view returns (bool) {
        // Eq1: c * alpha + commitD == MSM(gens, z_vector) + z_delta * h
        HyraxVerifier.G1Point memory lhs1 =
            HyraxVerifier.ecAdd(HyraxVerifier.scalarMul(alpha, podpChallenge), podp.commitD);
        HyraxVerifier.G1Point memory comZ = HyraxVerifier.ecAdd(
            _msmWithTruncatedGens(gens.messageGens, podp.zVector),
            HyraxVerifier.scalarMul(gens.blindingGen, podp.zDelta)
        );
        if (!HyraxVerifier.isEqual(lhs1, comZ)) revert PODPEq1Failed();

        // Eq2: c * dotProduct + commitDDotA == zDotJStar * g_scalar + z_beta * h
        HyraxVerifier.G1Point memory lhs2 =
            HyraxVerifier.ecAdd(HyraxVerifier.scalarMul(dotProduct, podpChallenge), podp.commitDDotA);
        HyraxVerifier.G1Point memory comZDotA = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(gens.scalarGen, zDotJStar), HyraxVerifier.scalarMul(gens.blindingGen, podp.zBeta)
        );
        if (!HyraxVerifier.isEqual(lhs2, comZDotA)) revert PODPEq2Failed();

        return true;
    }

    /// @notice Verify PoP entries for a layer (unchanged from GKRVerifier)
    /// @dev Uses the pre-derived popChallenge instead of squeezing from transcript
    function _verifyLayerPoPs(
        GKRVerifier.CommittedLayerProof memory layerProof,
        uint256 popChallenge,
        HyraxVerifier.PedersenGens memory gens
    ) private view returns (bool) {
        if (layerProof.pops.length == 0) return true;

        uint256 commitIdx = 0;
        for (uint256 i = 0; i < layerProof.pops.length; i++) {
            if (commitIdx + 2 >= layerProof.commitments.length) revert NotEnoughCommitsForPoP();

            if (!_popCheckEquations(
                    layerProof.pops[i],
                    layerProof.commitments[commitIdx],
                    layerProof.commitments[commitIdx + 1],
                    layerProof.commitments[commitIdx + 2],
                    gens,
                    popChallenge
                )) return false;

            commitIdx += 1;
        }
        return true;
    }

    // ========================================================================
    // INPUT HYRAX VERIFICATION
    // ========================================================================

    /// @notice Verify input Hyrax evaluation using Groth16 outputs
    /// @dev Uses lTensor and zDotR from Groth16 instead of computing on-chain
    function _verifyInputHyrax(
        HyraxVerifier.EvalProof memory inputProof,
        uint256 podpChallenge,
        Groth16Outputs memory outputs,
        HyraxVerifier.PedersenGens memory gens
    ) private view returns (bool) {
        // Step 1: Compute com_x = MSM(rows, lTensor) using Groth16 lTensor values
        // lTensor has 2 * 2^floor(num_vars/2) elements (matching commitmentRows count)
        HyraxVerifier.G1Point memory comX = HyraxVerifier.multiScalarMul(inputProof.commitmentRows, outputs.lTensor);

        // Step 2: com_y = commitmentToEvaluation
        HyraxVerifier.G1Point memory comY = inputProof.comEval;

        // Step 3: PODP Eq1 — vector commitment consistency
        // c * comX + commitD == MSM(gens, z_vector) + z_delta * h
        HyraxVerifier.G1Point memory lhs1 =
            HyraxVerifier.ecAdd(HyraxVerifier.scalarMul(comX, podpChallenge), inputProof.podp.commitD);
        HyraxVerifier.G1Point memory comZ = HyraxVerifier.ecAdd(
            _msmWithTruncatedGens(gens.messageGens, inputProof.podp.zVector),
            HyraxVerifier.scalarMul(gens.blindingGen, inputProof.podp.zDelta)
        );
        if (!HyraxVerifier.isEqual(lhs1, comZ)) revert InputPODPEq1Failed();

        // Step 4: PODP Eq2 — dot product consistency
        // c * comY + commitDDotA == zDotR * g_scalar + z_beta * h
        // Uses zDotR from Groth16 (replaces on-chain innerProduct(z, R_tensor))
        HyraxVerifier.G1Point memory lhs2 =
            HyraxVerifier.ecAdd(HyraxVerifier.scalarMul(comY, podpChallenge), inputProof.podp.commitDDotA);
        HyraxVerifier.G1Point memory comZDotA = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(gens.scalarGen, outputs.zDotR),
            HyraxVerifier.scalarMul(gens.blindingGen, inputProof.podp.zBeta)
        );
        if (!HyraxVerifier.isEqual(lhs2, comZDotA)) revert InputPODPEq2Failed();

        return true;
    }

    // ========================================================================
    // HELPERS (duplicated from GKRVerifier to avoid visibility changes)
    // ========================================================================

    /// @notice Absorb sumcheck messages and derive bindings
    function _absorbMessagesAndDeriveBindings(
        HyraxVerifier.G1Point[] memory messages,
        PoseidonSponge.Sponge memory sponge
    ) private pure returns (uint256[] memory bindings) {
        uint256 n = messages.length;
        bindings = new uint256[](n);

        if (n > 0) {
            PoseidonSponge.absorb(sponge, messages[0].x);
            PoseidonSponge.absorb(sponge, messages[0].y);
        }

        for (uint256 i = 1; i < n; i++) {
            bindings[i - 1] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
            PoseidonSponge.absorb(sponge, messages[i].x);
            PoseidonSponge.absorb(sponge, messages[i].y);
        }

        if (n > 0) {
            bindings[n - 1] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
        }
    }

    /// @notice Absorb PODP data and squeeze challenge
    function _absorbPODPAndSqueeze(HyraxVerifier.PODPProof memory podp, PoseidonSponge.Sponge memory sponge)
        private
        pure
        returns (uint256 challenge)
    {
        PoseidonSponge.absorb(sponge, podp.commitD.x);
        PoseidonSponge.absorb(sponge, podp.commitD.y);
        PoseidonSponge.absorb(sponge, podp.commitDDotA.x);
        PoseidonSponge.absorb(sponge, podp.commitDDotA.y);
        challenge = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        for (uint256 i = 0; i < podp.zVector.length; i++) {
            PoseidonSponge.absorb(sponge, podp.zVector[i]);
        }
        PoseidonSponge.absorb(sponge, podp.zDelta);
        PoseidonSponge.absorb(sponge, podp.zBeta);
    }

    /// @notice Squeeze multiple challenge values
    function _squeezeMultiple(PoseidonSponge.Sponge memory sponge, uint256 count)
        private
        pure
        returns (uint256[] memory values)
    {
        values = new uint256[](count);
        for (uint256 i = 0; i < count; i++) {
            values[i] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
        }
    }

    /// @notice Compute oracle eval from commitments (duplicated from GKRVerifier)
    function _computeOracleEval(HyraxVerifier.G1Point[] memory commitments, uint8 layerType, uint256 rlcBeta)
        private
        view
        returns (HyraxVerifier.G1Point memory)
    {
        if (commitments.length == 0) revert NoCommitments();

        if (layerType == 1) {
            // Multiply: rlcBeta * last commitment
            return HyraxVerifier.scalarMul(commitments[commitments.length - 1], rlcBeta);
        } else {
            // Subtract: rlcBeta * com[0] - rlcBeta * com[1]
            if (commitments.length < 2) revert SubtractNeedsTwoCommitments();
            HyraxVerifier.G1Point memory pos = HyraxVerifier.scalarMul(commitments[0], rlcBeta);
            HyraxVerifier.G1Point memory neg = HyraxVerifier.scalarMul(commitments[1], FR_MODULUS - rlcBeta);
            return HyraxVerifier.ecAdd(pos, neg);
        }
    }

    /// @notice MSM with truncated generators (duplicated from HyraxVerifier)
    function _msmWithTruncatedGens(HyraxVerifier.G1Point[] memory allGens, uint256[] memory scalars)
        private
        view
        returns (HyraxVerifier.G1Point memory)
    {
        uint256 n = scalars.length;
        if (n == allGens.length) {
            return HyraxVerifier.multiScalarMul(allGens, scalars);
        }
        HyraxVerifier.G1Point[] memory truncated = new HyraxVerifier.G1Point[](n);
        for (uint256 i = 0; i < n; i++) {
            truncated[i] = allGens[i];
        }
        return HyraxVerifier.multiScalarMul(truncated, scalars);
    }

    /// @notice PoP equation checks (duplicated from HyraxVerifier, split for stack depth)
    function _popCheckEquations(
        HyraxVerifier.ProofOfProduct memory pop,
        HyraxVerifier.G1Point memory comX,
        HyraxVerifier.G1Point memory comY,
        HyraxVerifier.G1Point memory comZ,
        HyraxVerifier.PedersenGens memory gens,
        uint256 c
    ) private view returns (bool) {
        if (!_popCheck1(pop, comX, gens, c)) return false;
        if (!_popCheck2(pop, comY, gens, c)) return false;
        return _popCheck3(pop, comX, comZ, gens, c);
    }

    function _popCheck1(
        HyraxVerifier.ProofOfProduct memory pop,
        HyraxVerifier.G1Point memory comX,
        HyraxVerifier.PedersenGens memory gens,
        uint256 c
    ) private view returns (bool) {
        HyraxVerifier.G1Point memory lhs = HyraxVerifier.ecAdd(pop.alpha, HyraxVerifier.scalarMul(comX, c));
        HyraxVerifier.G1Point memory rhs = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(gens.scalarGen, pop.z1), HyraxVerifier.scalarMul(gens.blindingGen, pop.z2)
        );
        return HyraxVerifier.isEqual(lhs, rhs);
    }

    function _popCheck2(
        HyraxVerifier.ProofOfProduct memory pop,
        HyraxVerifier.G1Point memory comY,
        HyraxVerifier.PedersenGens memory gens,
        uint256 c
    ) private view returns (bool) {
        HyraxVerifier.G1Point memory lhs = HyraxVerifier.ecAdd(pop.beta, HyraxVerifier.scalarMul(comY, c));
        HyraxVerifier.G1Point memory rhs = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(gens.scalarGen, pop.z3), HyraxVerifier.scalarMul(gens.blindingGen, pop.z4)
        );
        return HyraxVerifier.isEqual(lhs, rhs);
    }

    function _popCheck3(
        HyraxVerifier.ProofOfProduct memory pop,
        HyraxVerifier.G1Point memory comX,
        HyraxVerifier.G1Point memory comZ,
        HyraxVerifier.PedersenGens memory gens,
        uint256 c
    ) private view returns (bool) {
        HyraxVerifier.G1Point memory lhs = HyraxVerifier.ecAdd(pop.delta, HyraxVerifier.scalarMul(comZ, c));
        HyraxVerifier.G1Point memory rhs = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(comX, pop.z3), HyraxVerifier.scalarMul(gens.blindingGen, pop.z5)
        );
        return HyraxVerifier.isEqual(lhs, rhs);
    }

    // ========================================================================
    // PUBLIC INPUTS BUILDER (for Groth16 verifier)
    // ========================================================================

    /// @notice Build the public inputs array for the Groth16 verifier (dynamic size)
    /// @dev Layout matches gnark circuit struct field declaration order:
    ///      circuitHash(2) + pubInputs(2^N) + outputChallenges(N) + claimAggCoeff(1) +
    ///      per-layer(bindings + rhos + gammas + podpChallenge + popChallenge?) +
    ///      inputRLCCoeffs(2) + inputPODPChallenge(1) + interLayerCoeffs(numLayers-1) +
    ///      outputs(rlcBeta(numLayers) + zDotJStar(numLayers) + lTensor + zDotR + mleEval)
    /// @param circuitHashFr0 Circuit hash Fr value 0
    /// @param circuitHashFr1 Circuit hash Fr value 1
    /// @param pubInputs Public input values (2^num_vars elements)
    /// @param challenges Transcript-derived challenges
    /// @param groth16Outputs Groth16 output values
    /// @return inputs Dynamic array for Groth16 verifyProof
    function buildGroth16Inputs(
        uint256 circuitHashFr0,
        uint256 circuitHashFr1,
        uint256[] memory pubInputs,
        TranscriptChallenges memory challenges,
        Groth16Outputs memory groth16Outputs
    ) internal pure returns (uint256[] memory inputs) {
        // Calculate total size dynamically
        uint256 total = 2 + pubInputs.length + challenges.outputChallenges.length + 1;
        for (uint256 i = 0; i < challenges.layers.length; i++) {
            total += challenges.layers[i].bindings.length + challenges.layers[i].rhos.length
            + challenges.layers[i].gammas.length + 1; // +1 for podpChallenge
            if (challenges.layers[i].popChallenge != 0) total += 1;
        }
        total += 2 + 1; // inputRlcCoeffs + inputPodpChallenge
        total += challenges.interLayerCoeffs.length;
        total += challenges.mleEvalPoint.length; // mleEvalPoint(M)
        total += challenges.inputClaimPoint.length; // inputClaimPoint(InputNumVars)
        total += groth16Outputs.rlcBeta.length + groth16Outputs.zDotJStar.length + groth16Outputs.lTensor.length + 2; // +2 for zDotR + mleEval

        inputs = new uint256[](total);
        uint256 idx = 0;

        // Circuit hash
        inputs[idx++] = circuitHashFr0;
        inputs[idx++] = circuitHashFr1;

        // Public inputs
        for (uint256 i = 0; i < pubInputs.length; i++) {
            inputs[idx++] = pubInputs[i];
        }

        // Output challenges
        for (uint256 i = 0; i < challenges.outputChallenges.length; i++) {
            inputs[idx++] = challenges.outputChallenges[i];
        }

        // Claim agg coefficient
        inputs[idx++] = challenges.claimAggCoeff;

        // Per-layer challenges
        for (uint256 layer = 0; layer < challenges.layers.length; layer++) {
            for (uint256 i = 0; i < challenges.layers[layer].bindings.length; i++) {
                inputs[idx++] = challenges.layers[layer].bindings[i];
            }
            for (uint256 i = 0; i < challenges.layers[layer].rhos.length; i++) {
                inputs[idx++] = challenges.layers[layer].rhos[i];
            }
            for (uint256 i = 0; i < challenges.layers[layer].gammas.length; i++) {
                inputs[idx++] = challenges.layers[layer].gammas[i];
            }
            inputs[idx++] = challenges.layers[layer].podpChallenge;
            if (challenges.layers[layer].popChallenge != 0) {
                inputs[idx++] = challenges.layers[layer].popChallenge;
            }
        }

        // Input RLC coefficients
        inputs[idx++] = challenges.inputRlcCoeffs[0];
        inputs[idx++] = challenges.inputRlcCoeffs[1];
        // Input PODP challenge
        inputs[idx++] = challenges.inputPodpChallenge;
        // Inter-layer coefficients
        for (uint256 i = 0; i < challenges.interLayerCoeffs.length; i++) {
            inputs[idx++] = challenges.interLayerCoeffs[i];
        }
        // MLE evaluation point
        for (uint256 i = 0; i < challenges.mleEvalPoint.length; i++) {
            inputs[idx++] = challenges.mleEvalPoint[i];
        }
        // Input claim point (for Hyrax L/R tensor split)
        for (uint256 i = 0; i < challenges.inputClaimPoint.length; i++) {
            inputs[idx++] = challenges.inputClaimPoint[i];
        }

        // Groth16 outputs: rlcBeta (one per layer)
        for (uint256 i = 0; i < groth16Outputs.rlcBeta.length; i++) {
            inputs[idx++] = groth16Outputs.rlcBeta[i];
        }
        // zDotJStar (one per layer)
        for (uint256 i = 0; i < groth16Outputs.zDotJStar.length; i++) {
            inputs[idx++] = groth16Outputs.zDotJStar[i];
        }
        // lTensor
        for (uint256 i = 0; i < groth16Outputs.lTensor.length; i++) {
            inputs[idx++] = groth16Outputs.lTensor[i];
        }
        // zDotR
        inputs[idx++] = groth16Outputs.zDotR;
        // mleEval
        inputs[idx++] = groth16Outputs.mleEval;
    }
}
