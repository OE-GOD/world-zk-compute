package main

import (
	"math/big"
	"testing"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/backend"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"
	"github.com/consensys/gnark/test"
)

// smallDAGConfig returns a minimal DAG config for testing:
// 3 compute layers, 2 input groups, 1 public claim.
// Layer 0: 2 claims (from output challenges), nv=2, degree=2
// Layer 1: 1 claim (from layer 0 bindings), nv=3, degree=3
// Layer 2: 1 claim (from layer 1 bindings), nv=1, degree=2
func smallDAGConfig() DAGCircuitConfig {
	return DAGCircuitConfig{
		NumComputeLayers: 3,
		LayerNumVars:     []int{2, 3, 1},
		LayerDegrees:     []int{2, 3, 2},
		LayerNumClaims:   []int{2, 1, 1},
		OutputNumVars:    2,

		NumInputGroups:      2,
		InputGroupNumClaims: []int{1, 1},
		InputGroupLHalfLen:  []int{1, 1},
		InputGroupRHalfLen:  []int{1, 1},

		NumPublicClaims: 1,
		PubInputCount:   4,
		PubClaimNumVars: []int{2},

		// Point templates for compute layers only:
		// Layer 0, claim 0: output challenges [B(0), B(1)]
		// Layer 0, claim 1: output challenges [B(0), B(1)]
		// Layer 1, claim 0: layer 0 bindings [B(0), B(1), F(0)]
		// Layer 2, claim 0: layer 1 bindings [B(0)]
		PointTemplates: [][][]int{
			{{0, 1}, {0, 1}},                // Layer 0: 2 claims from output
			{{0, 1, FIXED_REF_BASE}},        // Layer 1: 1 claim from layer 0
			{{0}},                            // Layer 2: 1 claim from layer 1
		},
		PointTemplateSources: [][]int{
			{-1, -1}, // output challenges
			{0},      // from layer 0
			{1},      // from layer 1
		},
	}
}

func TestDAGCircuitCompiles(t *testing.T) {
	config := smallDAGConfig()
	circuit := AllocateDAGCircuit(config)
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		t.Fatalf("DAG circuit compilation failed: %v", err)
	}
	t.Logf("DAG circuit: %d constraints, expected pub inputs: %d",
		ccs.GetNbConstraints(), config.DAGExpectedPublicInputCount())
}

