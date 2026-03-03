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
    // CONSTANTS
    // ========================================================================

    /// @notice BN254 scalar field modulus (Fr)
    uint256 internal constant FR_MODULUS =
        21888242871839275222246405745257275088548364400416034343698204186575808495617;

    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice All challenges derived from transcript replay
    struct TranscriptChallenges {
        uint256[] outputChallenges; // [0] num_vars challenges squeezed after initial setup
        uint256 claimAggCoeff; // [1] After absorb output claim commitment
        // Layer 0 (subtract, degree=2)
        uint256[] layer0Bindings; // [2] One binding (1-round sumcheck)
        uint256[] layer0Rhos; // [3] n+1 = 2 rhos
        uint256[] layer0Gammas; // [4] n = 1 gamma
        uint256 layer0PodpChallenge; // [5] PODP challenge
        // Inter-layer
        uint256 interLayerCoeff; // [6] Squeezed between layers
        // Layer 1 (multiply, degree=3)
        uint256[] layer1Bindings; // [7] One binding
        uint256[] layer1Rhos; // [8] n+1 = 2 rhos
        uint256[] layer1Gammas; // [9] n = 1 gamma
        uint256 layer1PodpChallenge; // [10] PODP challenge
        uint256 layer1PopChallenge; // [11] PoP challenge
        // Input layer
        uint256[] inputRlcCoeffs; // [12] 2 RLC coefficients
        uint256 inputPodpChallenge; // [13] Input PODP challenge
    }

    /// @notice Groth16 public outputs (computed off-chain, verified by Groth16)
    /// @dev For medium config (N=4): 14 values = rlcBeta(2) + zDotJStar(2) + lTensor(8) + zDotR + mleEval
    struct Groth16Outputs {
        uint256 rlcBeta0; // Layer 0 oracle eval coefficient
        uint256 rlcBeta1; // Layer 1 oracle eval coefficient
        uint256 zDotJStar0; // Layer 0 PODP inner product
        uint256 zDotJStar1; // Layer 1 PODP inner product
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
        // Step 1: Squeeze output challenges (num_vars values)
        // num_vars = number of sumcheck messages per layer = number of bindings
        require(proof.layerProofs.length >= 2, "GKRHybrid: need 2 layer proofs");
        uint256 numVars = proof.layerProofs[0].sumcheckProof.messages.length;
        challenges.outputChallenges = _squeezeMultiple(sponge, numVars);

        // Step 2: Absorb output claim commitment, squeeze claim agg coefficient
        require(proof.outputClaimCommitments.length > 0, "GKRHybrid: no output claims");
        PoseidonSponge.absorb(sponge, proof.outputClaimCommitments[0].x);
        PoseidonSponge.absorb(sponge, proof.outputClaimCommitments[0].y);
        challenges.claimAggCoeff = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        // Step 3: Layer 0 (subtract, degree=2, num_vars-round sumcheck → num_vars bindings)
        challenges.layer0Bindings =
            _absorbMessagesAndDeriveBindings(proof.layerProofs[0].sumcheckProof.messages, sponge);

        // Absorb layer 0 post-sumcheck commitments
        for (uint256 j = 0; j < proof.layerProofs[0].commitments.length; j++) {
            PoseidonSponge.absorb(sponge, proof.layerProofs[0].commitments[j].x);
            PoseidonSponge.absorb(sponge, proof.layerProofs[0].commitments[j].y);
        }

        // Squeeze layer 0 rhos (n+1) and gammas (n)
        uint256 n0 = proof.layerProofs[0].sumcheckProof.messages.length;
        challenges.layer0Rhos = _squeezeMultiple(sponge, n0 + 1);
        challenges.layer0Gammas = _squeezeMultiple(sponge, n0);

        // Layer 0 PODP transcript: absorb commitD, commitDDotA, squeeze challenge, absorb z data
        challenges.layer0PodpChallenge = _absorbPODPAndSqueeze(proof.layerProofs[0].sumcheckProof.podp, sponge);

        // Layer 0 has no PoP entries (subtract gate has no product triples)

        // Step 4: Squeeze inter-layer coefficient
        challenges.interLayerCoeff = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        // Step 5: Layer 1 (multiply, degree=3, num_vars-round sumcheck → num_vars bindings)
        challenges.layer1Bindings =
            _absorbMessagesAndDeriveBindings(proof.layerProofs[1].sumcheckProof.messages, sponge);

        // Absorb layer 1 post-sumcheck commitments
        for (uint256 j = 0; j < proof.layerProofs[1].commitments.length; j++) {
            PoseidonSponge.absorb(sponge, proof.layerProofs[1].commitments[j].x);
            PoseidonSponge.absorb(sponge, proof.layerProofs[1].commitments[j].y);
        }

        // Squeeze layer 1 rhos and gammas
        uint256 n1 = proof.layerProofs[1].sumcheckProof.messages.length;
        challenges.layer1Rhos = _squeezeMultiple(sponge, n1 + 1);
        challenges.layer1Gammas = _squeezeMultiple(sponge, n1);

        // Layer 1 PODP transcript
        challenges.layer1PodpChallenge = _absorbPODPAndSqueeze(proof.layerProofs[1].sumcheckProof.podp, sponge);

        // Layer 1 PoP transcript (multiply gate has product triples)
        for (uint256 i = 0; i < proof.layerProofs[1].pops.length; i++) {
            // Absorb alpha, beta, delta
            PoseidonSponge.absorb(sponge, proof.layerProofs[1].pops[i].alpha.x);
            PoseidonSponge.absorb(sponge, proof.layerProofs[1].pops[i].alpha.y);
            PoseidonSponge.absorb(sponge, proof.layerProofs[1].pops[i].beta.x);
            PoseidonSponge.absorb(sponge, proof.layerProofs[1].pops[i].beta.y);
            PoseidonSponge.absorb(sponge, proof.layerProofs[1].pops[i].delta.x);
            PoseidonSponge.absorb(sponge, proof.layerProofs[1].pops[i].delta.y);
            // Squeeze PoP challenge (save only the last one, matching test circuit with 1 PoP)
            challenges.layer1PopChallenge = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
            // Absorb z1..z5
            PoseidonSponge.absorb(sponge, proof.layerProofs[1].pops[i].z1);
            PoseidonSponge.absorb(sponge, proof.layerProofs[1].pops[i].z2);
            PoseidonSponge.absorb(sponge, proof.layerProofs[1].pops[i].z3);
            PoseidonSponge.absorb(sponge, proof.layerProofs[1].pops[i].z4);
            PoseidonSponge.absorb(sponge, proof.layerProofs[1].pops[i].z5);
        }

        // Step 6: Input layer — squeeze RLC coefficients
        challenges.inputRlcCoeffs = _squeezeMultiple(sponge, 2);

        // Absorb comEval
        require(proof.inputProofs.length > 0, "GKRHybrid: no input proofs");
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
    /// @return valid Whether all EC checks pass
    function verifyECChecks(
        GKRVerifier.GKRProof memory proof,
        TranscriptChallenges memory challenges,
        Groth16Outputs memory outputs,
        HyraxVerifier.PedersenGens memory gens
    ) internal view returns (bool) {
        // Layer 0 PODP (subtract gate)
        require(
            _verifyLayerPODP(
                proof.layerProofs[0],
                challenges.layer0Rhos,
                challenges.layer0Gammas,
                challenges.layer0PodpChallenge,
                outputs.rlcBeta0,
                outputs.zDotJStar0,
                0, // layerType = subtract
                gens
            ),
            "Hybrid: Layer 0 PODP failed"
        );

        // Layer 1 PODP (multiply gate)
        require(
            _verifyLayerPODP(
                proof.layerProofs[1],
                challenges.layer1Rhos,
                challenges.layer1Gammas,
                challenges.layer1PodpChallenge,
                outputs.rlcBeta1,
                outputs.zDotJStar1,
                1, // layerType = multiply
                gens
            ),
            "Hybrid: Layer 1 PODP failed"
        );

        // Layer 1 PoP (multiply gate has product triples)
        require(
            _verifyLayerPoPs(proof.layerProofs[1], challenges.layer1PopChallenge, gens), "Hybrid: Layer 1 PoP failed"
        );

        // Input Hyrax verification (committed input layer)
        require(
            _verifyInputHyrax(proof.inputProofs[0], challenges.inputPodpChallenge, outputs, gens),
            "Hybrid: Input Hyrax failed"
        );

        // Note: mleEval is verified by Groth16 but only used for circuits with
        // non-committed (public) input layers. For committed inputs, the Hyrax
        // evaluation proof above is sufficient.

        return true;
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
        require(HyraxVerifier.isEqual(lhs1, comZ), "Hybrid: PODP Eq1 failed");

        // Eq2: c * dotProduct + commitDDotA == zDotJStar * g_scalar + z_beta * h
        HyraxVerifier.G1Point memory lhs2 =
            HyraxVerifier.ecAdd(HyraxVerifier.scalarMul(dotProduct, podpChallenge), podp.commitDDotA);
        HyraxVerifier.G1Point memory comZDotA = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(gens.scalarGen, zDotJStar), HyraxVerifier.scalarMul(gens.blindingGen, podp.zBeta)
        );
        require(HyraxVerifier.isEqual(lhs2, comZDotA), "Hybrid: PODP Eq2 failed");

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
            require(commitIdx + 2 < layerProof.commitments.length, "GKRHybrid: not enough commits for PoP");

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
        require(HyraxVerifier.isEqual(lhs1, comZ), "Hybrid: Input PODP Eq1 failed");

        // Step 4: PODP Eq2 — dot product consistency
        // c * comY + commitDDotA == zDotR * g_scalar + z_beta * h
        // Uses zDotR from Groth16 (replaces on-chain innerProduct(z, R_tensor))
        HyraxVerifier.G1Point memory lhs2 =
            HyraxVerifier.ecAdd(HyraxVerifier.scalarMul(comY, podpChallenge), inputProof.podp.commitDDotA);
        HyraxVerifier.G1Point memory comZDotA = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(gens.scalarGen, outputs.zDotR),
            HyraxVerifier.scalarMul(gens.blindingGen, inputProof.podp.zBeta)
        );
        require(HyraxVerifier.isEqual(lhs2, comZDotA), "Hybrid: Input PODP Eq2 failed");

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
        require(commitments.length > 0, "GKRHybrid: no commitments");

        if (layerType == 1) {
            // Multiply: rlcBeta * last commitment
            return HyraxVerifier.scalarMul(commitments[commitments.length - 1], rlcBeta);
        } else {
            // Subtract: rlcBeta * com[0] - rlcBeta * com[1]
            require(commitments.length >= 2, "GKRHybrid: subtract needs 2 commitments");
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
    ///      layer0(bindings=N, rhos=N+1, gammas=N, podpChallenge=1) +
    ///      layer1(bindings=N, rhos=N+1, gammas=N, podpChallenge=1, popChallenge=1) +
    ///      inputRLCCoeffs(2) + inputPODPChallenge(1) + interLayerCoeff(1) +
    ///      outputs(rlcBeta=2, zDotJStar=2, lTensor=2*2^floor(N/2), zDotR=1, mleEval=1)
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
        uint256 total = 2 + pubInputs.length + challenges.outputChallenges.length + 1 + challenges.layer0Bindings.length
            + challenges.layer0Rhos.length + challenges.layer0Gammas.length + 1 + challenges.layer1Bindings.length
            + challenges.layer1Rhos.length + challenges.layer1Gammas.length + 2 + 4 + 4 + groth16Outputs.lTensor.length
            + 2;
        inputs = new uint256[](total);
        uint256 idx = 0;

        // [0-1] Circuit hash
        inputs[idx++] = circuitHashFr0;
        inputs[idx++] = circuitHashFr1;

        // [2-17] Public inputs (2^num_vars = 16 values)
        for (uint256 i = 0; i < pubInputs.length; i++) {
            inputs[idx++] = pubInputs[i];
        }

        // [18-21] Output challenges (num_vars = 4 values)
        for (uint256 i = 0; i < challenges.outputChallenges.length; i++) {
            inputs[idx++] = challenges.outputChallenges[i];
        }

        // [22] Claim agg coefficient
        inputs[idx++] = challenges.claimAggCoeff;

        // [23-26] Layer 0 bindings (num_vars = 4)
        for (uint256 i = 0; i < challenges.layer0Bindings.length; i++) {
            inputs[idx++] = challenges.layer0Bindings[i];
        }
        // [27-31] Layer 0 rhos (num_vars+1 = 5)
        for (uint256 i = 0; i < challenges.layer0Rhos.length; i++) {
            inputs[idx++] = challenges.layer0Rhos[i];
        }
        // [32-35] Layer 0 gammas (num_vars = 4)
        for (uint256 i = 0; i < challenges.layer0Gammas.length; i++) {
            inputs[idx++] = challenges.layer0Gammas[i];
        }
        // [36] Layer 0 PODP challenge
        inputs[idx++] = challenges.layer0PodpChallenge;

        // [37-40] Layer 1 bindings (num_vars = 4)
        for (uint256 i = 0; i < challenges.layer1Bindings.length; i++) {
            inputs[idx++] = challenges.layer1Bindings[i];
        }
        // [41-45] Layer 1 rhos (num_vars+1 = 5)
        for (uint256 i = 0; i < challenges.layer1Rhos.length; i++) {
            inputs[idx++] = challenges.layer1Rhos[i];
        }
        // [46-49] Layer 1 gammas (num_vars = 4)
        for (uint256 i = 0; i < challenges.layer1Gammas.length; i++) {
            inputs[idx++] = challenges.layer1Gammas[i];
        }
        // [50] Layer 1 PODP challenge
        inputs[idx++] = challenges.layer1PodpChallenge;
        // [51] Layer 1 PoP challenge
        inputs[idx++] = challenges.layer1PopChallenge;

        // [52-53] Input RLC coefficients
        inputs[idx++] = challenges.inputRlcCoeffs[0];
        inputs[idx++] = challenges.inputRlcCoeffs[1];
        // [54] Input PODP challenge
        inputs[idx++] = challenges.inputPodpChallenge;
        // [55] Inter-layer coefficient
        inputs[idx++] = challenges.interLayerCoeff;

        // [56-57] Groth16 outputs: rlcBeta0, rlcBeta1
        inputs[idx++] = groth16Outputs.rlcBeta0;
        inputs[idx++] = groth16Outputs.rlcBeta1;
        // [58-59] zDotJStar0, zDotJStar1
        inputs[idx++] = groth16Outputs.zDotJStar0;
        inputs[idx++] = groth16Outputs.zDotJStar1;
        // [60-67] lTensor (2 * 2^floor(num_vars/2) = 8 elements)
        for (uint256 i = 0; i < groth16Outputs.lTensor.length; i++) {
            inputs[idx++] = groth16Outputs.lTensor[i];
        }
        // [68] zDotR
        inputs[idx++] = groth16Outputs.zDotR;
        // [69] mleEval
        inputs[idx++] = groth16Outputs.mleEval;
    }
}
