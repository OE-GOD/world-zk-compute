package main

import (
	"github.com/consensys/gnark/frontend"
)

// CircuitConfig defines the circuit dimensions for a 2-layer GKR circuit.
// The number of GKR layers is always 2 (subtract + multiply); only the WIDTH
// (bindings per layer = num_vars) changes.
type CircuitConfig struct {
	NumVars         int // bindings per GKR layer
	NumPublicInputs int // 2^NumVars public input values
	Layer0Degree    int // subtract gate degree = 2
	Layer1Degree    int // multiply gate degree = 3
}

// LeftDims returns floor(NumVars/2) — the number of left-half bindings for
// the Hyrax input layer matrix split.
func (c CircuitConfig) LeftDims() int {
	return c.NumVars / 2
}

// RightDims returns ceil(NumVars/2) — the number of right-half bindings.
func (c CircuitConfig) RightDims() int {
	return (c.NumVars + 1) / 2
}

// NumRTensorElems returns 2^RightDims — the size of the R-tensor and input PODP z_vector.
func (c CircuitConfig) NumRTensorElems() int {
	return 1 << c.RightDims()
}

// NumLTensorElems returns 2 * 2^LeftDims — the size of the combined L-tensor (2 shreds).
func (c CircuitConfig) NumLTensorElems() int {
	return 2 * (1 << c.LeftDims())
}

// SmallConfig returns the config for the toy circuit (num_vars=1, 2 public inputs).
func SmallConfig() CircuitConfig {
	return CircuitConfig{NumVars: 1, NumPublicInputs: 2, Layer0Degree: 2, Layer1Degree: 3}
}

// MediumConfig returns the config for medium XGBoost models (num_vars=4, 16 public inputs).
func MediumConfig() CircuitConfig {
	return CircuitConfig{NumVars: 4, NumPublicInputs: 16, Layer0Degree: 2, Layer1Degree: 3}
}

// PODPWitness holds private witness data for a PODP verification.
type PODPWitness struct {
	ZVector []frontend.Variable
	ZDelta  frontend.Variable
	ZBeta   frontend.Variable
}

// PopWitness holds private witness data for a ProofOfProduct.
type PopWitness struct {
	Z1 frontend.Variable
	Z2 frontend.Variable
	Z3 frontend.Variable
	Z4 frontend.Variable
	Z5 frontend.Variable
}

// RemainderWrapperCircuit wraps GKR algebraic verification inside a Groth16 SNARK.
//
// Option C design: Poseidon transcript replay stays on-chain (Fq). This circuit
// takes ALL Fiat-Shamir challenges as public inputs, computes Fr-based algebraic
// relations, and exposes the scalar results as public outputs for on-chain EC checks.
//
// Parameterized for num_vars: 2 GKR layers (subtract + multiply), width = 2^num_vars.
// The Hyrax input layer splits num_vars into left (floor(N/2)) and right (ceil(N/2)) halves.
//
// Public input count: 7*N + 2^N + 18 + 2*2^floor(N/2)
//   - Small  (N=1): 7+2+18+2 = 29
//   - Medium (N=4): 28+16+18+8 = 70
type RemainderWrapperCircuit struct {
	// Config (not a circuit variable, used at compile time only)
	Config CircuitConfig `gnark:"-"`

	// === Public inputs (on-chain -> gnark, from Poseidon transcript) ===
	CircuitHash  [2]frontend.Variable `gnark:",public"`
	PublicInputs []frontend.Variable  `gnark:",public"` // 2^num_vars values

	// Output challenges (claim point for layer 0) and claim aggregation
	OutputChallenges []frontend.Variable `gnark:",public"` // num_vars values
	ClaimAggCoeff    frontend.Variable   `gnark:",public"`

	// Layer 0 (subtract): num_vars bindings, num_vars+1 rhos, num_vars gammas
	Layer0Bindings      []frontend.Variable `gnark:",public"`
	Layer0Rhos          []frontend.Variable `gnark:",public"`
	Layer0Gammas        []frontend.Variable `gnark:",public"`
	Layer0PODPChallenge frontend.Variable   `gnark:",public"`

	// Layer 1 (multiply): num_vars bindings, num_vars+1 rhos, num_vars gammas
	Layer1Bindings      []frontend.Variable `gnark:",public"`
	Layer1Rhos          []frontend.Variable `gnark:",public"`
	Layer1Gammas        []frontend.Variable `gnark:",public"`
	Layer1PODPChallenge frontend.Variable   `gnark:",public"`
	Layer1PopChallenge  frontend.Variable   `gnark:",public"`

	// Input layer: RLC coefficients for multi-claim (always 2 shreds), PODP challenge
	InputRLCCoeffs     [2]frontend.Variable `gnark:",public"`
	InputPODPChallenge frontend.Variable    `gnark:",public"`

	// Inter-layer claim aggregation coefficient
	InterLayerCoeff frontend.Variable `gnark:",public"`

	// === Public outputs (gnark -> on-chain, used in EC equations) ===
	RlcBeta0   frontend.Variable `gnark:",public"`
	RlcBeta1   frontend.Variable `gnark:",public"`
	ZDotJStar0 frontend.Variable `gnark:",public"`
	ZDotJStar1 frontend.Variable `gnark:",public"`

	// L-tensor: 2 * 2^floor(num_vars/2) elements (2 shreds * left-half tensor product)
	LTensor []frontend.Variable `gnark:",public"`

	ZDotR   frontend.Variable `gnark:",public"`
	MLEEval frontend.Variable `gnark:",public"`

	// === Private witness (off-chain) ===
	Layer0PODP PODPWitness
	Layer1PODP PODPWitness
	Layer1Pop  PopWitness
	InputPODP  PODPWitness // z_vector has 2^ceil(num_vars/2) elements
}

