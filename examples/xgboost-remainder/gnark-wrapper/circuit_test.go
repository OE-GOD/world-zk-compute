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

// TestCircuitCompiles verifies the wrapper circuit compiles and reports constraint count.
func TestCircuitCompiles(t *testing.T) {
	circuit := AllocateCircuit()
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		t.Fatalf("circuit compilation failed: %v", err)
	}
	t.Logf("RemainderWrapperCircuit: %d constraints", ccs.GetNbConstraints())
}

// TestCircuitProves verifies the circuit can produce a valid Groth16 proof with dummy witness.
func TestCircuitProves(t *testing.T) {
	circuit := AllocateCircuit()
	assignment := makeDummyAssignment()

	assert := test.NewAssert(t)
	assert.ProverSucceeded(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("RemainderWrapperCircuit Groth16 proof verified successfully")
}

// TestCircuitRejectsInvalidWitness verifies the circuit rejects an incorrect output.
func TestCircuitRejectsInvalidWitness(t *testing.T) {
	circuit := AllocateCircuit()
	assignment := makeDummyAssignment()
	// Corrupt a public output — the assertion should fail
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

// TestCircuitRejectsWrongInnerProduct verifies inner product constraint.
func TestCircuitRejectsWrongInnerProduct(t *testing.T) {
	circuit := AllocateCircuit()
	assignment := makeDummyAssignment()
	// Corrupt the z_dot_jstar output
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

// makeDummyAssignment creates a self-consistent witness for testing.
func makeDummyAssignment() *RemainderWrapperCircuit {
	// Pick challenge values
	outputChallenge := big.NewInt(7)
	claimAggCoeff := big.NewInt(3)
	interLayerCoeff := big.NewInt(5)

	// Layer 0 (subtract, degree=2)
	l0binding := big.NewInt(11)
	l0rho0 := big.NewInt(13)
	l0rho1 := big.NewInt(17)
	l0gamma0 := big.NewInt(19)
	l0podpChallenge := big.NewInt(23)

	// Layer 1 (multiply, degree=3)
	l1binding := big.NewInt(29)
	l1rho0 := big.NewInt(31)
	l1rho1 := big.NewInt(37)
	l1gamma0 := big.NewInt(41)
	l1podpChallenge := big.NewInt(43)
	l1popChallenge := big.NewInt(47)

	// Input layer
	inputRLCCoeff0 := big.NewInt(53)
	inputRLCCoeff1 := big.NewInt(59)
	inputPODPChallenge := big.NewInt(61)

	// Pick z_vectors (arbitrary)
	l0z := []*big.Int{big.NewInt(100), big.NewInt(200), big.NewInt(300)}
	l0zDelta := big.NewInt(400)
	l0zBeta := big.NewInt(500)

	l1z := []*big.Int{big.NewInt(110), big.NewInt(220), big.NewInt(330), big.NewInt(440)}
	l1zDelta := big.NewInt(550)
	l1zBeta := big.NewInt(660)

	inputZ := []*big.Int{big.NewInt(700), big.NewInt(800)}
	inputZDelta := big.NewInt(900)
	inputZBeta := big.NewInt(1000)

	// PoP witness (arbitrary — EC checks are on-chain)
	popZ1 := big.NewInt(1001)
	popZ2 := big.NewInt(1002)
	popZ3 := big.NewInt(1003)
	popZ4 := big.NewInt(1004)
	popZ5 := big.NewInt(1005)

	// Public inputs
	pubInput0 := big.NewInt(6)
	pubInput1 := big.NewInt(20)

	// === Compute expected public outputs natively ===

	// rlc_beta_0 = beta([l0binding], [outputChallenge]) * claimAggCoeff
	rlcBeta0 := nativeComputeRlcBeta(
		[]*big.Int{l0binding},
		[]*big.Int{outputChallenge},
		claimAggCoeff,
	)

	// rlc_beta_1 = beta([l1binding], [l0binding]) * interLayerCoeff
	rlcBeta1 := nativeComputeRlcBeta(
		[]*big.Int{l1binding},
		[]*big.Int{l0binding},
		interLayerCoeff,
	)

	// j_star_0 and <z, j_star> for layer 0 (degree=2)
	jStar0 := nativeComputeJStar(
		[]*big.Int{l0rho0, l0rho1},
		[]*big.Int{l0gamma0},
		[]*big.Int{l0binding},
		2,
	)
	zDotJStar0 := nativeInnerProduct(l0z, jStar0)

	// j_star_1 and <z, j_star> for layer 1 (degree=3)
	jStar1 := nativeComputeJStar(
		[]*big.Int{l1rho0, l1rho1},
		[]*big.Int{l1gamma0},
		[]*big.Int{l1binding},
		3,
	)
	zDotJStar1 := nativeInnerProduct(l1z, jStar1)

	// L-tensor: [coeff0, coeff1]
	lTensor0 := new(big.Int).Set(inputRLCCoeff0)
	lTensor1 := new(big.Int).Set(inputRLCCoeff1)

	// R-tensor: [(1 - l1binding), l1binding]
	rCoeff0 := subMod(big.NewInt(1), l1binding)
	rCoeff1 := new(big.Int).Set(l1binding)

	// <input_z, R_tensor>
	zDotR := nativeInnerProduct(inputZ, []*big.Int{rCoeff0, rCoeff1})

	// MLE eval: (1-x)*pubInput0 + x*pubInput1, where x = l0binding
	oneMinusX := subMod(big.NewInt(1), l0binding)
	mleEval := addMod(mulMod(oneMinusX, pubInput0), mulMod(l0binding, pubInput1))

	return &RemainderWrapperCircuit{
		// Public inputs
		CircuitHash0:    big.NewInt(12345),
		CircuitHash1:    big.NewInt(67890),
		PublicInput0:    pubInput0,
		PublicInput1:    pubInput1,
		OutputChallenge: outputChallenge,
		ClaimAggCoeff:   claimAggCoeff,
		InterLayerCoeff: interLayerCoeff,

		Layer0Bindings:      [1]frontend.Variable{l0binding},
		Layer0Rhos:          [2]frontend.Variable{l0rho0, l0rho1},
		Layer0Gammas:        [1]frontend.Variable{l0gamma0},
		Layer0PODPChallenge: l0podpChallenge,

		Layer1Bindings:      [1]frontend.Variable{l1binding},
		Layer1Rhos:          [2]frontend.Variable{l1rho0, l1rho1},
		Layer1Gammas:        [1]frontend.Variable{l1gamma0},
		Layer1PODPChallenge: l1podpChallenge,
		Layer1PopChallenge:  l1popChallenge,

		InputRLCCoeff0:     inputRLCCoeff0,
		InputRLCCoeff1:     inputRLCCoeff1,
		InputPODPChallenge: inputPODPChallenge,

		// Public outputs
		RlcBeta0:   rlcBeta0,
		RlcBeta1:   rlcBeta1,
		ZDotJStar0: zDotJStar0,
		ZDotJStar1: zDotJStar1,
		LTensor0:   lTensor0,
		LTensor1:   lTensor1,
		ZDotR:      zDotR,
		MLEEval:    mleEval,

		// Private witnesses
		Layer0PODP: PODPWitness{
			ZVector: []frontend.Variable{l0z[0], l0z[1], l0z[2]},
			ZDelta:  l0zDelta,
			ZBeta:   l0zBeta,
		},
		Layer1PODP: PODPWitness{
			ZVector: []frontend.Variable{l1z[0], l1z[1], l1z[2], l1z[3]},
			ZDelta:  l1zDelta,
			ZBeta:   l1zBeta,
		},
		Layer1Pop: PopWitness{
			Z1: popZ1,
			Z2: popZ2,
			Z3: popZ3,
			Z4: popZ4,
			Z5: popZ5,
		},
		InputPODP: PODPWitness{
			ZVector: []frontend.Variable{inputZ[0], inputZ[1]},
			ZDelta:  inputZDelta,
			ZBeta:   inputZBeta,
		},
	}
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
