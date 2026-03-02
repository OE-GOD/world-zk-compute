package main

import (
	"math/big"

	"github.com/consensys/gnark/frontend"
)

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
// The on-chain verifier:
//  1. Replays the Poseidon transcript to derive challenges
//  2. Verifies this Groth16 proof with challenges as public inputs
//  3. Reads the public outputs (Fr scalars) from the Groth16 proof
//  4. Uses those scalars in EC equations (PODP, PoP) via precompiles
//
// Fixed-size circuit for the test proof: 2 layers (subtract + multiply), 2 public inputs.
type RemainderWrapperCircuit struct {
	// === Public inputs (on-chain → gnark, from Poseidon transcript) ===
	CircuitHash0 frontend.Variable `gnark:",public"`
	CircuitHash1 frontend.Variable `gnark:",public"`
	PublicInput0 frontend.Variable `gnark:",public"`
	PublicInput1 frontend.Variable `gnark:",public"`

	// Output challenge and claim aggregation coefficient
	OutputChallenge frontend.Variable `gnark:",public"`
	ClaimAggCoeff   frontend.Variable `gnark:",public"`

	// Layer 0 (subtract): 1 binding, rhos[2], gammas[1], PODP challenge
	Layer0Bindings      [1]frontend.Variable `gnark:",public"`
	Layer0Rhos          [2]frontend.Variable `gnark:",public"`
	Layer0Gammas        [1]frontend.Variable `gnark:",public"`
	Layer0PODPChallenge frontend.Variable    `gnark:",public"`

	// Layer 1 (multiply): 1 binding, rhos[2], gammas[1], PODP challenge, PoP challenge
	Layer1Bindings      [1]frontend.Variable `gnark:",public"`
	Layer1Rhos          [2]frontend.Variable `gnark:",public"`
	Layer1Gammas        [1]frontend.Variable `gnark:",public"`
	Layer1PODPChallenge frontend.Variable    `gnark:",public"`
	Layer1PopChallenge  frontend.Variable    `gnark:",public"`

	// Input layer: RLC coefficients for multi-claim, PODP challenge
	InputRLCCoeff0     frontend.Variable `gnark:",public"`
	InputRLCCoeff1     frontend.Variable `gnark:",public"`
	InputPODPChallenge frontend.Variable `gnark:",public"`

	// Inter-layer claim aggregation coefficient
	InterLayerCoeff frontend.Variable `gnark:",public"`

	// === Public outputs (gnark → on-chain, used in EC equations) ===
	// These are the Fr scalar values that the on-chain verifier needs for
	// EC operations (PODP, PoP, oracle_eval, Hyrax, MLE).

	// Per-layer: rlc_beta values (used to scale commitments in oracle_eval)
	RlcBeta0 frontend.Variable `gnark:",public"`
	RlcBeta1 frontend.Variable `gnark:",public"`

	// Per-layer: <z_vector, j_star> inner products (used in PODP com_z_dot_a)
	ZDotJStar0 frontend.Variable `gnark:",public"`
	ZDotJStar1 frontend.Variable `gnark:",public"`

	// Input layer: L-tensor coefficients (used in Hyrax MSM over commitment rows)
	LTensor0 frontend.Variable `gnark:",public"`
	LTensor1 frontend.Variable `gnark:",public"`

	// Input layer: <z_vector, R-tensor> (used in input PODP com_z_dot_a)
	ZDotR frontend.Variable `gnark:",public"`

	// Public input MLE evaluation (used to compute expectedCom = mleEval * g_scalar)
	MLEEval frontend.Variable `gnark:",public"`

	// === Private witness (off-chain) ===
	Layer0PODP PODPWitness
	Layer1PODP PODPWitness
	Layer1Pop  PopWitness
	InputPODP  PODPWitness
}

// BN254 scalar field modulus (Fr)
var frModulus, _ = new(big.Int).SetString("21888242871839275222246405745257275088548364400416034343698204186575808495617", 10)