// Define implements the gnark circuit interface.
func (c *RemainderWrapperCircuit) Define(api frontend.API) error {
	// ======================================================================
	// Layer 0 (subtract gate, degree=2)
	// ======================================================================

	// rlc_beta_0 = beta(layer0_bindings, output_challenges) * claimAggCoeff
	rlcBeta0 := computeRlcBeta(api, c.Layer0Bindings, c.OutputChallenges, c.ClaimAggCoeff)
	api.AssertIsEqual(rlcBeta0, c.RlcBeta0)

	// j_star and <z, j_star> for layer 0
	jStar0 := computeJStar(api, c.Layer0Rhos, c.Layer0Gammas, c.Layer0Bindings, c.Config.Layer0Degree)
	zDotJStar0 := innerProduct(api, c.Layer0PODP.ZVector, jStar0)
	api.AssertIsEqual(zDotJStar0, c.ZDotJStar0)

	// ======================================================================
	// Layer 1 (multiply gate, degree=3)
	// ======================================================================

	// rlc_beta_1 = beta(layer1_bindings, layer0_bindings) * interLayerCoeff
	rlcBeta1 := computeRlcBeta(api, c.Layer1Bindings, c.Layer0Bindings, c.InterLayerCoeff)
	api.AssertIsEqual(rlcBeta1, c.RlcBeta1)

	// j_star and <z, j_star> for layer 1
	jStar1 := computeJStar(api, c.Layer1Rhos, c.Layer1Gammas, c.Layer1Bindings, c.Config.Layer1Degree)
	zDotJStar1 := innerProduct(api, c.Layer1PODP.ZVector, jStar1)
	api.AssertIsEqual(zDotJStar1, c.ZDotJStar1)

	// ======================================================================
	// Input layer: Hyrax matrix split for L-tensor and R-tensor
	// ======================================================================
	// The Hyrax committed input (2 shreds x 2^num_vars values) uses a matrix layout:
	//   left_dims  = floor(num_vars/2) bindings -> L-tensor per shred
	//   right_dims = ceil(num_vars/2) bindings  -> R-tensor
	//
	// L-tensor = [coeff0 * tensor(left_bindings), coeff1 * tensor(left_bindings)]
	// R-tensor = tensor(right_bindings)
	// PODP z_vector has 2^right_dims elements

	leftDims := c.Config.LeftDims()

	// L-tensor: for each shred, scale tensor(left_bindings) by the RLC coefficient
	leftBindings := c.Layer1Bindings[:leftDims]
	lPerShred := computeTensorProduct(api, leftBindings) // 2^leftDims elements
	lPerShredLen := len(lPerShred)
	for i := 0; i < lPerShredLen; i++ {
		expected0 := api.Mul(c.InputRLCCoeffs[0], lPerShred[i])
		api.AssertIsEqual(expected0, c.LTensor[i])
		expected1 := api.Mul(c.InputRLCCoeffs[1], lPerShred[i])
		api.AssertIsEqual(expected1, c.LTensor[lPerShredLen+i])
	}

	// R-tensor from right-half bindings -> 2^rightDims elements
	rightBindings := c.Layer1Bindings[leftDims:]
	rTensor := computeTensorProduct(api, rightBindings)
	zDotR := innerProduct(api, c.InputPODP.ZVector, rTensor)
	api.AssertIsEqual(zDotR, c.ZDotR)

	// ======================================================================
	// Public input MLE evaluation
	// ======================================================================
	// MLE(publicInputs, layer0_bindings) via tensor product evaluation
	mleEval := evaluateMLE(api, c.PublicInputs, c.Layer0Bindings)
	api.AssertIsEqual(mleEval, c.MLEEval)

	return nil
}

