package main

import (
	"math/big"

	"github.com/consensys/gnark-crypto/ecc/bn254"
	"github.com/consensys/gnark-crypto/ecc/bn254/fr"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/std/algebra/emulated/sw_emulated"
	"github.com/consensys/gnark/std/math/emulated"
)

// G1Point is a BN254 G1 affine point in the gnark emulated field.
type G1Point = sw_emulated.AffinePoint[emulated.BN254Fp]

// ECCircuitConfig specifies the sizes of EC operation arrays at compile time.
type ECCircuitConfig struct {
	NumMulOps int // number of ec_mul operations to batch-verify
	NumAddOps int // number of ec_add operations to batch-verify
	// MSM ops are decomposed into mul+add before circuit compilation
}

// ECBatchCircuit verifies BN254 EC operations from GKR verification
// using random linear combination (RLC) batching.
//
// Architecture:
//
//	For each ec_mul claim: result_i == scalar_i * point_i
//	For each ec_add claim: result_j == p1_j + p2_j
//
// Batching reduces ALL checks to a single equation:
//
//	sum(r^i * result_i) + sum(r^(M+j) * result_j)
//	  == sum(r^i * scalar_i * point_i) + sum(r^(M+j) * p1_j) + sum(r^(M+j) * p2_j)
//
// Both sides are MSMs computable in the circuit.
type ECBatchCircuit struct {
	// Public inputs
	TranscriptDigest frontend.Variable `gnark:",public"`
	CircuitHashLo    frontend.Variable `gnark:",public"`
	CircuitHashHi    frontend.Variable `gnark:",public"`

	// ec_mul claims: result_i == scalar_i * point_i
	MulBasePoints []G1Point         `gnark:",secret"`
	MulScalars    []frontend.Variable `gnark:",secret"` // native Fr scalars
	MulResults    []G1Point         `gnark:",secret"`

	// ec_add claims: result_j == p1_j + p2_j
	AddP1s     []G1Point `gnark:",secret"`
	AddP2s     []G1Point `gnark:",secret"`
	AddResults []G1Point `gnark:",secret"`

	// Batch challenge (derived from hashing all claims, private but deterministic)
	BatchChallenge frontend.Variable `gnark:",secret"`

	Config ECCircuitConfig `gnark:"-"`
}

// AllocateECCircuit creates a circuit struct with pre-sized slices.
func AllocateECCircuit(config ECCircuitConfig) *ECBatchCircuit {
	c := &ECBatchCircuit{
		Config: config,
	}
	c.MulBasePoints = make([]G1Point, config.NumMulOps)
	c.MulScalars = make([]frontend.Variable, config.NumMulOps)
	c.MulResults = make([]G1Point, config.NumMulOps)

	c.AddP1s = make([]G1Point, config.NumAddOps)
	c.AddP2s = make([]G1Point, config.NumAddOps)
	c.AddResults = make([]G1Point, config.NumAddOps)

	return c
}

