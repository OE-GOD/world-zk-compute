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
	assignment.RlcBeta0 = big.NewInt(999)

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
	assignment.ZDotJStar0 = big.NewInt(12345)

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
	assignment.RlcBeta0 = big.NewInt(999)

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
	nv := config.NumVars
	numPub := config.NumPublicInputs // 2^nv

	// Output challenges (num_vars values)
	outputChallenges := make([]*big.Int, nv)
	for i := 0; i < nv; i++ {
		outputChallenges[i] = big.NewInt(int64(7 + i*3))
	}

	claimAggCoeff := big.NewInt(3)
	interLayerCoeff := big.NewInt(5)

	// Layer 0 (subtract, degree=2)
	l0bindings := make([]*big.Int, nv)
	l0gammas := make([]*big.Int, nv)
	l0rhos := make([]*big.Int, nv+1)
	for i := 0; i < nv; i++ {
		l0bindings[i] = big.NewInt(int64(11 + i*7))
		l0gammas[i] = big.NewInt(int64(19 + i*5))
	}
	for i := 0; i <= nv; i++ {
		l0rhos[i] = big.NewInt(int64(13 + i*4))
	}
	l0podpChallenge := big.NewInt(23)

	// Layer 1 (multiply, degree=3)
	l1bindings := make([]*big.Int, nv)
	l1gammas := make([]*big.Int, nv)
	l1rhos := make([]*big.Int, nv+1)
	for i := 0; i < nv; i++ {
		l1bindings[i] = big.NewInt(int64(29 + i*11))
		l1gammas[i] = big.NewInt(int64(41 + i*13))
	}
	for i := 0; i <= nv; i++ {
		l1rhos[i] = big.NewInt(int64(31 + i*6))
	}
	l1podpChallenge := big.NewInt(43)
	l1popChallenge := big.NewInt(47)

	// Input layer
	inputRLCCoeff0 := big.NewInt(53)
	inputRLCCoeff1 := big.NewInt(59)
	inputPODPChallenge := big.NewInt(61)

	// Layer 0 z_vector: (degree+1)*nv elements
	l0zLen := (config.Layer0Degree + 1) * nv
	l0z := make([]*big.Int, l0zLen)
	for i := 0; i < l0zLen; i++ {
		l0z[i] = big.NewInt(int64(100 + i*100))
	}
	l0zDelta := big.NewInt(400)
	l0zBeta := big.NewInt(500)

	// Layer 1 z_vector: (degree+1)*nv elements
	l1zLen := (config.Layer1Degree + 1) * nv
	l1z := make([]*big.Int, l1zLen)
	for i := 0; i < l1zLen; i++ {
		l1z[i] = big.NewInt(int64(110 + i*110))
	}
	l1zDelta := big.NewInt(550)
	l1zBeta := big.NewInt(660)

	// Input z_vector: 2^nv elements
	inputZ := make([]*big.Int, numPub)
	for i := 0; i < numPub; i++ {
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

	// Public inputs (2^nv values)
	pubInputs := make([]*big.Int, numPub)
	for i := 0; i < numPub; i++ {
		pubInputs[i] = big.NewInt(int64(6 + i*14))
	}

	// === Compute expected public outputs natively ===

	// rlc_beta_0 = beta(l0_bindings, output_challenges) * claimAggCoeff
	rlcBeta0 := nativeComputeRlcBeta(l0bindings, outputChallenges, claimAggCoeff)

	// rlc_beta_1 = beta(l1_bindings, l0_bindings) * interLayerCoeff
	rlcBeta1 := nativeComputeRlcBeta(l1bindings, l0bindings, interLayerCoeff)

	// j_star_0 and <z, j_star> for layer 0 (degree=2)
	jStar0 := nativeComputeJStar(l0rhos, l0gammas, l0bindings, config.Layer0Degree)
	zDotJStar0 := nativeInnerProduct(l0z, jStar0)

	// j_star_1 and <z, j_star> for layer 1 (degree=3)
	jStar1 := nativeComputeJStar(l1rhos, l1gammas, l1bindings, config.Layer1Degree)
	zDotJStar1 := nativeInnerProduct(l1z, jStar1)

	// L-tensor: [coeff0, coeff1]
	lTensor0 := new(big.Int).Set(inputRLCCoeff0)
	lTensor1 := new(big.Int).Set(inputRLCCoeff1)

	// R-tensor: tensor(l1_bindings) -> 2^nv elements
	rTensor := nativeComputeTensorProduct(l1bindings)

	// <input_z, R_tensor>
	zDotR := nativeInnerProduct(inputZ, rTensor)

	// MLE eval: multilinear extension of pubInputs evaluated at l0_bindings
	mleEval := nativeEvaluateMLE(pubInputs, l0bindings)

	// === Build assignment ===
	assignment := AllocateCircuit(config)

	assignment.CircuitHash = [2]frontend.Variable{big.NewInt(12345), big.NewInt(67890)}
	for i := 0; i < numPub; i++ {
		assignment.PublicInputs[i] = pubInputs[i]
	}
	for i := 0; i < nv; i++ {
		assignment.OutputChallenges[i] = outputChallenges[i]
	}
	assignment.ClaimAggCoeff = claimAggCoeff
	assignment.InterLayerCoeff = interLayerCoeff

	for i := 0; i < nv; i++ {
		assignment.Layer0Bindings[i] = l0bindings[i]
		assignment.Layer0Gammas[i] = l0gammas[i]
	}
	for i := 0; i <= nv; i++ {
		assignment.Layer0Rhos[i] = l0rhos[i]
	}
	assignment.Layer0PODPChallenge = l0podpChallenge

	for i := 0; i < nv; i++ {
		assignment.Layer1Bindings[i] = l1bindings[i]
		assignment.Layer1Gammas[i] = l1gammas[i]
	}
	for i := 0; i <= nv; i++ {
		assignment.Layer1Rhos[i] = l1rhos[i]
	}
	assignment.Layer1PODPChallenge = l1podpChallenge
	assignment.Layer1PopChallenge = l1popChallenge

	assignment.InputRLCCoeffs = [2]frontend.Variable{inputRLCCoeff0, inputRLCCoeff1}
	assignment.InputPODPChallenge = inputPODPChallenge

	// Public outputs
	assignment.RlcBeta0 = rlcBeta0
	assignment.RlcBeta1 = rlcBeta1
	assignment.ZDotJStar0 = zDotJStar0
	assignment.ZDotJStar1 = zDotJStar1
	assignment.LTensor = [2]frontend.Variable{lTensor0, lTensor1}
	assignment.ZDotR = zDotR
	assignment.MLEEval = mleEval

	// Private witnesses
	l0zVars := make([]frontend.Variable, l0zLen)
	for i, v := range l0z {
		l0zVars[i] = v
	}
	assignment.Layer0PODP = PODPWitness{ZVector: l0zVars, ZDelta: l0zDelta, ZBeta: l0zBeta}

	l1zVars := make([]frontend.Variable, l1zLen)
	for i, v := range l1z {
		l1zVars[i] = v
	}
	assignment.Layer1PODP = PODPWitness{ZVector: l1zVars, ZDelta: l1zDelta, ZBeta: l1zBeta}

	assignment.Layer1Pop = PopWitness{Z1: popZ1, Z2: popZ2, Z3: popZ3, Z4: popZ4, Z5: popZ5}

	inputZVars := make([]frontend.Variable, numPub)
	for i, v := range inputZ {
		inputZVars[i] = v
	}
	assignment.InputPODP = PODPWitness{ZVector: inputZVars, ZDelta: inputZDelta, ZBeta: inputZBeta}

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