// Define implements the gnark circuit interface.
func (c *RemainderWrapperCircuit) Define(api frontend.API) error {
	// ======================================================================
	// Layer 0 (subtract gate, degree=2)
	// ======================================================================

	// Compute rlc_beta_0 = beta(layer0_bindings, [outputChallenge]) * claimAggCoeff
	rlcBeta0 := computeRlcBeta(api,
		c.Layer0Bindings[:],
		[]frontend.Variable{c.OutputChallenge},
		c.ClaimAggCoeff,
	)
	api.AssertIsEqual(rlcBeta0, c.RlcBeta0)

	// Compute j_star for layer 0 and inner product <z, j_star>
	jStar0 := computeJStar(api, c.Layer0Rhos[:], c.Layer0Gammas[:], c.Layer0Bindings[:], 2)
	zDotJStar0 := innerProduct(api, c.Layer0PODP.ZVector, jStar0)
	api.AssertIsEqual(zDotJStar0, c.ZDotJStar0)

	// ======================================================================
	// Layer 1 (multiply gate, degree=3)
	// ======================================================================

	// Compute rlc_beta_1 = beta(layer1_bindings, layer0_bindings) * interLayerCoeff
	rlcBeta1 := computeRlcBeta(api,
		c.Layer1Bindings[:],
		c.Layer0Bindings[:],
		c.InterLayerCoeff,
	)
	api.AssertIsEqual(rlcBeta1, c.RlcBeta1)

	// Compute j_star for layer 1 and inner product
	jStar1 := computeJStar(api, c.Layer1Rhos[:], c.Layer1Gammas[:], c.Layer1Bindings[:], 3)
	zDotJStar1 := innerProduct(api, c.Layer1PODP.ZVector, jStar1)
	api.AssertIsEqual(zDotJStar1, c.ZDotJStar1)

	// PoP scalar relations: z1..z5 are absorbed into transcript on-chain.
	// The 3 EC equations are verified on-chain via precompiles.
	// No additional Fr constraints needed here — the PoP values are
	// only relevant to the EC checks.

	// ======================================================================
	// Input layer: tensor products and PODP
	// ======================================================================

	// L-tensor = RLC of individual L-tensors using input RLC coefficients.
	// For 2-shred input with claim points (0, r) and (1, r):
	//   L-tensor for claim 0 (first var=0): [1, 0]
	//   L-tensor for claim 1 (first var=1): [0, 1]
	//   lCoeffs = coeff0 * [1,0] + coeff1 * [0,1] = [coeff0, coeff1]
	api.AssertIsEqual(c.InputRLCCoeff0, c.LTensor0)
	api.AssertIsEqual(c.InputRLCCoeff1, c.LTensor1)

	// R-tensor from shared R-half (layer1 binding):
	// tensor([r]) = [(1-r), r]
	rCoeff0 := api.Sub(1, c.Layer1Bindings[0])
	rCoeff1 := c.Layer1Bindings[0]

	// Inner product <z_vector, R_tensor>
	zDotR := innerProduct(api, c.InputPODP.ZVector, []frontend.Variable{rCoeff0, rCoeff1})
	api.AssertIsEqual(zDotR, c.ZDotR)

	// ======================================================================
	// Public input MLE evaluation
	// ======================================================================
	// MLE([pubInput0, pubInput1], [x]) = (1-x)*pubInput0 + x*pubInput1
	// where x = layer0 binding
	{
		oneMinusX := api.Sub(1, c.Layer0Bindings[0])
		term0 := api.Mul(oneMinusX, c.PublicInput0)
		term1 := api.Mul(c.Layer0Bindings[0], c.PublicInput1)
		mleEval := api.Add(term0, term1)
		api.AssertIsEqual(mleEval, c.MLEEval)
	}

	return nil
}

// AllocateCircuit returns a circuit definition with correct slice sizes for the test proof.
func AllocateCircuit() *RemainderWrapperCircuit {
	return &RemainderWrapperCircuit{
		// Layer 0: subtract, 1 binding, degree=2 → PODP z_vector has 3 elements
		Layer0PODP: PODPWitness{
			ZVector: make([]frontend.Variable, 3),
		},
		// Layer 1: multiply, 1 binding, degree=3 → PODP z_vector has 4 elements
		Layer1PODP: PODPWitness{
			ZVector: make([]frontend.Variable, 4),
		},
		// Input layer: 2 R-tensor coefficients → z_vector has 2 elements
		InputPODP: PODPWitness{
			ZVector: make([]frontend.Variable, 2),
		},
	}
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
