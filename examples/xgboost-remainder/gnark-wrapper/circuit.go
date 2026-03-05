package main

import (
	"github.com/consensys/gnark/frontend"
)

// CircuitConfig defines the circuit dimensions for an N-layer GKR circuit.
// Supports per-layer num_vars (variable binding counts across layers).
type CircuitConfig struct {
	NumLayers     int   // number of computation layers
	LayerNumVars  []int // per-layer num_vars (binding count); len = NumLayers
	LayerDegrees  []int // degree of each layer; len = NumLayers
	OutputNumVars int   // output challenge count (0 for scalar output)
	PubInputCount int   // number of public input values (for MLE eval)
	InputNumVars  int   // num_vars for committed input layer (determines Hyrax tensor split)
}

// HasPoP returns true if the given layer has a ProofOfProduct challenge (degree > 2).
func (c CircuitConfig) HasPoP(layerIdx int) bool {
	return c.LayerDegrees[layerIdx] > 2
}

// LeftDims returns floor(InputNumVars/2) — the number of left-half bindings for
// the Hyrax input layer matrix split.
func (c CircuitConfig) LeftDims() int {
	return c.InputNumVars / 2
}

// RightDims returns ceil(InputNumVars/2) — the number of right-half bindings.
func (c CircuitConfig) RightDims() int {
	return (c.InputNumVars + 1) / 2
}

// NumRTensorElems returns 2^RightDims — the size of the R-tensor and input PODP z_vector.
func (c CircuitConfig) NumRTensorElems() int {
	return 1 << c.RightDims()
}

// NumLTensorElems returns 2 * 2^LeftDims — the size of the combined L-tensor (2 shreds).
func (c CircuitConfig) NumLTensorElems() int {
	return 2 * (1 << c.LeftDims())
}

// MleEvalNumVars returns log2(PubInputCount) — the number of variables for MLE eval.
func (c CircuitConfig) MleEvalNumVars() int {
	n := c.PubInputCount
	bits := 0
	for n > 1 {
		n >>= 1
		bits++
	}
	return bits
}

// NumPopLayers returns the count of layers that have a PoP challenge.
func (c CircuitConfig) NumPopLayers() int {
	count := 0
	for i := 0; i < c.NumLayers; i++ {
		if c.HasPoP(i) {
			count++
		}
	}
	return count
}

// ExpectedPublicInputCount returns the expected number of Groth16 public inputs.
func (c CircuitConfig) ExpectedPublicInputCount() int {
	// circuitHash(2) + pubInputs(P) + outputChallenges(O) + claimAggCoeff(1)
	total := 2 + c.PubInputCount + c.OutputNumVars + 1
	// per-layer: bindings(nv_i) + rhos(nv_i+1) + gammas(nv_i) + podpChallenge(1) + popChallenge(0 or 1)
	for i := 0; i < c.NumLayers; i++ {
		nv := c.LayerNumVars[i]
		total += 3*nv + 2
		if c.HasPoP(i) {
			total++
		}
	}
	// inputRLCCoeffs(2) + inputPODPChallenge(1) + interLayerCoeffs(L-1)
	total += 2 + 1
	if c.NumLayers > 1 {
		total += c.NumLayers - 1
	}
	// mleEvalPoint(M) + inputClaimPoint(InputNumVars)
	total += c.MleEvalNumVars() + c.InputNumVars
	// rlcBeta(L) + zDotJStar(L) + lTensor(2*2^floor(input_nv/2)) + zDotR(1) + mleEval(1)
	total += c.NumLayers + c.NumLayers + c.NumLTensorElems() + 2
	return total
}

// SmallConfig returns the config for the toy circuit (num_vars=1, 2 public inputs).
// InputNumVars matches per-shred num_vars (shred selector is handled by RLC, not dimensions).
func SmallConfig() CircuitConfig {
	return CircuitConfig{
		NumLayers: 2, LayerNumVars: []int{1, 1}, LayerDegrees: []int{2, 3},
		OutputNumVars: 1, PubInputCount: 2, InputNumVars: 1,
	}
}