// Define implements the gnark circuit constraints.
func (c *ECBatchCircuit) Define(api frontend.API) error {
	// Create the emulated BN254 curve API
	curve, err := sw_emulated.New[emulated.BN254Fp, emulated.BN254Fr](api, sw_emulated.GetBN254Params())
	if err != nil {
		return err
	}

	// Create emulated scalar field for converting native Fr to emulated
	scalarField, err := emulated.NewField[emulated.BN254Fr](api)
	if err != nil {
		return err
	}

	numMul := c.Config.NumMulOps
	numAdd := c.Config.NumAddOps

	if numMul == 0 && numAdd == 0 {
		return nil
	}

	// Compute batch challenge powers: r^0, r^1, ..., r^(N-1) where N = numMul + numAdd
	totalOps := numMul + numAdd
	rPowers := make([]frontend.Variable, totalOps)
	rPowers[0] = frontend.Variable(1)
	if totalOps > 1 {
		rPowers[1] = c.BatchChallenge
		for i := 2; i < totalOps; i++ {
			rPowers[i] = api.Mul(rPowers[i-1], c.BatchChallenge)
		}
	}

	// ====================================================================
	// Batch ec_mul checks: for each i, result_i == scalar_i * point_i
	// ====================================================================
	// We compute:
	//   LHS_mul = sum(r^i * result_i)     — an MSM with scalars r^i
	//   RHS_mul = sum((r^i * s_i) * P_i)  — an MSM with scalars (r^i * s_i)
	// And check LHS_mul == RHS_mul

	if numMul > 0 {
		// LHS: sum(r^i * result_i)
		lhsPoints := make([]*G1Point, numMul)
		lhsScalars := make([]*emulated.Element[emulated.BN254Fr], numMul)
		for i := 0; i < numMul; i++ {
			lhsPoints[i] = &c.MulResults[i]
			lhsScalars[i] = scalarField.FromBits(api.ToBinary(rPowers[i], int(fr.Bits))...)
		}
		lhsMSM, err := curve.MultiScalarMul(lhsPoints, lhsScalars)
		if err != nil {
			return err
		}

		// RHS: sum((r^i * scalar_i) * point_i)
		rhsPoints := make([]*G1Point, numMul)
		rhsScalars := make([]*emulated.Element[emulated.BN254Fr], numMul)
		for i := 0; i < numMul; i++ {
			rhsPoints[i] = &c.MulBasePoints[i]
			combinedScalar := api.Mul(rPowers[i], c.MulScalars[i])
			rhsScalars[i] = scalarField.FromBits(api.ToBinary(combinedScalar, int(fr.Bits))...)
		}
		rhsMSM, err := curve.MultiScalarMul(rhsPoints, rhsScalars)
		if err != nil {
			return err
		}

		// Assert LHS == RHS
		curve.AssertIsEqual(lhsMSM, rhsMSM)
	}

	// ====================================================================
	// Batch ec_add checks: for each j, result_j == p1_j + p2_j
	// ====================================================================
	// We compute:
	//   LHS_add = sum(r^(M+j) * result_j)
	//   RHS_add = sum(r^(M+j) * p1_j) + sum(r^(M+j) * p2_j)
	// And check LHS_add == RHS_add

	if numAdd > 0 {
		addScalars := make([]*emulated.Element[emulated.BN254Fr], numAdd)
		for j := 0; j < numAdd; j++ {
			rIdx := numMul + j
			addScalars[j] = scalarField.FromBits(api.ToBinary(rPowers[rIdx], int(fr.Bits))...)
		}

		// LHS: sum(r^(M+j) * result_j)
		lhsAddPts := make([]*G1Point, numAdd)
		for j := 0; j < numAdd; j++ {
			lhsAddPts[j] = &c.AddResults[j]
		}
		lhsAdd, err := curve.MultiScalarMul(lhsAddPts, addScalars)
		if err != nil {
			return err
		}

		// RHS part 1: sum(r^(M+j) * p1_j)
		rhsP1Pts := make([]*G1Point, numAdd)
		for j := 0; j < numAdd; j++ {
			rhsP1Pts[j] = &c.AddP1s[j]
		}
		rhsP1, err := curve.MultiScalarMul(rhsP1Pts, addScalars)
		if err != nil {
			return err
		}

		// RHS part 2: sum(r^(M+j) * p2_j)
		rhsP2Pts := make([]*G1Point, numAdd)
		for j := 0; j < numAdd; j++ {
			rhsP2Pts[j] = &c.AddP2s[j]
		}
		rhsP2, err := curve.MultiScalarMul(rhsP2Pts, addScalars)
		if err != nil {
			return err
		}

		// RHS = rhsP1 + rhsP2
		// Use AddUnified to handle the edge case where rhsP1 == rhsP2
		rhsAdd := curve.AddUnified(rhsP1, rhsP2)

		// Assert LHS == RHS
		curve.AssertIsEqual(lhsAdd, rhsAdd)
	}

	return nil
}

// MakeTestECAssignment creates a valid test assignment with N ec_mul and M ec_add
// operations using distinct BN254 G1 points to avoid degenerate cases.
func MakeTestECAssignment(numMul, numAdd int) *ECBatchCircuit {
	config := ECCircuitConfig{NumMulOps: numMul, NumAddOps: numAdd}
	assignment := AllocateECCircuit(config)

	assignment.TranscriptDigest = big.NewInt(42)
	assignment.CircuitHashLo = big.NewInt(1)
	assignment.CircuitHashHi = big.NewInt(2)
	assignment.BatchChallenge = big.NewInt(7)

	// BN254 generator point G1
	_, _, g1, _ := bn254.Generators()

	// ec_mul: scalar_i * base_i = result_i
	// Use different base points and scalars to avoid degenerate cases in jointScalarMul
	for i := 0; i < numMul; i++ {
		baseMul := big.NewInt(int64(2*i + 3)) // distinct base multipliers: 3, 5, 7, ...
		s := big.NewInt(int64(i + 2))          // scalars: 2, 3, 4, ...
		var base, result bn254.G1Affine
		base.ScalarMultiplication(&g1, baseMul)
		var combinedScalar big.Int
		combinedScalar.Mul(baseMul, s)
		result.ScalarMultiplication(&g1, &combinedScalar)
		assignment.MulBasePoints[i] = newG1PointAssignment(base)
		assignment.MulScalars[i] = s
		assignment.MulResults[i] = newG1PointAssignment(result)
	}

	// ec_add: p1_j + p2_j = result_j, using distinct points for each
	for j := 0; j < numAdd; j++ {
		s1 := big.NewInt(int64(2*j + 3))
		s2 := big.NewInt(int64(2*j + 5))
		var p1, p2, result bn254.G1Affine
		p1.ScalarMultiplication(&g1, s1)
		p2.ScalarMultiplication(&g1, s2)
		result.Add(&p1, &p2)
		assignment.AddP1s[j] = newG1PointAssignment(p1)
		assignment.AddP2s[j] = newG1PointAssignment(p2)
		assignment.AddResults[j] = newG1PointAssignment(result)
	}

	return assignment
}

// newG1PointAssignment creates a G1Point assignment from a native bn254.G1Affine.
func newG1PointAssignment(p bn254.G1Affine) G1Point {
	var xBig, yBig big.Int
	p.X.BigInt(&xBig)
	p.Y.BigInt(&yBig)
	return G1Point{
		X: emulated.ValueOf[emulated.BN254Fp](&xBig),
		Y: emulated.ValueOf[emulated.BN254Fp](&yBig),
	}
}