func TestDAGCircuitProves(t *testing.T) {
	config := smallDAGConfig()
	circuit := AllocateDAGCircuit(config)
	assignment := makeDAGDummyAssignment(config)

	assert := test.NewAssert(t)
	assert.ProverSucceeded(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("DAG Groth16 proof verified successfully")
}

func TestDAGCircuitRejectsInvalidRlcBeta(t *testing.T) {
	config := smallDAGConfig()
	circuit := AllocateDAGCircuit(config)
	assignment := makeDAGDummyAssignment(config)
	assignment.RlcBeta[0] = big.NewInt(999)

	assert := test.NewAssert(t)
	assert.ProverFailed(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("DAG circuit correctly rejected invalid rlcBeta")
}

func TestDAGCircuitRejectsInvalidZDotR(t *testing.T) {
	config := smallDAGConfig()
	circuit := AllocateDAGCircuit(config)
	assignment := makeDAGDummyAssignment(config)
	assignment.ZDotR[0] = big.NewInt(999)

	assert := test.NewAssert(t)
	assert.ProverFailed(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("DAG circuit correctly rejected invalid zDotR")
}

func TestDAGCircuitRejectsInvalidMleEval(t *testing.T) {
	config := smallDAGConfig()
	circuit := AllocateDAGCircuit(config)
	assignment := makeDAGDummyAssignment(config)
	assignment.MleEval[0] = big.NewInt(999)

	assert := test.NewAssert(t)
	assert.ProverFailed(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("DAG circuit correctly rejected invalid mleEval")
}

// makeDAGDummyAssignment creates a self-consistent witness for the DAG circuit.
func makeDAGDummyAssignment(config DAGCircuitConfig) *DAGWrapperCircuit {
	nLayers := config.NumComputeLayers

	// Output challenges
	outputChallenges := make([]*big.Int, config.OutputNumVars)
	for i := range outputChallenges {
		outputChallenges[i] = big.NewInt(int64(7 + i*3))
	}

	type layerData struct {
		rlcCoeffs     []*big.Int
		bindings      []*big.Int
		gammas        []*big.Int
		rhos          []*big.Int
		podpChallenge *big.Int
		zVector       []*big.Int
	}

	layersData := make([]layerData, nLayers)
	for li := 0; li < nLayers; li++ {
		nv := config.LayerNumVars[li]
		nc := config.LayerNumClaims[li]
		offset := li * 20

		ld := layerData{
			rlcCoeffs:     make([]*big.Int, nc),
			bindings:      make([]*big.Int, nv),
			gammas:        make([]*big.Int, nv),
			rhos:          make([]*big.Int, nv+1),
			podpChallenge: big.NewInt(int64(23 + offset)),
		}
		for i := 0; i < nc; i++ {
			ld.rlcCoeffs[i] = big.NewInt(int64(3 + i*5 + offset))
		}
		for i := 0; i < nv; i++ {
			ld.bindings[i] = big.NewInt(int64(11 + i*7 + offset))
			ld.gammas[i] = big.NewInt(int64(19 + i*5 + offset))
		}
		for i := 0; i <= nv; i++ {
			ld.rhos[i] = big.NewInt(int64(13 + i*4 + offset))
		}
		zLen := 0
		if nv > 0 {
			zLen = (config.LayerDegrees[li] + 1) * nv
		}
		ld.zVector = make([]*big.Int, zLen)
		for i := 0; i < zLen; i++ {
			ld.zVector[i] = big.NewInt(int64(100 + i*100 + offset*10))
		}
		layersData[li] = ld
	}

	// Native point resolver for compute layer templates
	resolveNativePoint := func(template []int, sourceLayer int) []*big.Int {
		point := make([]*big.Int, len(template))
		for i, entry := range template {
			if entry < 1000 {
				if sourceLayer == -1 {
					point[i] = outputChallenges[entry]
				} else {
					point[i] = layersData[sourceLayer].bindings[entry]
				}
			} else if entry >= FIXED_REF_BASE {
				point[i] = big.NewInt(int64(entry - FIXED_REF_BASE))
			}
		}
		return point
	}

	// Compute per-layer rlcBeta
	rlcBetas := make([]*big.Int, nLayers)
	for i := 0; i < nLayers; i++ {
		rlcBeta := big.NewInt(0)
		for k := 0; k < config.LayerNumClaims[i]; k++ {
			claimPoint := resolveNativePoint(
				config.PointTemplates[i][k],
				config.PointTemplateSources[i][k],
			)
			beta := nativeComputeBeta(layersData[i].bindings, claimPoint)
			term := mulMod(beta, layersData[i].rlcCoeffs[k])
			rlcBeta = addMod(rlcBeta, term)
		}
		rlcBetas[i] = rlcBeta
	}

	// Compute per-layer zDotJStar
	zDotJStars := make([]*big.Int, nLayers)
	for i := 0; i < nLayers; i++ {
		nv := config.LayerNumVars[i]
		if nv > 0 {
			jStar := nativeComputeJStar(layersData[i].rhos, layersData[i].gammas,
				layersData[i].bindings, config.LayerDegrees[i])
			zDotJStars[i] = nativeInnerProduct(layersData[i].zVector, jStar)
		} else {
			zDotJStars[i] = big.NewInt(0)
		}
	}

	// Input group data — use explicit L/R bindings
	inputGroupRLCCoeffs := make([][]*big.Int, config.NumInputGroups)
	inputGroupPODPChallenges := make([]*big.Int, config.NumInputGroups)
	inputGroupZVectors := make([][]*big.Int, config.NumInputGroups)
	inputGroupLBindings := make([][]*big.Int, config.NumInputGroups)
	inputGroupRBindings := make([][]*big.Int, config.NumInputGroups)
	lTensorFlat := make([]*big.Int, 0)
	zDotRs := make([]*big.Int, config.NumInputGroups)

	for g := 0; g < config.NumInputGroups; g++ {
		nc := config.InputGroupNumClaims[g]
		lHalfLen := config.InputGroupLHalfLen[g]
		rHalfLen := config.InputGroupRHalfLen[g]

		inputGroupRLCCoeffs[g] = make([]*big.Int, nc)
		for i := 0; i < nc; i++ {
			inputGroupRLCCoeffs[g][i] = big.NewInt(int64(53 + g*10 + i*3))
		}
		inputGroupPODPChallenges[g] = big.NewInt(int64(61 + g*7))

		rTensorLen := 1 << rHalfLen
		inputGroupZVectors[g] = make([]*big.Int, rTensorLen)
		for i := 0; i < rTensorLen; i++ {
			inputGroupZVectors[g][i] = big.NewInt(int64(700 + g*100 + i*50))
		}

		// Generate explicit L-half and R-half bindings
		inputGroupLBindings[g] = make([]*big.Int, lHalfLen)
		for i := 0; i < lHalfLen; i++ {
			inputGroupLBindings[g][i] = big.NewInt(int64(31 + g*17 + i*11))
		}
		inputGroupRBindings[g] = make([]*big.Int, rHalfLen)
		for i := 0; i < rHalfLen; i++ {
			inputGroupRBindings[g][i] = big.NewInt(int64(37 + g*13 + i*7))
		}

		// L-tensor: for each claim, scale tensor product by RLC coeff
		lPerShred := nativeComputeTensorProduct(inputGroupLBindings[g])
		for ci := 0; ci < nc; ci++ {
			for _, tv := range lPerShred {
				lTensorFlat = append(lTensorFlat, mulMod(inputGroupRLCCoeffs[g][ci], tv))
			}
		}

		// R-tensor and zDotR
		rTensor := nativeComputeTensorProduct(inputGroupRBindings[g])
		zDotRs[g] = nativeInnerProduct(inputGroupZVectors[g], rTensor)

		_ = lHalfLen
	}

	// Public input values
	pubInputs := make([]*big.Int, config.PubInputCount)
	for i := range pubInputs {
		pubInputs[i] = big.NewInt(int64(6 + i*14))
	}

	// Public claim points (explicit)
	pubClaimPoints := make([][]*big.Int, config.NumPublicClaims)
	for p := 0; p < config.NumPublicClaims; p++ {
		nv := config.PubClaimNumVars[p]
		pubClaimPoints[p] = make([]*big.Int, nv)
		for i := 0; i < nv; i++ {
			pubClaimPoints[p][i] = big.NewInt(int64(41 + p*19 + i*13))
		}
	}

	// MLE evals
	mleEvals := make([]*big.Int, config.NumPublicClaims)
	for p := 0; p < config.NumPublicClaims; p++ {
		mleEvals[p] = nativeEvaluateMLE(pubInputs, pubClaimPoints[p])
	}

	// Build assignment
	assignment := AllocateDAGCircuit(config)
	assignment.CircuitHash = [2]frontend.Variable{big.NewInt(12345), big.NewInt(67890)}

	for i := range outputChallenges {
		assignment.OutputChallenges[i] = outputChallenges[i]
	}

	for li := 0; li < nLayers; li++ {
		ld := layersData[li]
		for i := 0; i < config.LayerNumClaims[li]; i++ {
			assignment.Layers[li].RlcCoeffs[i] = ld.rlcCoeffs[i]
		}
		nv := config.LayerNumVars[li]
		for i := 0; i < nv; i++ {
			assignment.Layers[li].Bindings[i] = ld.bindings[i]
			assignment.Layers[li].Gammas[i] = ld.gammas[i]
		}
		for i := 0; i <= nv; i++ {
			assignment.Layers[li].Rhos[i] = ld.rhos[i]
		}
		assignment.Layers[li].PODPChallenge = ld.podpChallenge
		if config.HasPoP(li) {
			assignment.Layers[li].PopChallenges[0] = big.NewInt(47)
		}

		zVars := make([]frontend.Variable, len(ld.zVector))
		for i, v := range ld.zVector {
			zVars[i] = v
		}
		assignment.LayerPODPs[li] = PODPWitness{
			ZVector: zVars,
			ZDelta:  big.NewInt(int64(400 + li*10)),
			ZBeta:   big.NewInt(int64(500 + li*10)),
		}
		assignment.LayerPops[li] = PopWitness{
			Z1: big.NewInt(1001), Z2: big.NewInt(1002),
			Z3: big.NewInt(1003), Z4: big.NewInt(1004), Z5: big.NewInt(1005),
		}
	}

	// Input groups
	for g := 0; g < config.NumInputGroups; g++ {
		for i := 0; i < config.InputGroupNumClaims[g]; i++ {
			assignment.InputGroups[g].RlcCoeffs[i] = inputGroupRLCCoeffs[g][i]
		}
		assignment.InputGroups[g].PODPChallenge = inputGroupPODPChallenges[g]

		for i := 0; i < config.InputGroupLHalfLen[g]; i++ {
			assignment.InputGroups[g].LBindings[i] = inputGroupLBindings[g][i]
		}
		for i := 0; i < config.InputGroupRHalfLen[g]; i++ {
			assignment.InputGroups[g].RBindings[i] = inputGroupRBindings[g][i]
		}

		zVars := make([]frontend.Variable, len(inputGroupZVectors[g]))
		for i, v := range inputGroupZVectors[g] {
			zVars[i] = v
		}
		assignment.InputGroupPODPs[g] = PODPWitness{
			ZVector: zVars,
			ZDelta:  big.NewInt(int64(900 + g*10)),
			ZBeta:   big.NewInt(int64(1000 + g*10)),
		}
	}

	// Public claim points
	for p := 0; p < config.NumPublicClaims; p++ {
		for i := 0; i < config.PubClaimNumVars[p]; i++ {
			assignment.PubClaimPoints[p][i] = pubClaimPoints[p][i]
		}
	}

	// Public outputs
	for i := 0; i < nLayers; i++ {
		assignment.RlcBeta[i] = rlcBetas[i]
		assignment.ZDotJStar[i] = zDotJStars[i]
	}
	for i := range lTensorFlat {
		assignment.LTensorFlat[i] = lTensorFlat[i]
	}
	for g := 0; g < config.NumInputGroups; g++ {
		assignment.ZDotR[g] = zDotRs[g]
	}
	for p := 0; p < config.NumPublicClaims; p++ {
		assignment.MleEval[p] = mleEvals[p]
	}

	// Public input values
	for i := range pubInputs {
		assignment.PublicInputValues[i] = pubInputs[i]
	}

	return assignment
}

// nativeComputeBeta computes beta(bindings, claimPoint) without RLC scaling.
func nativeComputeBeta(bindings, claimPoint []*big.Int) *big.Int {
	n := len(bindings)
	if n > len(claimPoint) {
		n = len(claimPoint)
	}
	beta := big.NewInt(1)
	for i := 0; i < n; i++ {
		rc := mulMod(bindings[i], claimPoint[i])
		oneMinusR := subMod(big.NewInt(1), bindings[i])
		oneMinusC := subMod(big.NewInt(1), claimPoint[i])
		oneMinusTerm := mulMod(oneMinusR, oneMinusC)
		term := addMod(rc, oneMinusTerm)
		beta = mulMod(beta, term)
	}
	return beta
}