// AllocateCircuit returns a circuit definition with correct slice sizes for the given config.
func AllocateCircuit(config CircuitConfig) *RemainderWrapperCircuit {
	nv := config.NumVars
	return &RemainderWrapperCircuit{
		Config:           config,
		PublicInputs:     make([]frontend.Variable, config.NumPublicInputs),
		OutputChallenges: make([]frontend.Variable, nv),
		Layer0Bindings:   make([]frontend.Variable, nv),
		Layer0Rhos:       make([]frontend.Variable, nv+1),
		Layer0Gammas:     make([]frontend.Variable, nv),
		Layer1Bindings:   make([]frontend.Variable, nv),
		Layer1Rhos:       make([]frontend.Variable, nv+1),
		Layer1Gammas:     make([]frontend.Variable, nv),
		LTensor:          make([]frontend.Variable, config.NumLTensorElems()),
		Layer0PODP:       PODPWitness{ZVector: make([]frontend.Variable, (config.Layer0Degree+1)*nv)},
		Layer1PODP:       PODPWitness{ZVector: make([]frontend.Variable, (config.Layer1Degree+1)*nv)},
		InputPODP:        PODPWitness{ZVector: make([]frontend.Variable, config.NumRTensorElems())},
	}
}

// computeTensorProduct computes tensor([b0, b1, ..., bn]) producing 2^n elements.
// Each element is a product of (1-b_i) or b_i terms for the corresponding binary index.
func computeTensorProduct(api frontend.API, bindings []frontend.Variable) []frontend.Variable {
	result := []frontend.Variable{frontend.Variable(1)}
	for _, b := range bindings {
		oneMinusB := api.Sub(1, b)
		newResult := make([]frontend.Variable, len(result)*2)
		for j, r := range result {
			newResult[2*j] = api.Mul(r, oneMinusB)
			newResult[2*j+1] = api.Mul(r, b)
		}
		result = newResult
	}
	return result
}

// evaluateMLE evaluates the multilinear extension of values at point.
// MLE(values, point) = sum_i values[i] * tensor(point)[i]
func evaluateMLE(api frontend.API, values []frontend.Variable, point []frontend.Variable) frontend.Variable {
	basis := computeTensorProduct(api, point)
	return innerProduct(api, values, basis)
}

// computeJStar computes the j_star vector for PODP verification.
// j_star[i*(degree+1) + d] = gamma_inv[i] * (rhos[i]*coeff[d] - rhos[i+1]*bindings[i]^d)
// where coeff[d] = 2 if d==0, else 1
func computeJStar(
	api frontend.API,
	rhos []frontend.Variable,
	gammas []frontend.Variable,
	bindings []frontend.Variable,
	degree int,
) []frontend.Variable {
	n := len(bindings)
	degreePlusOne := degree + 1
	jStar := make([]frontend.Variable, degreePlusOne*n)

	for i := 0; i < n; i++ {
		gammaInv := api.Inverse(gammas[i])
		bindingPower := frontend.Variable(1)

		for d := 0; d < degreePlusOne; d++ {
			var coeff frontend.Variable
			if d == 0 {
				coeff = frontend.Variable(2)
			} else {
				coeff = frontend.Variable(1)
			}

			term1 := api.Mul(rhos[i], coeff)
			term2 := api.Mul(rhos[i+1], bindingPower)
			diff := api.Sub(term1, term2)
			jStar[i*degreePlusOne+d] = api.Mul(gammaInv, diff)

			bindingPower = api.Mul(bindingPower, bindings[i])
		}
	}

	return jStar
}

// computeRlcBeta computes rlc_beta = beta(bindings, claimPoint) * randomCoeff
// beta(r, c) = prod_i (r_i * c_i + (1 - r_i) * (1 - c_i))
func computeRlcBeta(
	api frontend.API,
	bindings []frontend.Variable,
	claimPoint []frontend.Variable,
	randomCoeff frontend.Variable,
) frontend.Variable {
	n := len(bindings)
	if n > len(claimPoint) {
		n = len(claimPoint)
	}

	beta := frontend.Variable(1)
	for i := 0; i < n; i++ {
		rc := api.Mul(bindings[i], claimPoint[i])
		oneMinusR := api.Sub(1, bindings[i])
		oneMinusC := api.Sub(1, claimPoint[i])
		oneMinusTerm := api.Mul(oneMinusR, oneMinusC)
		term := api.Add(rc, oneMinusTerm)
		beta = api.Mul(beta, term)
	}

	return api.Mul(beta, randomCoeff)
}

// innerProduct computes <a, b> = sum(a_i * b_i) over Fr.
func innerProduct(api frontend.API, a, b []frontend.Variable) frontend.Variable {
	if len(a) != len(b) {
		panic("innerProduct: length mismatch")
	}
	result := frontend.Variable(0)
	for i := 0; i < len(a); i++ {
		term := api.Mul(a[i], b[i])
		result = api.Add(result, term)
	}
	return result
}
