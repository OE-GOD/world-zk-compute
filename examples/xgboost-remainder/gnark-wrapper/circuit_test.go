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

// ============================================================
// Small config tests (regression)
// ============================================================

func TestSmallCircuitCompiles(t *testing.T) {
	circuit := AllocateCircuit(SmallConfig())
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		t.Fatalf("circuit compilation failed: %v", err)
	}
	t.Logf("SmallConfig circuit: %d constraints", ccs.GetNbConstraints())
}

func TestSmallCircuitProves(t *testing.T) {
	config := SmallConfig()
	circuit := AllocateCircuit(config)
	assignment := makeDummyAssignment(config)

	assert := test.NewAssert(t)
	assert.ProverSucceeded(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("SmallConfig Groth16 proof verified successfully")
}

func TestSmallCircuitRejectsInvalidWitness(t *testing.T) {
	config := SmallConfig()
	circuit := AllocateCircuit(config)
	assignment := makeDummyAssignment(config)
	assignment.RlcBeta[0] = big.NewInt(999)

	assert := test.NewAssert(t)
	assert.ProverFailed(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("Circuit correctly rejected invalid witness")
}

func TestSmallCircuitRejectsWrongInnerProduct(t *testing.T) {
	config := SmallConfig()
	circuit := AllocateCircuit(config)
	assignment := makeDummyAssignment(config)
	assignment.ZDotJStar[0] = big.NewInt(12345)

	assert := test.NewAssert(t)
	assert.ProverFailed(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("Circuit correctly rejected wrong inner product")
}

// ============================================================
// Medium config tests
// ============================================================

func TestMediumCircuitCompiles(t *testing.T) {
	circuit := AllocateCircuit(MediumConfig())
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		t.Fatalf("circuit compilation failed: %v", err)
	}
	t.Logf("MediumConfig circuit: %d constraints", ccs.GetNbConstraints())
}

func TestMediumCircuitProves(t *testing.T) {
	config := MediumConfig()
	circuit := AllocateCircuit(config)
	assignment := makeDummyAssignment(config)

	assert := test.NewAssert(t)
	assert.ProverSucceeded(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("MediumConfig Groth16 proof verified successfully")
}

func TestMediumCircuitRejectsInvalidWitness(t *testing.T) {
	config := MediumConfig()
	circuit := AllocateCircuit(config)
	assignment := makeDummyAssignment(config)
	assignment.RlcBeta[0] = big.NewInt(999)

	assert := test.NewAssert(t)
	assert.ProverFailed(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("MediumConfig correctly rejected invalid witness")
}

// ============================================================
// makeDummyAssignment: self-consistent witness for any config
// ============================================================

func makeDummyAssignment(config CircuitConfig) *RemainderWrapperCircuit {
	numPub := config.PubInputCount
	nLayers := config.NumLayers
	leftDims := config.LeftDims()
	rightDims := config.RightDims()
	numRTensor := 1 << rightDims
	numLTensor := config.NumLTensorElems()

	// Output challenges (OutputNumVars values)
	outputChallenges := make([]*big.Int, config.OutputNumVars)
	for i := 0; i < config.OutputNumVars; i++ {
		outputChallenges[i] = big.NewInt(int64(7 + i*3))
	}

	claimAggCoeff := big.NewInt(3)

	// Per-layer test data
	type layerData struct {
		bindings      []*big.Int
		gammas        []*big.Int
		rhos          []*big.Int
		podpChallenge *big.Int
		popChallenge  *big.Int
		zVector       []*big.Int
		zDelta        *big.Int
		zBeta         *big.Int
	}

	layersData := make([]layerData, nLayers)
	for li := 0; li < nLayers; li++ {
		nv := config.LayerNumVars[li]
		offset := li * 18 // spread values apart per layer
		ld := layerData{
			bindings:      make([]*big.Int, nv),
			gammas:        make([]*big.Int, nv),
			rhos:          make([]*big.Int, nv+1),
			podpChallenge: big.NewInt(int64(23 + offset)),
		}
		for i := 0; i < nv; i++ {
			ld.bindings[i] = big.NewInt(int64(11 + i*7 + offset))
			ld.gammas[i] = big.NewInt(int64(19 + i*5 + offset))
		}
		for i := 0; i <= nv; i++ {
			ld.rhos[i] = big.NewInt(int64(13 + i*4 + offset))
		}
		if config.HasPoP(li) {
			ld.popChallenge = big.NewInt(int64(47 + offset))
		}
		zLen := (config.LayerDegrees[li] + 1) * nv
		ld.zVector = make([]*big.Int, zLen)
		for i := 0; i < zLen; i++ {
			ld.zVector[i] = big.NewInt(int64(100 + i*100 + offset*10))
		}
		ld.zDelta = big.NewInt(int64(400 + offset*10))
		ld.zBeta = big.NewInt(int64(500 + offset*10))
		layersData[li] = ld
	}

	// Inter-layer coefficients (L-1 values)
	interLayerCoeffs := make([]*big.Int, 0)
	if nLayers > 1 {
		interLayerCoeffs = make([]*big.Int, nLayers-1)
		for i := 0; i < nLayers-1; i++ {
			interLayerCoeffs[i] = big.NewInt(int64(5 + i*3))
		}
	}

	// Input layer
	inputRLCCoeff0 := big.NewInt(53)
	inputRLCCoeff1 := big.NewInt(59)
	inputPODPChallenge := big.NewInt(61)

	// Input z_vector: 2^rightDims elements
	inputZ := make([]*big.Int, numRTensor)
	for i := 0; i < numRTensor; i++ {
		inputZ[i] = big.NewInt(int64(700 + i*100))
	}
	inputZDelta := big.NewInt(900)
	inputZBeta := big.NewInt(1000)

	// PoP witness (arbitrary - EC checks are on-chain)
	popZ1 := big.NewInt(1001)
	popZ2 := big.NewInt(1002)
	popZ3 := big.NewInt(1003)
	popZ4 := big.NewInt(1004)
	popZ5 := big.NewInt(1005)

	// Public inputs (PubInputCount values)
	pubInputs := make([]*big.Int, numPub)
	for i := 0; i < numPub; i++ {
		pubInputs[i] = big.NewInt(int64(6 + i*14))
	}

	// === Compute expected public outputs natively ===

	// Per-layer: rlcBeta and zDotJStar
	rlcBetas := make([]*big.Int, nLayers)
	zDotJStars := make([]*big.Int, nLayers)
	for i := 0; i < nLayers; i++ {
		var claimPoint []*big.Int
		var coeff *big.Int
		if i == 0 {
			claimPoint = outputChallenges
			coeff = claimAggCoeff
		} else {
			claimPoint = layersData[i-1].bindings
			coeff = interLayerCoeffs[i-1]
		}
		rlcBetas[i] = nativeComputeRlcBeta(layersData[i].bindings, claimPoint, coeff)

		jStar := nativeComputeJStar(layersData[i].rhos, layersData[i].gammas,
			layersData[i].bindings, config.LayerDegrees[i])
		zDotJStars[i] = nativeInnerProduct(layersData[i].zVector, jStar)
	}

	// L-tensor: uses LAST layer's bindings
	lastBindings := layersData[nLayers-1].bindings
	leftBindings := lastBindings[:leftDims]
	lPerShred := nativeComputeTensorProduct(leftBindings) // 2^leftDims elements
	lTensor := make([]*big.Int, numLTensor)
	lPerShredLen := len(lPerShred)
	for i := 0; i < lPerShredLen; i++ {
		lTensor[i] = mulMod(inputRLCCoeff0, lPerShred[i])
		lTensor[lPerShredLen+i] = mulMod(inputRLCCoeff1, lPerShred[i])
	}

	// R-tensor from right-half bindings of LAST layer
	rightBindings := lastBindings[leftDims:]
	rTensor := nativeComputeTensorProduct(rightBindings)

	// <input_z, R_tensor>
	zDotR := nativeInnerProduct(inputZ, rTensor)

	// MLE eval: uses FIRST layer's bindings
	mleEval := nativeEvaluateMLE(pubInputs, layersData[0].bindings)

	// === Build assignment ===
	assignment := AllocateCircuit(config)

	assignment.CircuitHash = [2]frontend.Variable{big.NewInt(12345), big.NewInt(67890)}
	for i := 0; i < numPub; i++ {
		assignment.PublicInputs[i] = pubInputs[i]
	}
	for i := 0; i < config.OutputNumVars; i++ {
		assignment.OutputChallenges[i] = outputChallenges[i]
	}
	assignment.ClaimAggCoeff = claimAggCoeff

	// Per-layer public inputs (each layer has its own num_vars)
	for li := 0; li < nLayers; li++ {
		nv := config.LayerNumVars[li]
		ld := layersData[li]
		for i := 0; i < nv; i++ {
			assignment.Layers[li].Bindings[i] = ld.bindings[i]
			assignment.Layers[li].Gammas[i] = ld.gammas[i]
		}
		for i := 0; i <= nv; i++ {
			assignment.Layers[li].Rhos[i] = ld.rhos[i]
		}
		assignment.Layers[li].PODPChallenge = ld.podpChallenge
		if config.HasPoP(li) {
			assignment.Layers[li].PopChallenge[0] = ld.popChallenge
		}
	}

	assignment.InputRLCCoeffs = [2]frontend.Variable{inputRLCCoeff0, inputRLCCoeff1}
	assignment.InputPODPChallenge = inputPODPChallenge

	// Inter-layer coefficients
	for i := 0; i < len(interLayerCoeffs); i++ {
		assignment.InterLayerCoeffs[i] = interLayerCoeffs[i]
	}

	// Public outputs
	for i := 0; i < nLayers; i++ {
		assignment.RlcBeta[i] = rlcBetas[i]
		assignment.ZDotJStar[i] = zDotJStars[i]
	}
	for i := 0; i < numLTensor; i++ {
		assignment.LTensor[i] = lTensor[i]
	}
	assignment.ZDotR = zDotR
	assignment.MLEEval = mleEval

	// Private witnesses
	for li := 0; li < nLayers; li++ {
		ld := layersData[li]
		zVars := make([]frontend.Variable, len(ld.zVector))
		for i, v := range ld.zVector {
			zVars[i] = v
		}
		assignment.LayerPODPs[li] = PODPWitness{ZVector: zVars, ZDelta: ld.zDelta, ZBeta: ld.zBeta}

		// Always set PopWitness (gnark requires all private fields to have values)
		assignment.LayerPops[li] = PopWitness{Z1: popZ1, Z2: popZ2, Z3: popZ3, Z4: popZ4, Z5: popZ5}
	}

	inputZVars := make([]frontend.Variable, numRTensor)
	for i, v := range inputZ {
		inputZVars[i] = v
	}
	assignment.InputPODP = PODPWitness{ZVector: inputZVars, ZDelta: inputZDelta, ZBeta: inputZBeta}

	_ = rightDims
	return assignment
}

// ---- Native Fr arithmetic helpers ----

func mulMod(a, b *big.Int) *big.Int {
	r := new(big.Int).Mul(a, b)
	return r.Mod(r, frMod)
}

func addMod(a, b *big.Int) *big.Int {
	r := new(big.Int).Add(a, b)
	return r.Mod(r, frMod)
}

func subMod(a, b *big.Int) *big.Int {
	r := new(big.Int).Sub(a, b)
	return r.Mod(r, frMod)
}

func nativeModInverse(a, m *big.Int) *big.Int {
	return new(big.Int).ModInverse(a, m)
}

func nativeComputeJStar(rhos, gammas, bindings []*big.Int, degree int) []*big.Int {
	n := len(bindings)
	degreePlusOne := degree + 1
	jStar := make([]*big.Int, degreePlusOne*n)

	for i := 0; i < n; i++ {
		gammaInv := nativeModInverse(gammas[i], frMod)
		bindingPower := big.NewInt(1)

		for d := 0; d < degreePlusOne; d++ {
			var coeff *big.Int
			if d == 0 {
				coeff = big.NewInt(2)
			} else {
				coeff = big.NewInt(1)
			}

			term1 := mulMod(rhos[i], coeff)
			term2 := mulMod(rhos[i+1], bindingPower)
			diff := subMod(term1, term2)
			jStar[i*degreePlusOne+d] = mulMod(gammaInv, diff)

			bindingPower = mulMod(bindingPower, bindings[i])
		}
	}

	return jStar
}

func nativeComputeRlcBeta(bindings, claimPoint []*big.Int, randomCoeff *big.Int) *big.Int {
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

	return mulMod(beta, randomCoeff)
}

func nativeInnerProduct(a, b []*big.Int) *big.Int {
	result := big.NewInt(0)
	for i := 0; i < len(a); i++ {
		term := mulMod(a[i], b[i])
		result = addMod(result, term)
	}
	return result
}

func nativeComputeTensorProduct(bindings []*big.Int) []*big.Int {
	result := []*big.Int{big.NewInt(1)}
	for _, b := range bindings {
		oneMinusB := subMod(big.NewInt(1), b)
		newResult := make([]*big.Int, len(result)*2)
		for j, r := range result {
			newResult[2*j] = mulMod(r, oneMinusB)
			newResult[2*j+1] = mulMod(r, b)
		}
		result = newResult
	}
	return result
}

func nativeEvaluateMLE(values []*big.Int, point []*big.Int) *big.Int {
	basis := nativeComputeTensorProduct(point)
	return nativeInnerProduct(values, basis)
}

// ============================================================
// Variable num_vars tests (per-layer parameterization)
// ============================================================

func TestVariableNumVarsCircuitCompiles(t *testing.T) {
	config := CircuitConfig{
		NumLayers:     3,
		LayerNumVars:  []int{2, 3, 1},
		LayerDegrees:  []int{2, 2, 3},
		OutputNumVars: 2,
		PubInputCount: 4,
	}
	circuit := AllocateCircuit(config)
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		t.Fatalf("circuit compilation failed: %v", err)
	}
	t.Logf("VariableNumVars circuit: %d constraints, expected pub inputs: %d",
		ccs.GetNbConstraints(), config.ExpectedPublicInputCount())
}

func TestVariableNumVarsCircuitProves(t *testing.T) {
	config := CircuitConfig{
		NumLayers:     3,
		LayerNumVars:  []int{2, 3, 1},
		LayerDegrees:  []int{2, 2, 3},
		OutputNumVars: 2,
		PubInputCount: 4,
	}
	circuit := AllocateCircuit(config)
	assignment := makeDummyAssignment(config)

	assert := test.NewAssert(t)
	assert.ProverSucceeded(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("VariableNumVars Groth16 proof verified successfully")
}

func TestVariableNumVarsCircuitRejectsInvalidWitness(t *testing.T) {
	config := CircuitConfig{
		NumLayers:     3,
		LayerNumVars:  []int{2, 3, 1},
		LayerDegrees:  []int{2, 2, 3},
		OutputNumVars: 2,
		PubInputCount: 4,
	}
	circuit := AllocateCircuit(config)
	assignment := makeDummyAssignment(config)
	assignment.RlcBeta[1] = big.NewInt(999)

	assert := test.NewAssert(t)
	assert.ProverFailed(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("VariableNumVars correctly rejected invalid witness")
}
