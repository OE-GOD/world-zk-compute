// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {PoseidonSponge} from "./PoseidonSponge.sol";
import {HyraxVerifier} from "./HyraxVerifier.sol";
import {CommittedSumcheckVerifier} from "./CommittedSumcheckVerifier.sol";
import {GKRVerifier} from "./GKRVerifier.sol";

/// @title GKRDAGVerifier
/// @notice Verifies GKR proofs for arbitrary DAG circuits with multi-claim RLC aggregation.
/// @dev Generalizes GKRVerifier to handle:
///      - Non-adjacent atom routing (atoms can target any forward layer or input layer)
///      - Multi-claim RLC aggregation (layers receiving claims from multiple sources)
///      - Zero-round layers (identity gates with num_vars=0)
///      - Point templates (claim points derived from bindings + fixed values)
library GKRDAGVerifier {
    // ========================================================================
    // ERRORS
    // ========================================================================

    error EvalProofCountMismatch();
    error NotEnoughPublicValueClaims();
    error PublicClaimCommitmentMismatch();
    error PedersenOpeningInvalid();
    error PublicInputMLEMismatch();
    error SumcheckFailed();
    error PoPFailed();
    error NotEnoughCommitmentsForPoP();
    error CommittedInputEvalFailed();
    error DataTooLargeForPoint();
    error Log2OfZero();

    // ========================================================================
    // CONSTANTS
    // ========================================================================

    uint256 internal constant FR_MODULUS =
        21888242871839275222246405745257275088548364400416034343698204186575808495617;

    /// @notice Point template encoding: entries < 1000 are binding refs,
    ///         entries >= 20000 are fixed values (entry - 20000).
    uint256 internal constant FIXED_REF_BASE = 20000;

    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice DAG circuit topology — all metadata needed for DAG verification.
    /// @dev Arrays are indexed in proof order (layer 0 = first processed).
    ///      Atom targets use proof-order indices for compute layers (0..N-1)
    ///      and N+j for input layer j.
    struct DAGCircuitDescription {
        uint256 numComputeLayers;
        uint256 numInputLayers;
        uint8[] layerTypes; // Per compute layer: 0=subtract/identity, 1=multiply
        uint256[] numSumcheckRounds; // Per compute layer
        uint256[] atomOffsets; // Length numComputeLayers+1: atomOffsets[i]..atomOffsets[i+1] = atoms for layer i
        uint256[] atomTargetLayers; // Flat: target layer index for each atom
        uint256[] atomCommitIdxs; // Flat: commitment index within source layer for each atom
        uint256[] ptOffsets; // Length totalAtoms+1: ptOffsets[k]..ptOffsets[k+1] = template for atom k
        uint256[] ptData; // Flat: point template entries
        bool[] inputIsCommitted; // Per input layer
        // Oracle expression: oracleEval = rlcBeta * SUM(exprCoeffs[j] * commitments[resultIdxs[j]])
        uint256[] oracleProductOffsets; // Length numComputeLayers+1: product range per layer
        uint256[] oracleResultIdxs; // Flat: which commitment index to use per product
        uint256[] oracleExprCoeffs; // Flat: circuit-level coefficient per product (Fr)
    }

    /// @notice Per-input-layer proof with multiple eval proofs (one per claim group).
    struct DAGInputLayerProof {
        HyraxVerifier.G1Point[] commitmentRows; // Shared Hyrax commitment rows
        HyraxVerifier.PODPProof[] podps; // One PODP per eval proof
        HyraxVerifier.G1Point[] comEvals; // One comEval per eval proof
    }

    /// @notice A claim on a public value: Pedersen opening (value, blinding, commitment).
    struct PublicValueClaim {
        uint256 value;
        uint256 blinding;
        HyraxVerifier.G1Point commitment;
    }

    /// @notice Bundled context to avoid stack-too-deep in layer processing.
    struct VerifyContext {
        GKRVerifier.GKRProof proof;
        DAGCircuitDescription desc;
        HyraxVerifier.PedersenGens gens;
        uint256[][] allBindings;
        uint256[] outputChallenges;
        HyraxVerifier.G1Point outputEval;
        uint256[] publicInputs;
        DAGInputLayerProof[] dagInputProofs; // Multi-eval input proofs
        PublicValueClaim[] publicValueClaims; // Pedersen openings for public input atoms
    }

    // ========================================================================
    // COMPUTE LAYER VERIFICATION
    // ========================================================================

    /// @notice Verify all compute layers of a DAG circuit.
    /// @param proof Complete GKR proof (reuses GKRVerifier.GKRProof struct)
    /// @param desc DAG circuit description
    /// @param gens Pedersen generators
    /// @param sponge Pre-initialized Fiat-Shamir transcript
    /// @return ctx Verification context (contains allBindings for input layer verification)
    function verifyComputeLayers(
        GKRVerifier.GKRProof memory proof,
        DAGCircuitDescription memory desc,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge,
        uint256[] memory publicInputs,
        DAGInputLayerProof[] memory dagInputProofs,
        PublicValueClaim[] memory publicValueClaims
    ) internal view returns (VerifyContext memory ctx) {
        ctx.proof = proof;
        ctx.desc = desc;
        ctx.gens = gens;
        ctx.allBindings = new uint256[][](desc.numComputeLayers);
        ctx.publicInputs = publicInputs;
        ctx.dagInputProofs = dagInputProofs;
        ctx.publicValueClaims = publicValueClaims;

        // Output layer: squeeze numVars challenges (matching first layer's sumcheck rounds)
        uint256 numVars = proof.layerProofs[0].sumcheckProof.messages.length;
        ctx.outputChallenges = new uint256[](numVars);
        for (uint256 i = 0; i < numVars; i++) {
            ctx.outputChallenges[i] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
        }
        ctx.outputEval = proof.outputClaimCommitments[0];
        PoseidonSponge.absorb(sponge, ctx.outputEval.x);
        PoseidonSponge.absorb(sponge, ctx.outputEval.y);

        // Process each compute layer in proof order
        for (uint256 i = 0; i < desc.numComputeLayers; i++) {
            ctx.allBindings[i] = _processOneLayer(i, ctx, sponge);
        }
    }

    /// @notice Verify a batch of compute layers (for multi-tx verification).
    /// @dev Like verifyComputeLayers but processes only [startLayer, endLayer) and uses
    ///      pre-populated bindings from prior batches.
    /// @param proof Complete GKR proof (re-supplied via calldata each batch)
    /// @param desc DAG circuit description
    /// @param gens Pedersen generators
    /// @param sponge Transcript state (loaded from storage for batches > 0)
    /// @param allBindings Pre-populated bindings array (prior batch bindings already filled)
    /// @param outputChallenges Output challenges (squeezed once in batch 0)
    /// @param outputEval Output commitment point (from batch 0)
    /// @param startLayer First compute layer to process (inclusive)
    /// @param endLayer Last compute layer to process (exclusive)
    function verifyComputeLayersBatch(
        GKRVerifier.GKRProof memory proof,
        DAGCircuitDescription memory desc,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge,
        uint256[][] memory allBindings,
        uint256[] memory outputChallenges,
        HyraxVerifier.G1Point memory outputEval,
        uint256 startLayer,
        uint256 endLayer
    ) internal view {
        // Build a minimal VerifyContext for _processOneLayer
        VerifyContext memory ctx;
        ctx.proof = proof;
        ctx.desc = desc;
        ctx.gens = gens;
        ctx.allBindings = allBindings;
        ctx.outputChallenges = outputChallenges;
        ctx.outputEval = outputEval;

        for (uint256 i = startLayer; i < endLayer; i++) {
            ctx.allBindings[i] = _processOneLayer(i, ctx, sponge);
        }
    }

    /// @notice Collect claim points for atoms targeting a specific input layer.
    /// @param targetLayer The target layer index (numComputeLayers + inputIdx)
    /// @param desc DAG circuit description
    /// @param allBindings All bindings from compute layer verification
    /// @return claimPoints Array of resolved claim points
    function collectClaimPoints(uint256 targetLayer, DAGCircuitDescription memory desc, uint256[][] memory allBindings)
        internal
        pure
        returns (uint256[][] memory claimPoints)
    {
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

    /// @notice Verify a batch of committed input eval groups [startGroup, endGroup).
    /// @dev Sorts claims and groups them (deterministic — same result each call), then
    ///      only verifies groups in the [startGroup, endGroup) range.
    ///      Sponge must be positioned at the start of group `startGroup` on entry.
    /// @param dagProof Input layer proof (commitment rows + PODP proofs)
    /// @param claimPoints All claim points for this input layer
    /// @param sponge Transcript sponge (threaded through groups sequentially)
    /// @param gens Pedersen generators
    /// @param startGroup First group to verify (inclusive)
    /// @param endGroup Last group to verify (exclusive)
    function verifyCommittedInputGroupsBatch(
        DAGInputLayerProof memory dagProof,
        uint256[][] memory claimPoints,
        PoseidonSponge.Sponge memory sponge,
        HyraxVerifier.PedersenGens memory gens,
        uint256 startGroup,
        uint256 endGroup
    ) internal view {
        uint256 numRows = dagProof.commitmentRows.length;
        uint256 n = claimPoints[0].length;
        uint256 lHalfLen = _log2(numRows > 0 ? numRows : 1);
        uint256 logNCols = n - lHalfLen;

        // Sort and group (deterministic, recomputed each call)
        uint256[] memory sortedIndices = _sortClaimIndices(claimPoints);
        uint256[][] memory groups = _groupClaimsByRHalf(claimPoints, sortedIndices, logNCols);

        if (groups.length != dagProof.podps.length) revert EvalProofCountMismatch();
        if (endGroup > groups.length) endGroup = groups.length;

        EvalGroupCtx memory egCtx;
        egCtx.dagProof = dagProof;
        egCtx.claimPoints = claimPoints;
        egCtx.lHalfLen = lHalfLen;
        egCtx.logNCols = logNCols;
        egCtx.gens = gens;

        for (uint256 g = startGroup; g < endGroup; g++) {
            _verifyOneEvalGroup(egCtx, g, groups[g], sponge);
        }
    }

    /// @notice Verify committed input layers using accumulated claims.
    /// @param ctx Verification context from compute layer verification
    /// @param sponge Transcript (continuing from compute layers)
    function verifyInputLayers(VerifyContext memory ctx, PoseidonSponge.Sponge memory sponge) internal view {
        uint256 dagInputIdx = 0;
        uint256 pubClaimIdx = 0;

        for (uint256 inputIdx = 0; inputIdx < ctx.desc.numInputLayers; inputIdx++) {
            uint256 targetLayer = ctx.desc.numComputeLayers + inputIdx;

            // Collect claim points for this input layer
            uint256 numClaims = _countClaimsFor(targetLayer, ctx.desc);
            uint256[][] memory claimPoints = new uint256[][](numClaims);
            uint256 cIdx = 0;

            // Scan atoms from all compute layers
            for (uint256 j = 0; j < ctx.desc.numComputeLayers; j++) {
                uint256 atomStart = ctx.desc.atomOffsets[j];
                uint256 atomEnd = ctx.desc.atomOffsets[j + 1];
                for (uint256 a = atomStart; a < atomEnd; a++) {
                    if (ctx.desc.atomTargetLayers[a] == targetLayer) {
                        claimPoints[cIdx] = _resolvePoint(a, ctx.allBindings[j], ctx.desc);
                        cIdx++;
                    }
                }
            }

            if (ctx.desc.inputIsCommitted[inputIdx]) {
                // Committed input: sort claims, group by R-half, verify per-group eval proofs
                _verifyCommittedInputBatchEval(ctx.dagInputProofs[dagInputIdx], claimPoints, sponge, ctx.gens);
                dagInputIdx++;
            } else {
                // Public input: verify Pedersen openings and MLE evaluations
                pubClaimIdx = _verifyPublicInputClaims(ctx, claimPoints, targetLayer, pubClaimIdx);
            }
        }
    }

    // ========================================================================
    // PUBLIC INPUT VERIFICATION
    // ========================================================================

    /// @notice Verify that atom evaluation commitments targeting a public input layer
    ///         are consistent with the known public data via Pedersen openings.
    /// @dev For each atom, finds the matching claim from claims_on_public_values and checks:
    ///      1. Pedersen opening: g*value + h*blinding == commitment
    ///      2. MLE evaluation: MLE(pubData, claimPoint) == value
    ///      3. Commitment consistency: claim.commitment == atom's commitment
    function _verifyPublicInputClaims(
        VerifyContext memory ctx,
        uint256[][] memory claimPoints,
        uint256 targetLayer,
        uint256 pubClaimStartIdx
    ) private view returns (uint256 nextPubClaimIdx) {
        nextPubClaimIdx = pubClaimStartIdx;

        uint256 cIdx = 0;
        for (uint256 j = 0; j < ctx.desc.numComputeLayers; j++) {
            uint256 atomStart = ctx.desc.atomOffsets[j];
            uint256 atomEnd = ctx.desc.atomOffsets[j + 1];
            for (uint256 a = atomStart; a < atomEnd; a++) {
                if (ctx.desc.atomTargetLayers[a] != targetLayer) continue;

                _verifyOnePublicClaim(
                    ctx,
                    claimPoints[cIdx],
                    ctx.proof.layerProofs[j].commitments[ctx.desc.atomCommitIdxs[a]],
                    nextPubClaimIdx
                );
                nextPubClaimIdx++;
                cIdx++;
            }
        }
    }

    /// @notice Verify a single public value claim: Pedersen opening + MLE + commitment match.
    function _verifyOnePublicClaim(
        VerifyContext memory ctx,
        uint256[] memory claimPoint,
        HyraxVerifier.G1Point memory atomCommitment,
        uint256 claimIdx
    ) private view {
        if (claimIdx >= ctx.publicValueClaims.length) revert NotEnoughPublicValueClaims();
        PublicValueClaim memory claim = ctx.publicValueClaims[claimIdx];

        // 1. Commitment consistency: claim commitment matches atom commitment
        if (!HyraxVerifier.isEqual(claim.commitment, atomCommitment)) revert PublicClaimCommitmentMismatch();

        // 2. Pedersen opening: g*value + h*blinding == commitment
        HyraxVerifier.G1Point memory expected = HyraxVerifier.ecAdd(
            HyraxVerifier.scalarMul(ctx.gens.scalarGen, claim.value),
            HyraxVerifier.scalarMul(ctx.gens.blindingGen, claim.blinding)
        );
        if (!HyraxVerifier.isEqual(expected, claim.commitment)) revert PedersenOpeningInvalid();

        // 3. MLE evaluation: MLE(pubData, point) == value
        if (evaluateMLEFromData(ctx.publicInputs, claimPoint) != claim.value) revert PublicInputMLEMismatch();
    }

    // ========================================================================
    // SINGLE LAYER PROCESSING
    // ========================================================================

    /// @notice Process one compute layer: RLC aggregation + committed sumcheck + PoP
    function _processOneLayer(uint256 layerIdx, VerifyContext memory ctx, PoseidonSponge.Sponge memory sponge)
        private
        view
        returns (uint256[] memory bindings)
    {
        // Count claims for this layer
        uint256 numClaims = _countClaimsFor(layerIdx, ctx.desc);
        if (layerIdx == 0) numClaims++; // Output claim targets layer 0

        // Squeeze RLC coefficients
        uint256[] memory rlcCoeffs = new uint256[](numClaims);
        for (uint256 k = 0; k < numClaims; k++) {
            rlcCoeffs[k] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
        }

        // Absorb messages and derive bindings
        GKRVerifier.CommittedLayerProof memory lp = ctx.proof.layerProofs[layerIdx];
        bindings = _absorbMessagesAndDeriveBindings(lp.sumcheckProof.messages, sponge);

        // Absorb post-sumcheck commitments
        _absorbCommitments(lp.commitments, sponge);

        // Compute RLC eval and beta from all incoming claims
        (, uint256 rlcBeta) = _computeRlcEvalAndBeta(layerIdx, ctx, bindings, rlcCoeffs);

        // Verify sumcheck and PoP
        _verifySumcheckAndPoP(layerIdx, lp, ctx.desc, ctx.gens, sponge, bindings, rlcBeta);
    }

    /// @notice Absorb post-sumcheck commitments into transcript.
    function _absorbCommitments(HyraxVerifier.G1Point[] memory commitments, PoseidonSponge.Sponge memory sponge)
        private
        pure
    {
        for (uint256 j = 0; j < commitments.length; j++) {
            PoseidonSponge.absorb(sponge, commitments[j].x);
            PoseidonSponge.absorb(sponge, commitments[j].y);
        }
    }

    /// @notice Verify committed sumcheck and proof-of-product for a layer.
    function _verifySumcheckAndPoP(
        uint256 layerIdx,
        GKRVerifier.CommittedLayerProof memory lp,
        DAGCircuitDescription memory desc,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge,
        uint256[] memory bindings,
        uint256 rlcBeta
    ) private view {
        // Compute oracle eval using general expression
        HyraxVerifier.G1Point memory oracleEval = _computeOracleEval(lp.commitments, layerIdx, rlcBeta, desc);

        // Verify committed sumcheck
        uint256 degree = (desc.layerTypes[layerIdx] == 1) ? 3 : 2;
        if (!CommittedSumcheckVerifier.verify(lp.sumcheckProof, oracleEval, degree, bindings, gens, sponge)) {
            revert SumcheckFailed();
        }

        // Verify ProofOfProduct
        if (!_verifyProducts(lp, gens, sponge)) revert PoPFailed();
    }

    // ========================================================================
    // RLC COMPUTATION
    // ========================================================================

    /// @notice Compute rlcEval (EC point) and rlcBeta (scalar) from all incoming claims.
    /// @dev Scans atoms from all earlier layers targeting this layer,
    ///      plus the output claim if this is layer 0.
    function _computeRlcEvalAndBeta(
        uint256 layerIdx,
        VerifyContext memory ctx,
        uint256[] memory bindings,
        uint256[] memory rlcCoeffs
    ) private view returns (HyraxVerifier.G1Point memory rlcEval, uint256 rlcBeta) {
        rlcEval = HyraxVerifier.G1Point(0, 0);
        rlcBeta = 0;
        uint256 coeffIdx = 0;

        // Output claim contribution (targets layer 0)
        if (layerIdx == 0) {
            rlcEval = HyraxVerifier.scalarMul(ctx.outputEval, rlcCoeffs[0]);
            uint256 beta = _computeBeta(bindings, ctx.outputChallenges);
            rlcBeta = mulmod(beta, rlcCoeffs[0], FR_MODULUS);
            coeffIdx = 1;
        }

        // Claims from earlier layers' atoms
        for (uint256 j = 0; j < layerIdx; j++) {
            uint256 atomStart = ctx.desc.atomOffsets[j];
            uint256 atomEnd = ctx.desc.atomOffsets[j + 1];
            for (uint256 a = atomStart; a < atomEnd; a++) {
                if (ctx.desc.atomTargetLayers[a] != layerIdx) continue;

                uint256 commitIdx = ctx.desc.atomCommitIdxs[a];
                HyraxVerifier.G1Point memory atomEval = ctx.proof.layerProofs[j].commitments[commitIdx];
                uint256[] memory atomPoint = _resolvePoint(a, ctx.allBindings[j], ctx.desc);
                uint256 beta = _computeBeta(bindings, atomPoint);

                rlcEval = HyraxVerifier.ecAdd(rlcEval, HyraxVerifier.scalarMul(atomEval, rlcCoeffs[coeffIdx]));
                rlcBeta = addmod(rlcBeta, mulmod(beta, rlcCoeffs[coeffIdx], FR_MODULUS), FR_MODULUS);
                coeffIdx++;
            }
        }
    }

    // ========================================================================
    // POINT RESOLUTION
    // ========================================================================

    /// @notice Resolve an atom's claim point from its template and source bindings.
    /// @dev Template entries: <1000 = binding ref, >=20000 = fixed value (entry-20000).
    function _resolvePoint(uint256 globalAtomIdx, uint256[] memory sourceBindings, DAGCircuitDescription memory desc)
        internal
        pure
        returns (uint256[] memory point)
    {
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
            // CLAIM_REF (10000-19999) not used in Phase 1a circuits
        }
    }

    // ========================================================================
    // BETA AND ORACLE EVAL
    // ========================================================================

    /// @notice Compute beta(bindings, point) = prod_i(r_i*c_i + (1-r_i)*(1-c_i))
    function _computeBeta(uint256[] memory bindings, uint256[] memory point) internal pure returns (uint256 beta) {
        uint256 n = bindings.length < point.length ? bindings.length : point.length;
        beta = 1;
        for (uint256 i = 0; i < n; i++) {
            uint256 rc = mulmod(bindings[i], point[i], FR_MODULUS);
            uint256 oneMinusR = addmod(1, FR_MODULUS - bindings[i], FR_MODULUS);
            uint256 oneMinusC = addmod(1, FR_MODULUS - point[i], FR_MODULUS);
            uint256 term = addmod(rc, mulmod(oneMinusR, oneMinusC, FR_MODULUS), FR_MODULUS);
            beta = mulmod(beta, term, FR_MODULUS);
        }
    }

    /// @notice Compute oracle_eval from PostSumcheckLayer commitments using general expression.
    /// @dev oracleEval = rlcBeta * SUM(exprCoeffs[j] * commitments[resultIdxs[j]])
    ///      The exprCoeffs are circuit-level constants (prod.coefficient / rlcBeta, which cancels rlcBeta
    ///      from the product coefficients, leaving only circuit-dependent factors).
    function _computeOracleEval(
        HyraxVerifier.G1Point[] memory commitments,
        uint256 layerIdx,
        uint256 rlcBeta,
        DAGCircuitDescription memory desc
    ) internal view returns (HyraxVerifier.G1Point memory result) {
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

    // ========================================================================
    // LAYER HELPERS (shared with GKRVerifier)
    // ========================================================================

    /// @notice Absorb committed sumcheck messages and derive bindings.
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

    /// @notice Verify ProofOfProduct entries (sliding window over commitments).
    function _verifyProducts(
        GKRVerifier.CommittedLayerProof memory layerProof,
        HyraxVerifier.PedersenGens memory gens,
        PoseidonSponge.Sponge memory sponge
    ) private view returns (bool) {
        if (layerProof.pops.length == 0) return true;

        uint256 commitIdx = 0;
        for (uint256 i = 0; i < layerProof.pops.length; i++) {
            if (commitIdx + 2 >= layerProof.commitments.length) revert NotEnoughCommitmentsForPoP();

            bool popValid = HyraxVerifier.verifyProofOfProduct(
                layerProof.pops[i],
                layerProof.commitments[commitIdx],
                layerProof.commitments[commitIdx + 1],
                layerProof.commitments[commitIdx + 2],
                gens,
                sponge
            );
            if (!popValid) return false;
            commitIdx += 1;
        }
        return true;
    }

    // ========================================================================
    // CLAIM COUNTING
    // ========================================================================

    /// @notice Count atoms targeting a specific layer.
    function _countClaimsFor(uint256 targetLayer, DAGCircuitDescription memory desc)
        internal
        pure
        returns (uint256 count)
    {
        for (uint256 j = 0; j < desc.atomTargetLayers.length; j++) {
            if (desc.atomTargetLayers[j] == targetLayer) count++;
        }
    }

    // ========================================================================
    // INPUT LAYER HELPERS
    // ========================================================================

    /// @notice Verify a committed input layer with batch evaluation proofs.
    /// @dev Matches Rust's HyraxInputLayerProof::verify with batch_opening=true:
    ///      1. Sort claims lexicographically by point
    ///      2. Group sorted claims by shared R-half (Rust's grouping function)
    ///      3. For each group, verify against the corresponding eval proof
    function _verifyCommittedInputBatchEval(
        DAGInputLayerProof memory dagProof,
        uint256[][] memory claimPoints,
        PoseidonSponge.Sponge memory sponge,
        HyraxVerifier.PedersenGens memory gens
    ) private view {
        uint256 numRows = dagProof.commitmentRows.length;
        uint256 n = claimPoints[0].length;
        uint256 lHalfLen = _log2(numRows > 0 ? numRows : 1);
        uint256 logNCols = n - lHalfLen;

        // Step 1: Sort claims lexicographically by point
        uint256[] memory sortedIndices = _sortClaimIndices(claimPoints);

        // Step 2: Group sorted claims by shared R-half (matching Rust's buggy grouping)
        // Each claim always creates a new group; if it matches an existing group, it's also added there.
        // Result: numGroups = numClaims (always)
        uint256[][] memory groups = _groupClaimsByRHalf(claimPoints, sortedIndices, logNCols);

        // Step 3: Verify each group against its eval proof
        if (groups.length != dagProof.podps.length) revert EvalProofCountMismatch();

        EvalGroupCtx memory egCtx;
        egCtx.dagProof = dagProof;
        egCtx.claimPoints = claimPoints;
        egCtx.lHalfLen = lHalfLen;
        egCtx.logNCols = logNCols;
        egCtx.gens = gens;

        for (uint256 g = 0; g < groups.length; g++) {
            _verifyOneEvalGroup(egCtx, g, groups[g], sponge);
        }
    }

    /// @notice Context for eval group verification (avoids stack-too-deep).
    struct EvalGroupCtx {
        DAGInputLayerProof dagProof;
        uint256[][] claimPoints;
        uint256 lHalfLen;
        uint256 logNCols;
        HyraxVerifier.PedersenGens gens;
    }

    /// @notice Verify a single eval proof group.
    function _verifyOneEvalGroup(
        EvalGroupCtx memory egCtx,
        uint256 groupIdx,
        uint256[] memory groupClaimIndices,
        PoseidonSponge.Sponge memory sponge
    ) private view {
        uint256 groupSize = groupClaimIndices.length;

        // Squeeze RLC coefficients for this group
        uint256[] memory rlcCoeffs = new uint256[](groupSize);
        for (uint256 i = 0; i < groupSize; i++) {
            rlcCoeffs[i] = PoseidonSponge.squeeze(sponge) % FR_MODULUS;
        }

        // Extract group's claim points
        uint256[][] memory groupPoints = new uint256[][](groupSize);
        for (uint256 i = 0; i < groupSize; i++) {
            groupPoints[i] = egCtx.claimPoints[groupClaimIndices[i]];
        }

        // Compute L and R tensors, absorb, and verify
        _verifyEvalGroupInner(egCtx, groupIdx, groupPoints, rlcCoeffs, sponge);
    }

    /// @notice Inner verification for eval group (split out to avoid stack-too-deep).
    function _verifyEvalGroupInner(
        EvalGroupCtx memory egCtx,
        uint256 groupIdx,
        uint256[][] memory groupPoints,
        uint256[] memory rlcCoeffs,
        PoseidonSponge.Sponge memory sponge
    ) private view {
        // Compute L tensor (RLC of L-halves)
        uint256[] memory lCoeffs = _computeRlcTensor(groupPoints, egCtx.lHalfLen, rlcCoeffs);

        // Compute R tensor (shared R-half from first claim in group)
        uint256[] memory rVars = new uint256[](egCtx.logNCols);
        for (uint256 j = 0; j < egCtx.logNCols; j++) {
            rVars[j] = groupPoints[0][egCtx.lHalfLen + j];
        }
        uint256[] memory rCoeffs = _initializeTensor(rVars);

        // Absorb comEval
        PoseidonSponge.absorb(sponge, egCtx.dagProof.comEvals[groupIdx].x);
        PoseidonSponge.absorb(sponge, egCtx.dagProof.comEvals[groupIdx].y);

        // Absorb PODP data and squeeze challenge
        uint256 podpChallenge = _absorbPODP(egCtx.dagProof.podps[groupIdx], sponge);

        // Build temp EvalProof struct for verifyEvaluation
        HyraxVerifier.EvalProof memory evalProof;
        evalProof.commitmentRows = egCtx.dagProof.commitmentRows;
        evalProof.podp = egCtx.dagProof.podps[groupIdx];
        evalProof.comEval = egCtx.dagProof.comEvals[groupIdx];

        if (!HyraxVerifier.verifyEvaluation(evalProof, lCoeffs, rCoeffs, 0, podpChallenge, egCtx.gens)) {
            revert CommittedInputEvalFailed();
        }
    }

    /// @notice Sort claim indices lexicographically by point (insertion sort for small arrays).
    function _sortClaimIndices(uint256[][] memory claimPoints) private pure returns (uint256[] memory indices) {
        uint256 n = claimPoints.length;
        indices = new uint256[](n);
        for (uint256 i = 0; i < n; i++) {
            indices[i] = i;
        }

        // Insertion sort (adequate for n <= ~50)
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

    /// @notice Lexicographic comparison of two claim points. Returns >0 if a > b, 0 if equal, <0 if a < b.
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

    /// @notice Group sorted claims by shared R-half, matching Rust's grouping function.
    /// @dev The Rust function always creates a new singleton group for each claim,
    ///      AND if a matching existing group is found, also adds the claim there.
    ///      Result: numGroups == numClaims always.
    function _groupClaimsByRHalf(uint256[][] memory claimPoints, uint256[] memory sortedIndices, uint256 logNCols)
        private
        pure
        returns (uint256[][] memory groups)
    {
        uint256 numClaims = sortedIndices.length;
        // Each claim creates exactly one group (Rust behavior)
        groups = new uint256[][](numClaims);

        // Temporary: track which group each claim initially creates
        // AND whether it was also added to a previous group
        uint256[][] memory tempGroups = new uint256[][](numClaims);
        uint256[] memory tempGroupSizes = new uint256[](numClaims);

        for (uint256 i = 0; i < numClaims; i++) {
            uint256 claimIdx = sortedIndices[i];
            bool foundMatch = false;

            // Try to find an existing group with matching R-half
            for (uint256 g = 0; g < i && !foundMatch; g++) {
                if (tempGroupSizes[g] > 0) {
                    uint256 firstInGroup = tempGroups[g][0];
                    if (_rHalfEquals(claimPoints[firstInGroup], claimPoints[claimIdx], logNCols)) {
                        // Add to existing group
                        uint256 sz = tempGroupSizes[g];
                        // Grow group array if needed
                        if (sz >= tempGroups[g].length) {
                            uint256[] memory newArr = new uint256[](sz + 4);
                            for (uint256 k = 0; k < sz; k++) {
                                newArr[k] = tempGroups[g][k];
                            }
                            tempGroups[g] = newArr;
                        }
                        tempGroups[g][sz] = claimIdx;
                        tempGroupSizes[g] = sz + 1;
                        foundMatch = true;
                    }
                }
            }

            // Always create a new singleton group (Rust behavior)
            tempGroups[i] = new uint256[](1);
            tempGroups[i][0] = claimIdx;
            tempGroupSizes[i] = 1;
        }

        // Convert to final format
        for (uint256 i = 0; i < numClaims; i++) {
            uint256 sz = tempGroupSizes[i];
            groups[i] = new uint256[](sz);
            for (uint256 j = 0; j < sz; j++) {
                groups[i][j] = tempGroups[i][j];
            }
        }
    }

    /// @notice Check if two claim points share the same R-half (last logNCols coordinates).
    function _rHalfEquals(uint256[] memory a, uint256[] memory b, uint256 logNCols) private pure returns (bool) {
        uint256 n = a.length;
        uint256 startR = n - logNCols;
        for (uint256 i = startR; i < n; i++) {
            if (a[i] != b[i]) return false;
        }
        return true;
    }

    /// @notice Compute RLC of tensor products for L-halves of claim points.
    function _computeRlcTensor(uint256[][] memory claimPoints, uint256 lHalfLen, uint256[] memory rlcCoeffs)
        private
        pure
        returns (uint256[] memory result)
    {
        // forge-lint: disable-next-line(incorrect-shift)
        uint256 tensorLen = 1 << lHalfLen;
        result = new uint256[](tensorLen);

        for (uint256 c = 0; c < claimPoints.length; c++) {
            uint256[] memory lVars = new uint256[](lHalfLen);
            for (uint256 j = 0; j < lHalfLen; j++) {
                lVars[j] = claimPoints[c][j];
            }
            uint256[] memory tensor = _initializeTensor(lVars);
            for (uint256 j = 0; j < tensorLen; j++) {
                result[j] = addmod(result[j], mulmod(tensor[j], rlcCoeffs[c], FR_MODULUS), FR_MODULUS);
            }
        }
    }

    /// @notice Absorb PODP data and return challenge.
    function _absorbPODP(HyraxVerifier.PODPProof memory podp, PoseidonSponge.Sponge memory sponge)
        private
        pure
        returns (uint256)
    {
        PoseidonSponge.absorb(sponge, podp.commitD.x);
        PoseidonSponge.absorb(sponge, podp.commitD.y);
        PoseidonSponge.absorb(sponge, podp.commitDDotA.x);
        PoseidonSponge.absorb(sponge, podp.commitDDotA.y);
        uint256 podpChallenge = PoseidonSponge.squeeze(sponge) % FR_MODULUS;

        for (uint256 j = 0; j < podp.zVector.length; j++) {
            PoseidonSponge.absorb(sponge, podp.zVector[j]);
        }
        PoseidonSponge.absorb(sponge, podp.zDelta);
        PoseidonSponge.absorb(sponge, podp.zBeta);

        return podpChallenge;
    }

    /// @notice Tensor product matching Rust's initialize_tensor (reverse order).
    function _initializeTensor(uint256[] memory coords) private pure returns (uint256[] memory table) {
        // forge-lint: disable-next-line(incorrect-shift)
        uint256 size = 1 << coords.length;
        table = new uint256[](size);
        table[0] = 1;
        if (coords.length == 0) return table;

        uint256 len = 1;
        for (uint256 idx = coords.length; idx > 0; idx--) {
            uint256 r = coords[idx - 1];
            uint256 oneMinusR = addmod(1, FR_MODULUS - r, FR_MODULUS);
            for (uint256 i = 0; i < len; i++) {
                table[i + len] = mulmod(table[i], r, FR_MODULUS);
                table[i] = mulmod(table[i], oneMinusR, FR_MODULUS);
            }
            len *= 2;
        }
    }

    /// @notice Evaluate MLE of data at point: MLE(x) = sum_w data[w] * eq(w, x)
    /// @dev Uses Remainder's MSB-first convention: point[0] is the MSB of the data index.
    ///      This matches initialize_tensor's reverse processing order.
    function evaluateMLEFromData(uint256[] memory data, uint256[] memory point) internal pure returns (uint256) {
        uint256 n = point.length;
        // forge-lint: disable-next-line(incorrect-shift)
        if (data.length > (1 << n)) revert DataTooLargeForPoint();

        uint256 result = 0;
        for (uint256 w = 0; w < data.length; w++) {
            if (data[w] == 0) continue;
            uint256 eq = 1;
            for (uint256 i = 0; i < n; i++) {
                // MSB-first: point[0] controls the highest bit of w
                uint256 wi = (w >> (n - 1 - i)) & 1;
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

    /// @notice floor(log2(x)) for x > 0
    function _log2(uint256 x) private pure returns (uint256 result) {
        if (x == 0) revert Log2OfZero();
        result = 0;
        uint256 v = x;
        while (v > 1) {
            v >>= 1;
            result++;
        }
    }
}
