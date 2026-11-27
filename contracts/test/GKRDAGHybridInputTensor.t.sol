// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {GKRDAGHybridVerifier} from "../src/remainder/GKRDAGHybridVerifier.sol";
import {GKRDAGVerifier} from "../src/remainder/GKRDAGVerifier.sol";
import {GKRVerifier} from "../src/remainder/GKRVerifier.sol";
import {HyraxVerifier} from "../src/remainder/HyraxVerifier.sol";

uint256 constant PODP_CHALLENGE = 5;
uint256 constant Z_DOT_R = 2;

/// @dev Runs GKRDAGHybridVerifier.verifyDAGECChecks on a circuit with no compute layers and one
///      committed input layer holding a single eval group with a single claim, so that only the
///      input-layer check runs. Every point is a multiple of the curve generator G = (1, 2):
///      message generator G, blinding generator 2G, scalar generator 3G, comEval G.
contract InputGroupECHarness {
    function check(
        HyraxVerifier.G1Point[] memory commitmentRows,
        uint256[] memory lTensor,
        uint256 lHalfLen,
        HyraxVerifier.PODPProof memory podp
    ) external view returns (bool) {
        GKRDAGVerifier.DAGCircuitDescription memory desc;
        desc.numInputLayers = 1;
        desc.inputIsCommitted = new bool[](1);
        desc.inputIsCommitted[0] = true;

        GKRDAGVerifier.DAGInputLayerProof[] memory dagInputProofs = new GKRDAGVerifier.DAGInputLayerProof[](1);
        dagInputProofs[0].commitmentRows = commitmentRows;
        dagInputProofs[0].podps = new HyraxVerifier.PODPProof[](1);
        dagInputProofs[0].podps[0] = podp;
        dagInputProofs[0].comEvals = new HyraxVerifier.G1Point[](1);
        dagInputProofs[0].comEvals[0] = HyraxVerifier.G1Point(1, 2);

        GKRVerifier.GKRProof memory proof;
        return GKRDAGHybridVerifier.verifyDAGECChecks(
            proof,
            _challenges(lHalfLen),
            _outputs(lTensor),
            _gens(),
            desc,
            dagInputProofs,
            new GKRDAGVerifier.PublicValueClaim[](0),
            new uint256[](0)
        );
    }

    function _challenges(uint256 lHalfLen)
        private
        pure
        returns (GKRDAGHybridVerifier.DAGTranscriptChallenges memory challenges)
    {
        challenges.inputGroups = new GKRDAGHybridVerifier.DAGInputGroupChallenges[](1);
        challenges.inputGroups[0].rlcCoeffs = new uint256[](1);
        challenges.inputGroups[0].rlcCoeffs[0] = 1;
        challenges.inputGroups[0].podpChallenge = PODP_CHALLENGE;
        challenges.inputGroups[0].lBindings = new uint256[](lHalfLen);
    }

    function _outputs(uint256[] memory lTensor)
        private
        pure
        returns (GKRDAGHybridVerifier.DAGGroth16Outputs memory outputs)
    {
        outputs.lTensorFlat = lTensor;
        outputs.lTensorOffsets = new uint256[](2);
        outputs.lTensorOffsets[1] = lTensor.length;
        outputs.zDotR = new uint256[](1);
        outputs.zDotR[0] = Z_DOT_R;
    }

    function _gens() private view returns (HyraxVerifier.PedersenGens memory gens) {
        HyraxVerifier.G1Point memory g = HyraxVerifier.G1Point(1, 2);
        gens.messageGens = new HyraxVerifier.G1Point[](1);
        gens.messageGens[0] = g;
        gens.blindingGen = HyraxVerifier.scalarMul(g, 2);
        gens.scalarGen = HyraxVerifier.scalarMul(g, 3);
    }
}

/// @notice The hybrid input-layer check needs one L-tensor coefficient per commitment row
contract GKRDAGHybridInputTensorTest is Test {
    InputGroupECHarness harness;

    function setUp() public {
        harness = new InputGroupECHarness();
    }

    function _rows(uint256 count) internal pure returns (HyraxVerifier.G1Point[] memory rows) {
        rows = new HyraxVerifier.G1Point[](count);
        for (uint256 i = 0; i < count; i++) {
            rows[i] = HyraxVerifier.G1Point(1, 2);
        }
    }

    function _tensor(uint256 count) internal pure returns (uint256[] memory lTensor) {
        lTensor = new uint256[](count);
        for (uint256 i = 0; i < count; i++) {
            lTensor[i] = 2 + i;
        }
    }

    /// @dev Dot-product proof that passes both equations when MSM(rows, coefficients) = comXScalar * G:
    ///      Eq1: c * comX + G == (c * comXScalar + 1) * G
    ///      Eq2: c * G + G == zDotR * 3G  (c = 5, zDotR = 2)
    function _podp(uint256 comXScalar) internal pure returns (HyraxVerifier.PODPProof memory podp) {
        podp.commitD = HyraxVerifier.G1Point(1, 2);
        podp.commitDDotA = HyraxVerifier.G1Point(1, 2);
        podp.zVector = new uint256[](1);
        podp.zVector[0] = PODP_CHALLENGE * comXScalar + 1;
    }

    /// @notice Two rows, coefficients [2, 3]: comX = 5G
    function test_matching_rows_and_coefficients_verify() public view {
        assertTrue(harness.check(_rows(2), _tensor(2), 1, _podp(5)));
    }

    /// @notice A proof built for comX = 5G does not pass for other coefficients (the check is live)
    function test_wrong_comX_is_rejected() public {
        vm.expectRevert(GKRDAGHybridVerifier.InputPODPEq1Failed.selector);
        harness.check(_rows(2), _tensor(2), 1, _podp(4));
    }

    /// @notice One row more than coefficients (the proof is built for comX = identity)
    function test_too_many_rows_reverts() public {
        vm.expectRevert(HyraxVerifier.LCoeffsLengthMismatch.selector);
        harness.check(_rows(3), _tensor(2), 1, _podp(0));
    }

    /// @notice Fewer rows than coefficients (the proof is built for comX = (2 + 3) * G)
    function test_too_few_rows_reverts() public {
        vm.expectRevert(HyraxVerifier.LCoeffsLengthMismatch.selector);
        harness.check(_rows(1), _tensor(2), 1, _podp(5));
    }

    /// @notice Tensor length equals claims * rows, but the row count is not 2^lHalfLen (4 rows, lHalfLen 1)
    function test_row_count_not_matching_l_half_reverts() public {
        vm.expectRevert(HyraxVerifier.LCoeffsLengthMismatch.selector);
        harness.check(_rows(4), _tensor(4), 1, _podp(14));
    }

    /// @notice No rows at all
    function test_zero_rows_reverts() public {
        vm.expectRevert(HyraxVerifier.LCoeffsLengthMismatch.selector);
        harness.check(_rows(0), _tensor(2), 1, _podp(0));
    }

    /// @notice Two sub-tensors for a group with a single claim (the proof is built for comX = (2 + 4 + 3 + 5) * G)
    function test_more_sub_tensors_than_claims_reverts() public {
        vm.expectRevert(HyraxVerifier.LCoeffsLengthMismatch.selector);
        harness.check(_rows(2), _tensor(4), 1, _podp(14));
    }
}