// MediumConfig returns the config for medium circuits (num_vars=4, 16 public inputs).
// InputNumVars matches per-shred num_vars (shred selector is handled by RLC, not dimensions).
func MediumConfig() CircuitConfig {
	return CircuitConfig{
		NumLayers: 2, LayerNumVars: []int{4, 4}, LayerDegrees: []int{2, 3},
		OutputNumVars: 4, PubInputCount: 16, InputNumVars: 4,
	}
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

// LayerPublic holds the per-layer public inputs for the Groth16 circuit.
// PopChallenge has length 0 (no PoP, degree <= 2) or 1 (has PoP, degree > 2).
type LayerPublic struct {
	Bindings      []frontend.Variable
	Rhos          []frontend.Variable
	Gammas        []frontend.Variable
	PODPChallenge frontend.Variable
	PopChallenge  []frontend.Variable
}

// RemainderWrapperCircuit wraps GKR algebraic verification inside a Groth16 SNARK.
//
// Option C design: Poseidon transcript replay stays on-chain (Fq). This circuit
// takes ALL Fiat-Shamir challenges as public inputs, computes Fr-based algebraic
// relations, and exposes the scalar results as public outputs for on-chain EC checks.
//
// Parameterized for N layers with per-layer num_vars (variable binding counts).
// The Hyrax input layer splits the last layer's num_vars into left/right halves.
//
// Public input layout matches Solidity buildGroth16Inputs():
//
//	circuitHash[2] | pubInputs[P] | outputChallenges[O] | claimAggCoeff |
//	per-layer{ bindings[nv_i], rhos[nv_i+1], gammas[nv_i], podpChallenge, popChallenge? } |
//	inputRLCCoeffs[2] | inputPODPChallenge | interLayerCoeffs[L-1] |
//	mleEvalPoint[M] | inputClaimPoint[InputNumVars] |
//	rlcBeta[L] | zDotJStar[L] | lTensor[2*2^floor(input_nv/2)] | zDotR | mleEval
type RemainderWrapperCircuit struct {
	// Config (not a circuit variable, used at compile time only)
	Config CircuitConfig `gnark:"-"`

	// === Public inputs (on-chain -> gnark, from Poseidon transcript) ===
	CircuitHash      [2]frontend.Variable `gnark:",public"`
	PublicInputs     []frontend.Variable  `gnark:",public"` // PubInputCount values
	OutputChallenges []frontend.Variable  `gnark:",public"` // OutputNumVars values
	ClaimAggCoeff    frontend.Variable    `gnark:",public"`

	// Per-layer challenges
	Layers []LayerPublic `gnark:",public"`

	// Input layer
	InputRLCCoeffs     [2]frontend.Variable `gnark:",public"`
	InputPODPChallenge frontend.Variable    `gnark:",public"`

	// Inter-layer claim aggregation coefficients (L-1 values)
	InterLayerCoeffs []frontend.Variable `gnark:",public"`

	// MLE evaluation point (log2(PubInputCount) values, from GKR claim propagation)
	MleEvalPoint []frontend.Variable `gnark:",public"`

	// Hyrax input claim point (InputNumVars values, for L/R tensor split)
	InputClaimPoint []frontend.Variable `gnark:",public"`

	// === Public outputs (gnark -> on-chain, used in EC equations) ===
	RlcBeta   []frontend.Variable `gnark:",public"` // one per layer
	ZDotJStar []frontend.Variable `gnark:",public"` // one per layer
	LTensor   []frontend.Variable `gnark:",public"` // 2 * 2^floor(num_vars/2)
	ZDotR     frontend.Variable   `gnark:",public"`
	MLEEval   frontend.Variable   `gnark:",public"`

	// === Private witness (off-chain) ===
	LayerPODPs []PODPWitness
	LayerPops  []PopWitness
	InputPODP  PODPWitness // z_vector has 2^ceil(num_vars/2) elements
}

// Define implements the gnark circuit interface.
func (c *RemainderWrapperCircuit) Define(api frontend.API) error {
	numLayers := c.Config.NumLayers

	for i := 0; i < numLayers; i++ {
		// rlcBeta: beta(layer_bindings, claim_point) * coeff
		var claimPoint []frontend.Variable
		var coeff frontend.Variable
		if i == 0 {
			claimPoint = c.OutputChallenges
			coeff = c.ClaimAggCoeff
		} else {
			claimPoint = c.Layers[i-1].Bindings
			coeff = c.InterLayerCoeffs[i-1]
		}
		rlcBeta := computeRlcBeta(api, c.Layers[i].Bindings, claimPoint, coeff)
		api.AssertIsEqual(rlcBeta, c.RlcBeta[i])

		// j_star and <z, j_star> for this layer
		jStar := computeJStar(api, c.Layers[i].Rhos, c.Layers[i].Gammas,
			c.Layers[i].Bindings, c.Config.LayerDegrees[i])
		zDotJStar := innerProduct(api, c.LayerPODPs[i].ZVector, jStar)
		api.AssertIsEqual(zDotJStar, c.ZDotJStar[i])
	}

	// Input layer: Hyrax matrix split for L-tensor and R-tensor
	// Uses InputClaimPoint for L/R tensor split (from GKR claim propagation)
	leftDims := c.Config.LeftDims()

	// L-tensor: for each shred, scale tensor(left_bindings) by the RLC coefficient
	leftBindings := c.InputClaimPoint[:leftDims]
	lPerShred := computeTensorProduct(api, leftBindings)
	lPerShredLen := len(lPerShred)
	for i := 0; i < lPerShredLen; i++ {
		expected0 := api.Mul(c.InputRLCCoeffs[0], lPerShred[i])
		api.AssertIsEqual(expected0, c.LTensor[i])
		expected1 := api.Mul(c.InputRLCCoeffs[1], lPerShred[i])
		api.AssertIsEqual(expected1, c.LTensor[lPerShredLen+i])
	}

	// R-tensor from right-half bindings -> 2^rightDims elements
	rightBindings := c.InputClaimPoint[leftDims:]
	rTensor := computeTensorProduct(api, rightBindings)
	zDotR := innerProduct(api, c.InputPODP.ZVector, rTensor)
	api.AssertIsEqual(zDotR, c.ZDotR)

	// Public input MLE evaluation: uses the explicit MLE eval point
	mleEval := evaluateMLE(api, c.PublicInputs, c.MleEvalPoint)
	api.AssertIsEqual(mleEval, c.MLEEval)

	return nil
}

// AllocateCircuit returns a circuit definition with correct slice sizes for the given config.
func AllocateCircuit(config CircuitConfig) *RemainderWrapperCircuit {
	nLayers := config.NumLayers

	layers := make([]LayerPublic, nLayers)
	layerPODPs := make([]PODPWitness, nLayers)
	layerPops := make([]PopWitness, nLayers)
	for i := 0; i < nLayers; i++ {
		nv := config.LayerNumVars[i]
		popLen := 0
		if config.HasPoP(i) {
			popLen = 1
		}
		layers[i] = LayerPublic{
			Bindings:     make([]frontend.Variable, nv),
			Rhos:         make([]frontend.Variable, nv+1),
			Gammas:       make([]frontend.Variable, nv),
			PopChallenge: make([]frontend.Variable, popLen),
		}
		layerPODPs[i] = PODPWitness{
			ZVector: make([]frontend.Variable, (config.LayerDegrees[i]+1)*nv),
		}
	}

	interLayerCoeffs := make([]frontend.Variable, 0)
	if nLayers > 1 {
		interLayerCoeffs = make([]frontend.Variable, nLayers-1)
	}

	return &RemainderWrapperCircuit{
		Config:           config,
		PublicInputs:     make([]frontend.Variable, config.PubInputCount),
		OutputChallenges: make([]frontend.Variable, config.OutputNumVars),
		Layers:           layers,
		InterLayerCoeffs: interLayerCoeffs,
		MleEvalPoint:     make([]frontend.Variable, config.MleEvalNumVars()),
		InputClaimPoint:  make([]frontend.Variable, config.InputNumVars),
		RlcBeta:          make([]frontend.Variable, nLayers),
		ZDotJStar:        make([]frontend.Variable, nLayers),
		LTensor:          make([]frontend.Variable, config.NumLTensorElems()),
		LayerPODPs:       layerPODPs,
		LayerPops:        layerPops,
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
