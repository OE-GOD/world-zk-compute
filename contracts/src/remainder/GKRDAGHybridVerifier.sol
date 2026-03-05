// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {PoseidonSponge} from "./PoseidonSponge.sol";
import {HyraxVerifier} from "./HyraxVerifier.sol";
import {GKRVerifier} from "./GKRVerifier.sol";
import {GKRDAGVerifier} from "./GKRDAGVerifier.sol";

/// @title GKRDAGHybridVerifier
/// @notice Hybrid verifier for DAG circuits: on-chain Poseidon transcript replay + Groth16 for Fr arithmetic.
/// @dev Follows the same absorb/squeeze sequence as GKRDAGVerifier but only collects challenge values.
///      Groth16 computes: beta, rlcBeta, j_star, zDotJStar, lTensor, zDotR, mleEval.
///      On-chain verifies: EC equation checks (alpha MSM, oracle eval, PODP, PoP, input PODP).
library GKRDAGHybridVerifier {
    // ========================================================================
    // CONSTANTS
    // ========================================================================

    uint256 internal constant FR_MODULUS =
        21888242871839275222246405745257275088548364400416034343698204186575808495617;

    uint256 internal constant FIXED_REF_BASE = 20000;

    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice Per-compute-layer challenges derived from transcript replay
    struct DAGLayerChallenges {
        uint256[] rlcCoeffs;       // numClaims per layer (variable)
        uint256[] bindings;        // numSumcheckRounds per layer
        uint256[] rhos;            // numSumcheckRounds+1
        uint256[] gammas;          // numSumcheckRounds
        uint256 podpChallenge;
        uint256[] popChallenges;   // One per PoP entry (empty if no PoPs)
    }

    /// @notice Per-input-group challenges (for committed input layers)
    struct DAGInputGroupChallenges {
        uint256[] rlcCoeffs;
        uint256 podpChallenge;
        uint256[] lBindings;    // L-half of claim point (row selection)
        uint256[] rBindings;    // R-half of claim point (column selection)
    }

    /// @notice All challenges from DAG transcript replay
    struct DAGTranscriptChallenges {
        uint256[] outputChallenges;
        DAGLayerChallenges[] layers;
        DAGInputGroupChallenges[] inputGroups;
        uint256[][] allBindings; // bindings per compute layer (for point resolution in EC checks)
    }

    /// @notice Groth16 outputs for DAG circuit verification
    struct DAGGroth16Outputs {
        uint256[] rlcBeta;          // One per compute layer (88)
        uint256[] zDotJStar;        // One per compute layer (88)
        uint256[] lTensorFlat;      // Flattened across all input groups
        uint256[] lTensorOffsets;   // Start index per input group (length = numInputGroups+1)
        uint256[] zDotR;            // One per input group
        uint256[] mleEval;          // One per public value claim
    }

    // ========================================================================
    // TRANSCRIPT REPLAY
    // ========================================================================

    /// @notice Replay the full DAG Poseidon transcript and collect all Fiat-Shamir challenges.
    /// @dev Follows the exact absorb/squeeze sequence of GKRDAGVerifier.verifyComputeLayers()
    ///      + CommittedSumcheckVerifier.verify() + GKRDAGVerifier.verifyInputLayers().
    function replayDAGTranscript(
        GKRVerifier.GKRProof memory proof,
        GKRDAGVerifier.DAGCircuitDescription memory desc,
        PoseidonSponge.Sponge memory sponge
    ) internal pure returns (DAGTranscriptChallenges memory challenges) {
        uint256 numLayers = desc.numComputeLayers;

        // Step 1: Squeeze output challenges (numVars = first layer's message count)
        uint256 numVars = proof.layerProofs[0].sumcheckProof.messages.length;
        challenges.outputChallenges = new uint256[](numVars);
        for (uint256 i = 0; i < numVars; i++) {
            challenges.outputChallenges[i] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
        }

        // Absorb output claim commitment
        PoseidonSponge.absorb(sponge, proof.outputClaimCommitments[0].x);
        PoseidonSponge.absorb(sponge, proof.outputClaimCommitments[0].y);

        // Step 2: Process each compute layer
        challenges.layers = new DAGLayerChallenges[](numLayers);
        challenges.allBindings = new uint256[][](numLayers);

        for (uint256 i = 0; i < numLayers; i++) {
            challenges.layers[i] = _replayOneLayer(i, proof.layerProofs[i], desc, sponge);
            challenges.allBindings[i] = challenges.layers[i].bindings;
        }

        // Step 3: Process input layers
        challenges.inputGroups = _replayInputLayers(proof, desc, challenges.allBindings, sponge);
    }

    /// @notice Replay one compute layer's transcript and collect challenges.
    function _replayOneLayer(
        uint256 layerIdx,
        GKRVerifier.CommittedLayerProof memory lp,
        GKRDAGVerifier.DAGCircuitDescription memory desc,
        PoseidonSponge.Sponge memory sponge
    ) private pure returns (DAGLayerChallenges memory layerCh) {
        // Count claims for this layer
        uint256 numClaims = _countClaimsFor(layerIdx, desc);
        if (layerIdx == 0) numClaims++; // Output claim targets layer 0

        // Squeeze RLC coefficients
        layerCh.rlcCoeffs = _squeezeMultiple(sponge, numClaims);

        // Absorb sumcheck messages → derive bindings
        layerCh.bindings = _absorbMessagesAndDeriveBindings(lp.sumcheckProof.messages, sponge);

        // Absorb post-sumcheck commitments
        for (uint256 j = 0; j < lp.commitments.length; j++) {
            PoseidonSponge.absorb(sponge, lp.commitments[j].x);
            PoseidonSponge.absorb(sponge, lp.commitments[j].y);
        }

        // Squeeze rhos (n+1) and gammas (n) — from CommittedSumcheckVerifier.verify
        uint256 n = lp.sumcheckProof.messages.length;
        layerCh.rhos = _squeezeMultiple(sponge, n + 1);
        layerCh.gammas = _squeezeMultiple(sponge, n);

        // PODP transcript
        layerCh.podpChallenge = _absorbPODPAndSqueeze(lp.sumcheckProof.podp, sponge);

        // PoP transcript (one challenge per PoP entry)
        layerCh.popChallenges = new uint256[](lp.pops.length);
        for (uint256 p = 0; p < lp.pops.length; p++) {
            PoseidonSponge.absorb(sponge, lp.pops[p].alpha.x);
            PoseidonSponge.absorb(sponge, lp.pops[p].alpha.y);
            PoseidonSponge.absorb(sponge, lp.pops[p].beta.x);
            PoseidonSponge.absorb(sponge, lp.pops[p].beta.y);
            PoseidonSponge.absorb(sponge, lp.pops[p].delta.x);
            PoseidonSponge.absorb(sponge, lp.pops[p].delta.y);
            layerCh.popChallenges[p] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
            PoseidonSponge.absorb(sponge, lp.pops[p].z1);
            PoseidonSponge.absorb(sponge, lp.pops[p].z2);
            PoseidonSponge.absorb(sponge, lp.pops[p].z3);
            PoseidonSponge.absorb(sponge, lp.pops[p].z4);
            PoseidonSponge.absorb(sponge, lp.pops[p].z5);
        }
    }

    /// @notice Replay input layer transcript and collect per-group challenges.
    function _replayInputLayers(
        GKRVerifier.GKRProof memory proof,
        GKRDAGVerifier.DAGCircuitDescription memory desc,
        uint256[][] memory allBindings,
        PoseidonSponge.Sponge memory sponge
    ) private pure returns (DAGInputGroupChallenges[] memory allGroupChallenges) {
        // Count total input groups across all committed input layers
        uint256 totalGroups = 0;
        uint256 dagInputIdx = 0;

        // First pass: count groups
        for (uint256 inputIdx = 0; inputIdx < desc.numInputLayers; inputIdx++) {
            if (desc.inputIsCommitted[inputIdx]) {
                uint256 targetLayer = desc.numComputeLayers + inputIdx;
                uint256 numClaims = _countClaimsFor(targetLayer, desc);
                // numGroups == numClaims (prover creates one group per claim)
                totalGroups += numClaims;
                dagInputIdx++;
            }
        }

        allGroupChallenges = new DAGInputGroupChallenges[](totalGroups);
        uint256 groupIdx = 0;
        dagInputIdx = 0;

        // Second pass: replay transcript for each input layer
        for (uint256 inputIdx = 0; inputIdx < desc.numInputLayers; inputIdx++) {
            uint256 targetLayer = desc.numComputeLayers + inputIdx;

            if (desc.inputIsCommitted[inputIdx]) {
                // Collect and sort claim points (must match GKRDAGVerifier's order)
                uint256 numClaims = _countClaimsFor(targetLayer, desc);
                uint256[][] memory claimPoints = _collectClaimPoints(targetLayer, desc, allBindings);

                // Sort claims lexicographically and group by R-half
                uint256[] memory sortedIndices = _sortClaimIndices(claimPoints);
                uint256 numRows = proof.inputProofs[dagInputIdx].commitmentRows.length;
                uint256 n = claimPoints[0].length;
                uint256 lHalfLen = _log2(numRows > 0 ? numRows : 1);
                uint256 logNCols = n - lHalfLen;

                uint256[][] memory groups = _groupClaimsByRHalf(claimPoints, sortedIndices, logNCols);

                // Replay per-group transcript
                for (uint256 g = 0; g < groups.length; g++) {
                    uint256 groupSize = groups[g].length;

                    // Squeeze RLC coefficients for this group
                    allGroupChallenges[groupIdx].rlcCoeffs = _squeezeMultiple(sponge, groupSize);

                    // Absorb comEval
                    // The dagInputProofs aren't available in GKRProof, but the comEval is stored
                    // in the proof in a specific order. We need to access it via the DAGInputLayerProof.
                    // Since we're working with the standard GKR proof struct for transcript replay,
                    // we need the caller to pass DAG input proofs separately.
                    // For now, this is handled by the outer function.

                    groupIdx++;
                }

                dagInputIdx++;
            }
            // Public (non-committed) input layers don't have transcript elements
        }

    }

    /// @notice Full transcript replay including input layer PODP/comEval absorptions.
    /// @dev This is the main entry point that has access to DAGInputLayerProofs.
    function replayDAGTranscriptFull(
        GKRVerifier.GKRProof memory proof,
        GKRDAGVerifier.DAGCircuitDescription memory desc,
        GKRDAGVerifier.DAGInputLayerProof[] memory dagInputProofs,
        PoseidonSponge.Sponge memory sponge
    ) internal pure returns (DAGTranscriptChallenges memory challenges) {
        uint256 numLayers = desc.numComputeLayers;

        // Step 1: Squeeze output challenges
        uint256 numVars = proof.layerProofs[0].sumcheckProof.messages.length;
        challenges.outputChallenges = new uint256[](numVars);
        for (uint256 i = 0; i < numVars; i++) {
            challenges.outputChallenges[i] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
        }

        // Absorb output claim commitment
        PoseidonSponge.absorb(sponge, proof.outputClaimCommitments[0].x);
        PoseidonSponge.absorb(sponge, proof.outputClaimCommitments[0].y);

        // Step 2: Process each compute layer
        challenges.layers = new DAGLayerChallenges[](numLayers);
        challenges.allBindings = new uint256[][](numLayers);

        for (uint256 i = 0; i < numLayers; i++) {
            challenges.layers[i] = _replayOneLayer(i, proof.layerProofs[i], desc, sponge);
            challenges.allBindings[i] = challenges.layers[i].bindings;
        }

        // Step 3: Replay input layers with full DAGInputLayerProof access
        challenges.inputGroups = _replayInputLayersFull(desc, challenges.allBindings, dagInputProofs, sponge);
    }

    /// @notice Context for input layer replay (avoids stack-too-deep).
    struct InputReplayCtx {
        GKRDAGVerifier.DAGCircuitDescription desc;
        uint256[][] allBindings;
        GKRDAGVerifier.DAGInputLayerProof[] dagInputProofs;
    }

    /// @notice Replay input layer transcript with DAGInputLayerProof access.
    function _replayInputLayersFull(
        GKRDAGVerifier.DAGCircuitDescription memory desc,
        uint256[][] memory allBindings,
        GKRDAGVerifier.DAGInputLayerProof[] memory dagInputProofs,
        PoseidonSponge.Sponge memory sponge
    ) private pure returns (DAGInputGroupChallenges[] memory allGroupChallenges) {
        // Count total groups
        uint256 totalGroups = _countTotalInputGroups(desc);

        allGroupChallenges = new DAGInputGroupChallenges[](totalGroups);

        InputReplayCtx memory ctx;
        ctx.desc = desc;
        ctx.allBindings = allBindings;
        ctx.dagInputProofs = dagInputProofs;

        uint256 groupIdx = 0;
        uint256 dagInputIdx = 0;

        for (uint256 inputIdx = 0; inputIdx < desc.numInputLayers; inputIdx++) {
            if (!desc.inputIsCommitted[inputIdx]) continue;

            uint256 numProcessed = _replayOneInputLayer(
                ctx, inputIdx, dagInputIdx, groupIdx, allGroupChallenges, sponge
            );

            groupIdx += numProcessed;
            dagInputIdx++;
        }
    }

    /// @notice Count total input groups across all committed input layers.
    function _countTotalInputGroups(GKRDAGVerifier.DAGCircuitDescription memory desc)
        private
        pure
        returns (uint256 totalGroups)
    {
        for (uint256 inputIdx = 0; inputIdx < desc.numInputLayers; inputIdx++) {
            if (desc.inputIsCommitted[inputIdx]) {
                uint256 targetLayer = desc.numComputeLayers + inputIdx;
                totalGroups += _countClaimsFor(targetLayer, desc);
            }
        }
    }

    /// @notice Replay transcript for one committed input layer.
    function _replayOneInputLayer(
        InputReplayCtx memory ctx,
        uint256 inputIdx,
        uint256 dagInputIdx,
        uint256 startGroupIdx,
        DAGInputGroupChallenges[] memory allGroupChallenges,
        PoseidonSponge.Sponge memory sponge
    ) private pure returns (uint256 numGroups) {
        uint256 targetLayer = ctx.desc.numComputeLayers + inputIdx;
        uint256[][] memory claimPoints = _collectClaimPoints(targetLayer, ctx.desc, ctx.allBindings);

        uint256[][] memory groups = _sortAndGroupClaims(claimPoints, ctx.dagInputProofs[dagInputIdx]);
        numGroups = groups.length;

        for (uint256 g = 0; g < numGroups; g++) {
            allGroupChallenges[startGroupIdx + g].rlcCoeffs = _squeezeMultiple(sponge, groups[g].length);

            PoseidonSponge.absorb(sponge, ctx.dagInputProofs[dagInputIdx].comEvals[g].x);
            PoseidonSponge.absorb(sponge, ctx.dagInputProofs[dagInputIdx].comEvals[g].y);

            allGroupChallenges[startGroupIdx + g].podpChallenge =
                _absorbPODPAndSqueeze(ctx.dagInputProofs[dagInputIdx].podps[g], sponge);
        }

        // Extract L/R bindings per group (separate pass to avoid stack depth issues)
        _extractGroupBindings(claimPoints, groups, ctx.dagInputProofs[dagInputIdx], allGroupChallenges, startGroupIdx);
    }

    /// @notice Extract L-half and R-half bindings for each input group.
    function _extractGroupBindings(
        uint256[][] memory claimPoints,
        uint256[][] memory groups,
        GKRDAGVerifier.DAGInputLayerProof memory dagProof,
        DAGInputGroupChallenges[] memory allGroupChallenges,
        uint256 startGroupIdx
    ) private pure {
        uint256 numRows = dagProof.commitmentRows.length;
        uint256 n = claimPoints[0].length;
        uint256 lHalfLen = _log2(numRows > 0 ? numRows : 1);
        uint256 rHalfLen = n - lHalfLen;

        for (uint256 g = 0; g < groups.length; g++) {
            uint256 firstClaim = groups[g][0];
            allGroupChallenges[startGroupIdx + g].lBindings = new uint256[](lHalfLen);
            for (uint256 k = 0; k < lHalfLen; k++) {
                allGroupChallenges[startGroupIdx + g].lBindings[k] = claimPoints[firstClaim][k];
            }
            allGroupChallenges[startGroupIdx + g].rBindings = new uint256[](rHalfLen);
            for (uint256 k = 0; k < rHalfLen; k++) {
                allGroupChallenges[startGroupIdx + g].rBindings[k] = claimPoints[firstClaim][lHalfLen + k];
            }
        }
    }

    /// @notice Sort claims and group by R-half (helper to reduce stack depth).
    function _sortAndGroupClaims(
        uint256[][] memory claimPoints,
        GKRDAGVerifier.DAGInputLayerProof memory dagProof
    ) private pure returns (uint256[][] memory groups) {
        uint256[] memory sortedIndices = _sortClaimIndices(claimPoints);
        uint256 numRows = dagProof.commitmentRows.length;
        uint256 n = claimPoints[0].length;
        uint256 lHalfLen = _log2(numRows > 0 ? numRows : 1);
        uint256 logNCols = n - lHalfLen;
        groups = _groupClaimsByRHalf(claimPoints, sortedIndices, logNCols);
    }

    // ========================================================================
    // EC EQUATION CHECKS
    // ========================================================================

    /// @notice Verify all EC equations using Groth16 outputs as Fr scalars.
    function verifyDAGECChecks(
        GKRVerifier.GKRProof memory proof,
        DAGTranscriptChallenges memory challenges,
        DAGGroth16Outputs memory outputs,
        HyraxVerifier.PedersenGens memory gens,
        GKRDAGVerifier.DAGCircuitDescription memory desc,
        GKRDAGVerifier.DAGInputLayerProof[] memory dagInputProofs,
        GKRDAGVerifier.PublicValueClaim[] memory publicValueClaims,
        uint256[] memory publicInputs
    ) internal view returns (bool) {
        // Verify compute layers
        for (uint256 i = 0; i < desc.numComputeLayers; i++) {
            _verifyComputeLayerEC(
                i, proof.layerProofs[i], challenges.layers[i], outputs.rlcBeta[i],
                outputs.zDotJStar[i], desc, gens
            );
        }

        // Verify input layers
        _verifyInputLayersEC(desc, challenges, outputs, gens, dagInputProofs);

        // Verify public value claims
        _verifyPublicValueClaimsEC(desc, challenges.allBindings, publicValueClaims, outputs, publicInputs, gens, proof);

        return true;
    }

    /// @notice Verify EC equations for one compute layer.
    function _verifyComputeLayerEC(
        uint256 layerIdx,
        GKRVerifier.CommittedLayerProof memory lp,
        DAGLayerChallenges memory layerCh,
        uint256 rlcBeta,
        uint256 zDotJStar,
        GKRDAGVerifier.DAGCircuitDescription memory desc,
        HyraxVerifier.PedersenGens memory gens
    ) private view {
        uint256 n = lp.sumcheckProof.messages.length;
        if (n == 0) return; // Zero-round layer: no sumcheck to verify

        // Compute alpha and oracleEval
        HyraxVerifier.G1Point memory alpha = HyraxVerifier.multiScalarMul(
            lp.sumcheckProof.messages, layerCh.gammas
        );
        HyraxVerifier.G1Point memory oracleEval = _computeOracleEval(
            lp.commitments, layerIdx, rlcBeta, desc
        );

        // Compute dotProduct = sum * rhos[0] + oracleEval * (-rhos[n])
        HyraxVerifier.G1Point memory dotProduct = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(lp.sumcheckProof.sum, layerCh.rhos[0]),
            HyraxVerifier.scalarMul(oracleEval, FR_MODULUS - layerCh.rhos[n])
        );

        // PODP checks (split out to avoid stack-too-deep)
        _verifyLayerPODPChecks(lp.sumcheckProof.podp, alpha, dotProduct, layerCh.podpChallenge, zDotJStar, gens);

        // PoP verification (per-PoP challenges)
        if (lp.pops.length > 0) {
            require(_verifyLayerPoPs(lp, layerCh.popChallenges, gens), "DAGHybrid: PoP failed");
        }
    }

    /// @notice PODP equation checks for a layer (split out from _verifyComputeLayerEC).
    function _verifyLayerPODPChecks(
        HyraxVerifier.PODPProof memory podp,
        HyraxVerifier.G1Point memory alpha,
        HyraxVerifier.G1Point memory dotProduct,
        uint256 podpChallenge,
        uint256 zDotJStar,
        HyraxVerifier.PedersenGens memory gens
    ) private view {
        // Eq1: c * alpha + commitD == MSM(gens, z_vector) + z_delta * h
        HyraxVerifier.G1Point memory lhs1 = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(alpha, podpChallenge), podp.commitD
        );
        HyraxVerifier.G1Point memory comZ = HyraxVerifier.ecAdd(
            _msmWithTruncatedGens(gens.messageGens, podp.zVector),
            HyraxVerifier.scalarMul(gens.blindingGen, podp.zDelta)
        );
        require(HyraxVerifier.isEqual(lhs1, comZ), "DAGHybrid: PODP Eq1 failed");

        // Eq2: c * dotProduct + commitDDotA == zDotJStar * g_scalar + z_beta * h
        HyraxVerifier.G1Point memory lhs2 = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(dotProduct, podpChallenge), podp.commitDDotA
        );
        HyraxVerifier.G1Point memory comZDotA = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(gens.scalarGen, zDotJStar),
            HyraxVerifier.scalarMul(gens.blindingGen, podp.zBeta)
        );
        require(HyraxVerifier.isEqual(lhs2, comZDotA), "DAGHybrid: PODP Eq2 failed");
    }

    /// @dev Context for input layer EC verification (avoids stack-too-deep).
    struct InputLayerECCtx {
        DAGTranscriptChallenges challenges;
        DAGGroth16Outputs outputs;
        HyraxVerifier.PedersenGens gens;
    }

    /// @notice Verify committed input layer EC equations using Groth16 outputs.
    function _verifyInputLayersEC(
        GKRDAGVerifier.DAGCircuitDescription memory desc,
        DAGTranscriptChallenges memory challenges,
        DAGGroth16Outputs memory outputs,
        HyraxVerifier.PedersenGens memory gens,
        GKRDAGVerifier.DAGInputLayerProof[] memory dagInputProofs
    ) private view {
        InputLayerECCtx memory ecCtx;
        ecCtx.challenges = challenges;
        ecCtx.outputs = outputs;
        ecCtx.gens = gens;

        uint256 groupIdx = 0;
        uint256 dagInputIdx = 0;

        for (uint256 inputIdx = 0; inputIdx < desc.numInputLayers; inputIdx++) {
            if (!desc.inputIsCommitted[inputIdx]) continue;

            // Use actual group count from proof data (claims are grouped by R-half,
            // so numGroups <= numClaims when multiple claims share the same R-half)
            uint256 numGroups = dagInputProofs[dagInputIdx].podps.length;

            for (uint256 g = 0; g < numGroups; g++) {
                _verifyOneInputGroupEC(
                    dagInputProofs[dagInputIdx], g, groupIdx, ecCtx
                );
                groupIdx++;
            }

            dagInputIdx++;
        }
    }

    /// @notice Verify one input eval group's PODP equations.
    function _verifyOneInputGroupEC(
        GKRDAGVerifier.DAGInputLayerProof memory dagProof,
        uint256 evalIdx,
        uint256 groupIdx,
        InputLayerECCtx memory ecCtx
    ) private view {
        // Extract full lTensor slice using precomputed offsets
        uint256 ltStart = ecCtx.outputs.lTensorOffsets[groupIdx];
        uint256 ltEnd = ecCtx.outputs.lTensorOffsets[groupIdx + 1];
        uint256 ltLen = ltEnd - ltStart;
        uint256[] memory lTensorFull = new uint256[](ltLen);
        for (uint256 k = 0; k < ltLen; k++) {
            lTensorFull[k] = ecCtx.outputs.lTensorFlat[ltStart + k];
        }

        // Fold per-claim sub-tensors into combined tensor of size numRows
        uint256 numRows = dagProof.commitmentRows.length;
        uint256[] memory lTensor = _foldLTensor(lTensorFull, numRows);

        // comX = MSM(rows, lTensor)
        HyraxVerifier.G1Point memory comX = HyraxVerifier.multiScalarMul(dagProof.commitmentRows, lTensor);

        // comY = comEval
        HyraxVerifier.G1Point memory comY = dagProof.comEvals[evalIdx];

        // Reuse the PODP check pattern
        _verifyInputPODPChecks(
            dagProof.podps[evalIdx], comX, comY,
            ecCtx.challenges.inputGroups[groupIdx].podpChallenge,
            ecCtx.outputs.zDotR[groupIdx], ecCtx.gens
        );
    }

    /// @notice Fold per-claim sub-tensors into combined tensor by summing.
    /// @dev lTensorFull has numClaims * tensorSize entries; result has tensorSize entries.
    function _foldLTensor(uint256[] memory lTensorFull, uint256 tensorSize)
        private
        pure
        returns (uint256[] memory combined)
    {
        if (lTensorFull.length == tensorSize) return lTensorFull; // single claim, no folding
        combined = new uint256[](tensorSize);
        uint256 numClaims = lTensorFull.length / tensorSize;
        for (uint256 ci = 0; ci < numClaims; ci++) {
            for (uint256 j = 0; j < tensorSize; j++) {
                combined[j] = addmod(combined[j], lTensorFull[ci * tensorSize + j], FR_MODULUS);
            }
        }
    }

    /// @notice PODP checks for input layer (split out to avoid stack-too-deep).
    function _verifyInputPODPChecks(
        HyraxVerifier.PODPProof memory podp,
        HyraxVerifier.G1Point memory comX,
        HyraxVerifier.G1Point memory comY,
        uint256 podpChallenge,
        uint256 zDotR,
        HyraxVerifier.PedersenGens memory gens
    ) private view {
        // Eq1: c * comX + commitD == MSM(gens, z_vector) + z_delta * h
        HyraxVerifier.G1Point memory lhs1 = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(comX, podpChallenge), podp.commitD
        );
        HyraxVerifier.G1Point memory comZ = HyraxVerifier.ecAdd(
            _msmWithTruncatedGens(gens.messageGens, podp.zVector),
            HyraxVerifier.scalarMul(gens.blindingGen, podp.zDelta)
        );
        require(HyraxVerifier.isEqual(lhs1, comZ), "DAGHybrid: Input PODP Eq1 failed");

        // Eq2: c * comY + commitDDotA == zDotR * g_scalar + z_beta * h
        HyraxVerifier.G1Point memory lhs2 = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(comY, podpChallenge), podp.commitDDotA
        );
        HyraxVerifier.G1Point memory comZDotA = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(gens.scalarGen, zDotR),
            HyraxVerifier.scalarMul(gens.blindingGen, podp.zBeta)
        );
        require(HyraxVerifier.isEqual(lhs2, comZDotA), "DAGHybrid: Input PODP Eq2 failed");
    }

    /// @notice Context for public value claim verification (avoids stack-too-deep).
    struct PubClaimCtx {
        GKRDAGVerifier.DAGCircuitDescription desc;
        uint256[][] allBindings;
        GKRDAGVerifier.PublicValueClaim[] claims;
        uint256[] mleEval;
        uint256[] publicInputs;
        HyraxVerifier.PedersenGens gens;
        GKRVerifier.GKRProof proof;
    }

    /// @notice Verify public value claims (Pedersen openings + MLE eval from Groth16).
    function _verifyPublicValueClaimsEC(
        GKRDAGVerifier.DAGCircuitDescription memory desc,
        uint256[][] memory allBindings,
        GKRDAGVerifier.PublicValueClaim[] memory publicValueClaims,
        DAGGroth16Outputs memory outputs,
        uint256[] memory publicInputs,
        HyraxVerifier.PedersenGens memory gens,
        GKRVerifier.GKRProof memory proof
    ) private view {
        PubClaimCtx memory ctx;
        ctx.desc = desc;
        ctx.allBindings = allBindings;
        ctx.claims = publicValueClaims;
        ctx.mleEval = outputs.mleEval;
        ctx.publicInputs = publicInputs;
        ctx.gens = gens;
        ctx.proof = proof;

        uint256 pubClaimIdx = 0;

        for (uint256 inputIdx = 0; inputIdx < ctx.desc.numInputLayers; inputIdx++) {
            if (ctx.desc.inputIsCommitted[inputIdx]) continue;

            uint256 targetLayer = ctx.desc.numComputeLayers + inputIdx;
            pubClaimIdx = _verifyPublicClaimsForLayer(ctx, targetLayer, pubClaimIdx);
        }
    }

    /// @notice Verify public claims for a single non-committed input layer.
    function _verifyPublicClaimsForLayer(
        PubClaimCtx memory ctx,
        uint256 targetLayer,
        uint256 startClaimIdx
    ) private view returns (uint256 nextClaimIdx) {
        nextClaimIdx = startClaimIdx;

        for (uint256 j = 0; j < ctx.desc.numComputeLayers; j++) {
            uint256 atomStart = ctx.desc.atomOffsets[j];
            uint256 atomEnd = ctx.desc.atomOffsets[j + 1];
            for (uint256 a = atomStart; a < atomEnd; a++) {
                if (ctx.desc.atomTargetLayers[a] != targetLayer) continue;

                _verifyOnePublicClaimEC(ctx, j, a, nextClaimIdx);
                nextClaimIdx++;
            }
        }
    }

    /// @notice Verify a single public value claim.
    function _verifyOnePublicClaimEC(
        PubClaimCtx memory ctx,
        uint256 sourceLayer,
        uint256 atomIdx,
        uint256 claimIdx
    ) private view {
        GKRDAGVerifier.PublicValueClaim memory claim = ctx.claims[claimIdx];

        // 1. Commitment consistency
        uint256 commitIdx = ctx.desc.atomCommitIdxs[atomIdx];
        require(
            HyraxVerifier.isEqual(
                claim.commitment,
                ctx.proof.layerProofs[sourceLayer].commitments[commitIdx]
            ),
            "DAGHybrid: public claim commitment mismatch"
        );

        // 2. Pedersen opening: g*value + h*blinding == commitment
        HyraxVerifier.G1Point memory expected = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(ctx.gens.scalarGen, claim.value),
            HyraxVerifier.scalarMul(ctx.gens.blindingGen, claim.blinding)
        );
        require(HyraxVerifier.isEqual(expected, claim.commitment), "DAGHybrid: Pedersen opening invalid");

        // 3. MLE eval from Groth16 matches claim value
        require(ctx.mleEval[claimIdx] == claim.value, "DAGHybrid: mleEval mismatch");

        // 4. Defense-in-depth: verify MLE on-chain too
        uint256[] memory claimPoint = _resolvePoint(atomIdx, ctx.allBindings[sourceLayer], ctx.desc);
        uint256 onChainMle = GKRDAGVerifier.evaluateMLEFromData(ctx.publicInputs, claimPoint);
        require(onChainMle == claim.value, "DAGHybrid: on-chain MLE mismatch");
    }

    // ========================================================================
    // GROTH16 INPUTS BUILDER
    // ========================================================================

    /// @notice Build the flat public inputs array for the DAG Groth16 verifier.
    /// @dev Layout: circuitHash(2) + outputChallenges(V) +
    ///      per-layer(rlcCoeffs + bindings + rhos + gammas + podpChallenge + popChallenge?) +
    ///      per-inputGroup(rlcCoeffs + podpChallenge + lBindings + rBindings) +
    ///      per-pubClaim(claimPoint) +
    ///      publicInputValues(P) +
    ///      outputs(rlcBeta + zDotJStar + lTensorFlat + zDotR + mleEval)
    function buildDAGGroth16Inputs(
        uint256 circuitHashFr0,
        uint256 circuitHashFr1,
        DAGTranscriptChallenges memory challenges,
        uint256[][] memory pubClaimPoints,
        uint256[] memory publicInputValues,
        DAGGroth16Outputs memory outputs
    ) internal pure returns (uint256[] memory inputs) {
        // Calculate total size
        uint256 total = 2 + challenges.outputChallenges.length;

        for (uint256 i = 0; i < challenges.layers.length; i++) {
            total += challenges.layers[i].rlcCoeffs.length;
            total += challenges.layers[i].bindings.length;
            total += challenges.layers[i].rhos.length;
            total += challenges.layers[i].gammas.length;
            total += 1; // podpChallenge
            total += challenges.layers[i].popChallenges.length;
        }

        for (uint256 i = 0; i < challenges.inputGroups.length; i++) {
            total += challenges.inputGroups[i].rlcCoeffs.length;
            total += 1; // podpChallenge
            total += challenges.inputGroups[i].lBindings.length;
            total += challenges.inputGroups[i].rBindings.length;
        }

        for (uint256 i = 0; i < pubClaimPoints.length; i++) {
            total += pubClaimPoints[i].length;
        }

        total += publicInputValues.length;

        total += outputs.rlcBeta.length + outputs.zDotJStar.length;
        total += outputs.lTensorFlat.length;
        total += outputs.zDotR.length + outputs.mleEval.length;

        inputs = new uint256[](total);
        uint256 idx = 0;

        // Circuit hash
        inputs[idx++] = circuitHashFr0;
        inputs[idx++] = circuitHashFr1;

        // Output challenges
        for (uint256 i = 0; i < challenges.outputChallenges.length; i++) {
            inputs[idx++] = challenges.outputChallenges[i];
        }

        // Per-layer challenges
        for (uint256 layer = 0; layer < challenges.layers.length; layer++) {
            DAGLayerChallenges memory lc = challenges.layers[layer];
            for (uint256 i = 0; i < lc.rlcCoeffs.length; i++) inputs[idx++] = lc.rlcCoeffs[i];
            for (uint256 i = 0; i < lc.bindings.length; i++) inputs[idx++] = lc.bindings[i];
            for (uint256 i = 0; i < lc.rhos.length; i++) inputs[idx++] = lc.rhos[i];
            for (uint256 i = 0; i < lc.gammas.length; i++) inputs[idx++] = lc.gammas[i];
            inputs[idx++] = lc.podpChallenge;
            for (uint256 i = 0; i < lc.popChallenges.length; i++) inputs[idx++] = lc.popChallenges[i];
        }

        // Per-input-group challenges + L/R bindings
        for (uint256 i = 0; i < challenges.inputGroups.length; i++) {
            for (uint256 j = 0; j < challenges.inputGroups[i].rlcCoeffs.length; j++) {
                inputs[idx++] = challenges.inputGroups[i].rlcCoeffs[j];
            }
            inputs[idx++] = challenges.inputGroups[i].podpChallenge;
            for (uint256 j = 0; j < challenges.inputGroups[i].lBindings.length; j++) {
                inputs[idx++] = challenges.inputGroups[i].lBindings[j];
            }
            for (uint256 j = 0; j < challenges.inputGroups[i].rBindings.length; j++) {
                inputs[idx++] = challenges.inputGroups[i].rBindings[j];
            }
        }

        // Per-public-claim points
        for (uint256 i = 0; i < pubClaimPoints.length; i++) {
            for (uint256 j = 0; j < pubClaimPoints[i].length; j++) {
                inputs[idx++] = pubClaimPoints[i][j];
            }
        }

        // Public input values
        for (uint256 i = 0; i < publicInputValues.length; i++) {
            inputs[idx++] = publicInputValues[i];
        }

        // Groth16 outputs
        for (uint256 i = 0; i < outputs.rlcBeta.length; i++) inputs[idx++] = outputs.rlcBeta[i];
        for (uint256 i = 0; i < outputs.zDotJStar.length; i++) inputs[idx++] = outputs.zDotJStar[i];
        for (uint256 i = 0; i < outputs.lTensorFlat.length; i++) inputs[idx++] = outputs.lTensorFlat[i];
        for (uint256 i = 0; i < outputs.zDotR.length; i++) inputs[idx++] = outputs.zDotR[i];
        for (uint256 i = 0; i < outputs.mleEval.length; i++) inputs[idx++] = outputs.mleEval[i];
    }

    /// @notice Collect all public claim points from non-committed input layers.
    function collectPubClaimPoints(
        GKRDAGVerifier.DAGCircuitDescription memory desc,
        uint256[][] memory allBindings
    ) internal pure returns (uint256[][] memory pubClaimPoints) {
        // Count total public claims
        uint256 count = 0;
        for (uint256 inputIdx = 0; inputIdx < desc.numInputLayers; inputIdx++) {
            if (desc.inputIsCommitted[inputIdx]) continue;
            uint256 targetLayer = desc.numComputeLayers + inputIdx;
            for (uint256 j = 0; j < desc.atomTargetLayers.length; j++) {
                if (desc.atomTargetLayers[j] == targetLayer) count++;
            }
        }

        pubClaimPoints = new uint256[][](count);
        uint256 idx = 0;

        for (uint256 inputIdx = 0; inputIdx < desc.numInputLayers; inputIdx++) {
            if (desc.inputIsCommitted[inputIdx]) continue;
            uint256 targetLayer = desc.numComputeLayers + inputIdx;

            for (uint256 j = 0; j < desc.numComputeLayers; j++) {
                uint256 atomStart = desc.atomOffsets[j];
                uint256 atomEnd = desc.atomOffsets[j + 1];
                for (uint256 a = atomStart; a < atomEnd; a++) {
                    if (desc.atomTargetLayers[a] != targetLayer) continue;
                    pubClaimPoints[idx] = _resolvePoint(a, allBindings[j], desc);
                    idx++;
                }
            }
        }
    }

    // ========================================================================
    // HELPERS (shared logic duplicated from GKRDAGVerifier / GKRHybridVerifier)
    // ========================================================================

    /// @notice Oracle eval: rlcBeta * SUM(exprCoeffs[j] * commitments[resultIdxs[j]])
    function _computeOracleEval(
        HyraxVerifier.G1Point[] memory commitments,
        uint256 layerIdx,
        uint256 rlcBeta,
        GKRDAGVerifier.DAGCircuitDescription memory desc
    ) private view returns (HyraxVerifier.G1Point memory result) {
        uint256 prodStart = desc.oracleProductOffsets[layerIdx];
        uint256 prodEnd = desc.oracleProductOffsets[layerIdx + 1];

        result = HyraxVerifier.G1Point(0, 0);
        for (uint256 j = prodStart; j < prodEnd; j++) {
            uint256 resultIdx = desc.oracleResultIdxs[j];
            uint256 coeff = mulmod(rlcBeta, desc.oracleExprCoeffs[j], FR_MODULUS);
            HyraxVerifier.G1Point memory term = HyraxVerifier.scalarMul(commitments[resultIdx], coeff);
            result = HyraxVerifier.ecAdd(result, term);
        }
    }

    /// @notice Resolve an atom's claim point from its template and source bindings.
    function _resolvePoint(
        uint256 globalAtomIdx,
        uint256[] memory sourceBindings,
        GKRDAGVerifier.DAGCircuitDescription memory desc
    ) private pure returns (uint256[] memory point) {
        uint256 start = desc.ptOffsets[globalAtomIdx];
        uint256 end = desc.ptOffsets[globalAtomIdx + 1];
        uint256 len = end - start;
        point = new uint256[](len);

        for (uint256 k = 0; k < len; k++) {
            uint256 entry = desc.ptData[start + k];
            if (entry < 1000) {
                point[k] = sourceBindings[entry];
            } else if (entry >= FIXED_REF_BASE) {
                point[k] = entry - FIXED_REF_BASE;
            }
        }
    }

    /// @notice Collect claim points for a target layer (input layer).
    function _collectClaimPoints(
        uint256 targetLayer,
        GKRDAGVerifier.DAGCircuitDescription memory desc,
        uint256[][] memory allBindings
    ) private pure returns (uint256[][] memory claimPoints) {
        uint256 numClaims = _countClaimsFor(targetLayer, desc);
        claimPoints = new uint256[][](numClaims);
        uint256 cIdx = 0;

        for (uint256 j = 0; j < desc.numComputeLayers; j++) {
            uint256 atomStart = desc.atomOffsets[j];
            uint256 atomEnd = desc.atomOffsets[j + 1];
            for (uint256 a = atomStart; a < atomEnd; a++) {
                if (desc.atomTargetLayers[a] == targetLayer) {
                    claimPoints[cIdx] = _resolvePoint(a, allBindings[j], desc);
                    cIdx++;
                }
            }
        }
    }

    /// @notice Count atoms targeting a specific layer.
    function _countClaimsFor(uint256 targetLayer, GKRDAGVerifier.DAGCircuitDescription memory desc)
        private
        pure
        returns (uint256 count)
    {
        for (uint256 j = 0; j < desc.atomTargetLayers.length; j++) {
            if (desc.atomTargetLayers[j] == targetLayer) count++;
        }
    }

    /// @notice Absorb sumcheck messages and derive bindings.
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

    /// @notice Absorb PODP data and squeeze challenge.
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

    /// @notice Squeeze multiple challenge values.
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

    /// @notice MSM with truncated generators.
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

    /// @notice Verify PoP entries for a layer (per-PoP challenges).
    function _verifyLayerPoPs(
        GKRVerifier.CommittedLayerProof memory layerProof,
        uint256[] memory popChallenges,
        HyraxVerifier.PedersenGens memory gens
    ) private view returns (bool) {
        if (layerProof.pops.length == 0) return true;

        uint256 commitIdx = 0;
        for (uint256 i = 0; i < layerProof.pops.length; i++) {
            require(commitIdx + 2 < layerProof.commitments.length, "DAGHybrid: not enough commits for PoP");

            if (!_popCheckEquations(
                    layerProof.pops[i],
                    layerProof.commitments[commitIdx],
                    layerProof.commitments[commitIdx + 1],
                    layerProof.commitments[commitIdx + 2],
                    gens,
                    popChallenges[i]
                )) return false;

            commitIdx += 1;
        }
        return true;
    }

    /// @notice PoP equation checks (3 equations per product triple).
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
    // SORTING AND GROUPING (duplicated from GKRDAGVerifier for transcript match)
    // ========================================================================

    /// @notice Sort claim indices lexicographically by point.
    function _sortClaimIndices(uint256[][] memory claimPoints) private pure returns (uint256[] memory indices) {
        uint256 n = claimPoints.length;
        indices = new uint256[](n);
        for (uint256 i = 0; i < n; i++) indices[i] = i;

        for (uint256 i = 1; i < n; i++) {
            uint256 key = indices[i];
            uint256 j = i;
            while (j > 0 && _comparePoints(claimPoints[indices[j - 1]], claimPoints[key]) > 0) {
                indices[j] = indices[j - 1];
                j--;
            }
            indices[j] = key;
        }
    }

    function _comparePoints(uint256[] memory a, uint256[] memory b) private pure returns (int256) {
        uint256 len = a.length < b.length ? a.length : b.length;
        for (uint256 i = 0; i < len; i++) {
            if (a[i] < b[i]) return -1;
            if (a[i] > b[i]) return int256(1);
        }
        if (a.length < b.length) return -1;
        if (a.length > b.length) return int256(1);
        return 0;
    }

    /// @notice Group sorted claims by shared R-half, matching Remainder_CE prover.
    /// @dev The prover always creates a singleton group for each claim, AND adds
    ///      the claim to any earlier matching group. Result: numGroups == numClaims.
    function _groupClaimsByRHalf(
        uint256[][] memory claimPoints,
        uint256[] memory sortedIndices,
        uint256 logNCols
    ) private pure returns (uint256[][] memory groups) {
        uint256 numClaims = sortedIndices.length;
        groups = new uint256[][](numClaims);

        uint256[][] memory tempGroups = new uint256[][](numClaims);
        uint256[] memory tempGroupSizes = new uint256[](numClaims);

        for (uint256 i = 0; i < numClaims; i++) {
            uint256 claimIdx = sortedIndices[i];
            bool foundMatch = false;

            for (uint256 g = 0; g < i && !foundMatch; g++) {
                if (tempGroupSizes[g] > 0) {
                    uint256 firstInGroup = tempGroups[g][0];
                    if (_rHalfEquals(claimPoints[firstInGroup], claimPoints[claimIdx], logNCols)) {
                        uint256 sz = tempGroupSizes[g];
                        if (sz >= tempGroups[g].length) {
                            uint256[] memory newArr = new uint256[](sz + 4);
                            for (uint256 k = 0; k < sz; k++) newArr[k] = tempGroups[g][k];
                            tempGroups[g] = newArr;
                        }
                        tempGroups[g][sz] = claimIdx;
                        tempGroupSizes[g] = sz + 1;
                        foundMatch = true;
                    }
                }
            }

            // Always create singleton (matching prover behavior)
            tempGroups[i] = new uint256[](1);
            tempGroups[i][0] = claimIdx;
            tempGroupSizes[i] = 1;
        }

        for (uint256 i = 0; i < numClaims; i++) {
            uint256 sz = tempGroupSizes[i];
            groups[i] = new uint256[](sz);
            for (uint256 j = 0; j < sz; j++) {
                groups[i][j] = tempGroups[i][j];
            }
        }
    }

    function _rHalfEquals(uint256[] memory a, uint256[] memory b, uint256 logNCols)
        private pure returns (bool)
    {
        uint256 n = a.length;
        uint256 startR = n - logNCols;
        for (uint256 i = startR; i < n; i++) {
            if (a[i] != b[i]) return false;
        }
        return true;
    }

    /// @notice floor(log2(x)) for x > 0
    function _log2(uint256 x) private pure returns (uint256 result) {
        require(x > 0, "GKRDAGHybrid: log2(0)");
        result = 0;
        uint256 v = x;
        while (v > 1) {
            v >>= 1;
            result++;
        }
    }
}
