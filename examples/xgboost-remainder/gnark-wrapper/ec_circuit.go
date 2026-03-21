package main

import (
	"math/big"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark-crypto/ecc/bn254"
	"github.com/consensys/gnark-crypto/ecc/bn254/fr"
	mimcbn254 "github.com/consensys/gnark-crypto/ecc/bn254/fr/mimc"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"
	"github.com/consensys/gnark/std/algebra/emulated/sw_emulated"
	stdmimc "github.com/consensys/gnark/std/hash/mimc"
	"github.com/consensys/gnark/std/math/emulated"
)

// G1Point is a BN254 G1 affine point in the gnark emulated field.
type G1Point = sw_emulated.AffinePoint[emulated.BN254Fp]

// ECCircuitConfig specifies the sizes of EC operation arrays at compile time.
//
// Measured R1CS constraint counts (gnark v0.11.0, BN254, with MiMC batch challenge):
//
//	1 mul:              ~193K   (192K EC + ~1K MiMC)
//	10 mul:             ~1.01M
//	1 add:              ~285K
//	10 add:             ~1.46M
//	5 mul + 5 add:      ~1.38M
//
// XGBoost full circuit estimate: ~2213 mul + ~1649 add ≈ 300-400M constraints
// → will need chunked proving + recursion (each chunk ≤ 50M constraints)
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
// Soundness: The RLC batch challenge is derived in-circuit via MiMC hash of
// (transcriptDigest || all operation data). This prevents a malicious prover
// from choosing the challenge after seeing the operations.
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
	MulBasePoints []G1Point           `gnark:",secret"`
	MulScalars    []frontend.Variable `gnark:",secret"` // native Fr scalars
	MulResults    []G1Point           `gnark:",secret"`

	// ec_add claims: result_j == p1_j + p2_j
	AddP1s     []G1Point `gnark:",secret"`
	AddP2s     []G1Point `gnark:",secret"`
	AddResults []G1Point `gnark:",secret"`

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

	// ====================================================================
	// Derive batch challenge via MiMC hash of all operation data.
	// This binds the RLC challenge to the specific operations being verified.
	// ====================================================================
	batchChallenge := c.computeBatchChallenge(api)

	// Compute batch challenge powers: r^0, r^1, ..., r^(N-1) where N = numMul + numAdd
	totalOps := numMul + numAdd
	rPowers := make([]frontend.Variable, totalOps)
	rPowers[0] = frontend.Variable(1)
	if totalOps > 1 {
		rPowers[1] = batchChallenge
		for i := 2; i < totalOps; i++ {
			rPowers[i] = api.Mul(rPowers[i-1], batchChallenge)
		}
	}

	// ====================================================================
	// Batch ec_mul checks: for each i, result_i == scalar_i * point_i
	// ====================================================================
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

		curve.AssertIsEqual(lhsMSM, rhsMSM)
	}

	// ====================================================================
	// Batch ec_add checks: for each j, result_j == p1_j + p2_j
	// ====================================================================
	if numAdd > 0 {
		addScalars := make([]*emulated.Element[emulated.BN254Fr], numAdd)
		for j := 0; j < numAdd; j++ {
			rIdx := numMul + j
			addScalars[j] = scalarField.FromBits(api.ToBinary(rPowers[rIdx], int(fr.Bits))...)
		}

		lhsAddPts := make([]*G1Point, numAdd)
		for j := 0; j < numAdd; j++ {
			lhsAddPts[j] = &c.AddResults[j]
		}
		lhsAdd, err := curve.MultiScalarMul(lhsAddPts, addScalars)
		if err != nil {
			return err
		}

		rhsP1Pts := make([]*G1Point, numAdd)
		for j := 0; j < numAdd; j++ {
			rhsP1Pts[j] = &c.AddP1s[j]
		}
		rhsP1, err := curve.MultiScalarMul(rhsP1Pts, addScalars)
		if err != nil {
			return err
		}

		rhsP2Pts := make([]*G1Point, numAdd)
		for j := 0; j < numAdd; j++ {
			rhsP2Pts[j] = &c.AddP2s[j]
		}
		rhsP2, err := curve.MultiScalarMul(rhsP2Pts, addScalars)
		if err != nil {
			return err
		}

		// Use AddUnified to handle the edge case where rhsP1 == rhsP2
		rhsAdd := curve.AddUnified(rhsP1, rhsP2)
		curve.AssertIsEqual(lhsAdd, rhsAdd)
	}

	return nil
}

// computeBatchChallenge derives the RLC batch challenge by MiMC-hashing
// the transcript digest and all EC operation data (point coords as limbs + scalars).
// This ensures the challenge is bound to the specific operations.
func (c *ECBatchCircuit) computeBatchChallenge(api frontend.API) frontend.Variable {
	h, _ := stdmimc.NewMiMC(api)

	// Hash transcript digest first (domain separator)
	h.Write(c.TranscriptDigest)

	// Hash all ec_mul operation data
	for i := 0; i < c.Config.NumMulOps; i++ {
		// Hash base point coordinates (emulated Fp, 4 limbs each)
		h.Write(c.MulBasePoints[i].X.Limbs...)
		h.Write(c.MulBasePoints[i].Y.Limbs...)
		// Hash scalar (native Fr)
		h.Write(c.MulScalars[i])
		// Hash result point
		h.Write(c.MulResults[i].X.Limbs...)
		h.Write(c.MulResults[i].Y.Limbs...)
	}

	// Hash all ec_add operation data
	for j := 0; j < c.Config.NumAddOps; j++ {
		h.Write(c.AddP1s[j].X.Limbs...)
		h.Write(c.AddP1s[j].Y.Limbs...)
		h.Write(c.AddP2s[j].X.Limbs...)
		h.Write(c.AddP2s[j].Y.Limbs...)
		h.Write(c.AddResults[j].X.Limbs...)
		h.Write(c.AddResults[j].Y.Limbs...)
	}

	return h.Sum()
}

// ComputeBatchChallengeOutOfCircuit computes the same MiMC hash as the circuit
// does, but outside the circuit (for witness generation).
// Points are passed as [x, y] big.Int pairs, scalars as big.Int.
func ComputeBatchChallengeOutOfCircuit(
	transcriptDigest *big.Int,
	mulBasePoints [][2]*big.Int, // [i][0]=x, [i][1]=y
	mulScalars []*big.Int,
	mulResults [][2]*big.Int,
	addP1s [][2]*big.Int,
	addP2s [][2]*big.Int,
	addResults [][2]*big.Int,
) *big.Int {
	h := mimcbn254.NewMiMC()

	// Helper: write a big.Int as 32-byte big-endian to the hasher
	writeBigInt := func(v *big.Int) {
		var buf [32]byte
		b := v.Bytes()
		copy(buf[32-len(b):], b)
		h.Write(buf[:])
	}

	// Helper: write an Fp element's 4 limbs (matching gnark's emulated representation)
	writeFpLimbs := func(v *big.Int) {
		// gnark stores BN254Fp as 4 × 64-bit limbs in little-endian order
		// The MiMC in-circuit hashes these limb values as native Fr elements
		limbs := fpToLimbs(v)
		for _, limb := range limbs {
			writeBigInt(limb)
		}
	}

	writeBigInt(transcriptDigest)

	for i := range mulBasePoints {
		writeFpLimbs(mulBasePoints[i][0])
		writeFpLimbs(mulBasePoints[i][1])
		writeBigInt(mulScalars[i])
		writeFpLimbs(mulResults[i][0])
		writeFpLimbs(mulResults[i][1])
	}

	for j := range addP1s {
		writeFpLimbs(addP1s[j][0])
		writeFpLimbs(addP1s[j][1])
		writeFpLimbs(addP2s[j][0])
		writeFpLimbs(addP2s[j][1])
		writeFpLimbs(addResults[j][0])
		writeFpLimbs(addResults[j][1])
	}

	digest := h.Sum(nil)
	result := new(big.Int).SetBytes(digest)

	// Reduce mod BN254 Fr order (MiMC output is already in Fr, but be safe)
	frModulus := ecc.BN254.ScalarField()
	result.Mod(result, frModulus)
	return result
}

// fpToLimbs converts a big.Int (BN254 Fp value) into 4 × 64-bit limbs
// matching gnark's emulated representation (little-endian limb order).
func fpToLimbs(v *big.Int) [4]*big.Int {
	mask := new(big.Int).SetUint64(^uint64(0)) // 2^64 - 1
	var limbs [4]*big.Int
	tmp := new(big.Int).Set(v)
	for i := 0; i < 4; i++ {
		limbs[i] = new(big.Int).And(tmp, mask)
		tmp.Rsh(tmp, 64)
	}
	return limbs
}

// MakeTestECAssignment creates a valid test assignment with N ec_mul and M ec_add
// operations using distinct BN254 G1 points to avoid degenerate cases.
func MakeTestECAssignment(numMul, numAdd int) *ECBatchCircuit {
	config := ECCircuitConfig{NumMulOps: numMul, NumAddOps: numAdd}
	assignment := AllocateECCircuit(config)

	assignment.TranscriptDigest = big.NewInt(42)
	assignment.CircuitHashLo = big.NewInt(1)
	assignment.CircuitHashHi = big.NewInt(2)

	// BN254 generator point G1
	_, _, g1, _ := bn254.Generators()

	// Build mul and add operations with distinct points
	mulBasePointCoords := make([][2]*big.Int, numMul)
	mulScalarsBig := make([]*big.Int, numMul)
	mulResultCoords := make([][2]*big.Int, numMul)

	for i := 0; i < numMul; i++ {
		baseMul := big.NewInt(int64(2*i + 3))
		s := big.NewInt(int64(i + 2))
		var base, result bn254.G1Affine
		base.ScalarMultiplication(&g1, baseMul)
		var combinedScalar big.Int
		combinedScalar.Mul(baseMul, s)
		result.ScalarMultiplication(&g1, &combinedScalar)
		assignment.MulBasePoints[i] = newG1PointAssignment(base)
		assignment.MulScalars[i] = s
		assignment.MulResults[i] = newG1PointAssignment(result)

		var bx, by, rx, ry big.Int
		base.X.BigInt(&bx)
		base.Y.BigInt(&by)
		result.X.BigInt(&rx)
		result.Y.BigInt(&ry)
		mulBasePointCoords[i] = [2]*big.Int{&bx, &by}
		mulScalarsBig[i] = new(big.Int).Set(s)
		mulResultCoords[i] = [2]*big.Int{&rx, &ry}
	}

	addP1Coords := make([][2]*big.Int, numAdd)
	addP2Coords := make([][2]*big.Int, numAdd)
	addResultCoords := make([][2]*big.Int, numAdd)

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

		var p1x, p1y, p2x, p2y, rx, ry big.Int
		p1.X.BigInt(&p1x)
		p1.Y.BigInt(&p1y)
		p2.X.BigInt(&p2x)
		p2.Y.BigInt(&p2y)
		result.X.BigInt(&rx)
		result.Y.BigInt(&ry)
		addP1Coords[j] = [2]*big.Int{&p1x, &p1y}
		addP2Coords[j] = [2]*big.Int{&p2x, &p2y}
		addResultCoords[j] = [2]*big.Int{&rx, &ry}
	}

	// BatchChallenge is no longer an input — it's computed in-circuit.
	// But we don't need to set it since we removed it from the struct.

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

// compileECCircuit is a helper for tests that compiles and returns constraint count.
func compileECCircuit(config ECCircuitConfig) (int, error) {
	circuit := AllocateECCircuit(config)
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		return 0, err
	}
	return ccs.GetNbConstraints(), nil
}

// ============================================================================
// Chunked EC Batch Circuit
// ============================================================================

// DefaultChunkSize is the default number of total operations (mul+add) per chunk.
// ~500 ops yields ~50M constraints, which gnark can handle in a single Groth16 proof.
const DefaultChunkSize = 500

// ChunkedECConfig specifies the sizes for a single chunk of EC operations.
type ChunkedECConfig struct {
	NumMulOps int // number of ec_mul operations in this chunk
	NumAddOps int // number of ec_add operations in this chunk
}

// ChunkedECBatchCircuit verifies a single chunk of BN254 EC operations.
// Multiple chunks together verify the full set of EC operations from GKR verification.
//
// Each chunk shares the same public inputs that bind it to the overall computation:
//   - TranscriptDigest: binds to the Stylus/on-chain transcript
//   - CircuitHashLo/Hi: identifies the circuit being verified
//   - OpsDigest: MiMC hash of ALL operations across ALL chunks (prevents tampering/reordering)
//   - ChunkIndex: which chunk this is (0-indexed)
//   - TotalChunks: total number of chunks
//
// Within each chunk, operations are verified using RLC batching with an in-circuit
// MiMC-derived challenge (same approach as ECBatchCircuit).
type ChunkedECBatchCircuit struct {
	// Public inputs shared across all chunks
	TranscriptDigest frontend.Variable `gnark:",public"`
	CircuitHashLo    frontend.Variable `gnark:",public"`
	CircuitHashHi    frontend.Variable `gnark:",public"`
	OpsDigest        frontend.Variable `gnark:",public"` // MiMC hash of ALL operations
	ChunkIndex       frontend.Variable `gnark:",public"` // 0-indexed chunk number
	TotalChunks      frontend.Variable `gnark:",public"` // total number of chunks

	// Per-chunk EC operations (private witnesses)
	MulBasePoints []G1Point           `gnark:",secret"`
	MulScalars    []frontend.Variable `gnark:",secret"`
	MulResults    []G1Point           `gnark:",secret"`
	AddP1s        []G1Point           `gnark:",secret"`
	AddP2s        []G1Point           `gnark:",secret"`
	AddResults    []G1Point           `gnark:",secret"`

	Config ChunkedECConfig `gnark:"-"`
}

// AllocateChunkedECCircuit creates a chunked circuit struct with pre-sized slices.
func AllocateChunkedECCircuit(config ChunkedECConfig) *ChunkedECBatchCircuit {
	c := &ChunkedECBatchCircuit{
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

// Define implements the gnark circuit constraints for a single chunk.
func (c *ChunkedECBatchCircuit) Define(api frontend.API) error {
	numMul := c.Config.NumMulOps
	numAdd := c.Config.NumAddOps

	if numMul == 0 && numAdd == 0 {
		return nil
	}

	// Create the emulated BN254 curve API
	curve, err := sw_emulated.New[emulated.BN254Fp, emulated.BN254Fr](api, sw_emulated.GetBN254Params())
	if err != nil {
		return err
	}

	scalarField, err := emulated.NewField[emulated.BN254Fr](api)
	if err != nil {
		return err
	}

	// ====================================================================
	// Constrain ChunkIndex < TotalChunks via range check.
	// We check that TotalChunks - ChunkIndex - 1 >= 0 by bit-decomposing it.
	// ====================================================================
	diff := api.Sub(c.TotalChunks, c.ChunkIndex, 1)
	api.ToBinary(diff, 32) // constrains diff to [0, 2^32), proving ChunkIndex < TotalChunks

	// ====================================================================
	// Derive per-chunk batch challenge via MiMC.
	// We hash: OpsDigest || ChunkIndex || all chunk operation data.
	// This ensures each chunk's RLC challenge is unique and bound to its ops.
	// ====================================================================
	batchChallenge := c.computeChunkBatchChallenge(api)

	// Compute batch challenge powers
	totalOps := numMul + numAdd
	rPowers := make([]frontend.Variable, totalOps)
	rPowers[0] = frontend.Variable(1)
	if totalOps > 1 {
		rPowers[1] = batchChallenge
		for i := 2; i < totalOps; i++ {
			rPowers[i] = api.Mul(rPowers[i-1], batchChallenge)
		}
	}

	// ====================================================================
	// Batch ec_mul checks
	// ====================================================================
	if numMul > 0 {
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

		curve.AssertIsEqual(lhsMSM, rhsMSM)
	}

	// ====================================================================
	// Batch ec_add checks
	// ====================================================================
	if numAdd > 0 {
		addScalars := make([]*emulated.Element[emulated.BN254Fr], numAdd)
		for j := 0; j < numAdd; j++ {
			rIdx := numMul + j
			addScalars[j] = scalarField.FromBits(api.ToBinary(rPowers[rIdx], int(fr.Bits))...)
		}

		lhsAddPts := make([]*G1Point, numAdd)
		for j := 0; j < numAdd; j++ {
			lhsAddPts[j] = &c.AddResults[j]
		}
		lhsAdd, err := curve.MultiScalarMul(lhsAddPts, addScalars)
		if err != nil {
			return err
		}

		rhsP1Pts := make([]*G1Point, numAdd)
		for j := 0; j < numAdd; j++ {
			rhsP1Pts[j] = &c.AddP1s[j]
		}
		rhsP1, err := curve.MultiScalarMul(rhsP1Pts, addScalars)
		if err != nil {
			return err
		}

		rhsP2Pts := make([]*G1Point, numAdd)
		for j := 0; j < numAdd; j++ {
			rhsP2Pts[j] = &c.AddP2s[j]
		}
		rhsP2, err := curve.MultiScalarMul(rhsP2Pts, addScalars)
		if err != nil {
			return err
		}

		rhsAdd := curve.AddUnified(rhsP1, rhsP2)
		curve.AssertIsEqual(lhsAdd, rhsAdd)
	}

	return nil
}

// computeChunkBatchChallenge derives the per-chunk RLC batch challenge.
// Hash: MiMC(OpsDigest || ChunkIndex || all chunk EC operation data).
func (c *ChunkedECBatchCircuit) computeChunkBatchChallenge(api frontend.API) frontend.Variable {
	h, _ := stdmimc.NewMiMC(api)

	// Domain: bind to global ops digest and chunk index
	h.Write(c.OpsDigest)
	h.Write(c.ChunkIndex)

	// Hash all ec_mul operation data in this chunk
	for i := 0; i < c.Config.NumMulOps; i++ {
		h.Write(c.MulBasePoints[i].X.Limbs...)
		h.Write(c.MulBasePoints[i].Y.Limbs...)
		h.Write(c.MulScalars[i])
		h.Write(c.MulResults[i].X.Limbs...)
		h.Write(c.MulResults[i].Y.Limbs...)
	}

	// Hash all ec_add operation data in this chunk
	for j := 0; j < c.Config.NumAddOps; j++ {
		h.Write(c.AddP1s[j].X.Limbs...)
		h.Write(c.AddP1s[j].Y.Limbs...)
		h.Write(c.AddP2s[j].X.Limbs...)
		h.Write(c.AddP2s[j].Y.Limbs...)
		h.Write(c.AddResults[j].X.Limbs...)
		h.Write(c.AddResults[j].Y.Limbs...)
	}

	return h.Sum()
}

// ComputeOpsDigest computes the MiMC digest of ALL EC operations across all chunks.
// This is the public input OpsDigest that each chunk circuit verifies against.
// It hashes: transcriptDigest || all mul data (Fp limbs + scalar) || all add data (Fp limbs).
func ComputeOpsDigest(
	transcriptDigest *big.Int,
	mulBasePoints [][2]*big.Int,
	mulScalars []*big.Int,
	mulResults [][2]*big.Int,
	addP1s [][2]*big.Int,
	addP2s [][2]*big.Int,
	addResults [][2]*big.Int,
) *big.Int {
	h := mimcbn254.NewMiMC()

	writeBigInt := func(v *big.Int) {
		var buf [32]byte
		b := v.Bytes()
		copy(buf[32-len(b):], b)
		h.Write(buf[:])
	}

	writeFpLimbs := func(v *big.Int) {
		limbs := fpToLimbs(v)
		for _, limb := range limbs {
			writeBigInt(limb)
		}
	}

	writeBigInt(transcriptDigest)

	for i := range mulBasePoints {
		writeFpLimbs(mulBasePoints[i][0])
		writeFpLimbs(mulBasePoints[i][1])
		writeBigInt(mulScalars[i])
		writeFpLimbs(mulResults[i][0])
		writeFpLimbs(mulResults[i][1])
	}

	for j := range addP1s {
		writeFpLimbs(addP1s[j][0])
		writeFpLimbs(addP1s[j][1])
		writeFpLimbs(addP2s[j][0])
		writeFpLimbs(addP2s[j][1])
		writeFpLimbs(addResults[j][0])
		writeFpLimbs(addResults[j][1])
	}

	digest := h.Sum(nil)
	result := new(big.Int).SetBytes(digest)
	frModulus := ecc.BN254.ScalarField()
	result.Mod(result, frModulus)
	return result
}

// ChunkOperations splits flat mul and add operations into chunks of at most chunkSize
// total operations. Returns a slice of (mulOps, addOps) pairs, one per chunk.
// Muls are filled first in each chunk, then adds fill the remaining capacity.
func ChunkOperations(
	allMulBasePoints [][2]*big.Int,
	allMulScalars []*big.Int,
	allMulResults [][2]*big.Int,
	allAddP1s [][2]*big.Int,
	allAddP2s [][2]*big.Int,
	allAddResults [][2]*big.Int,
	chunkSize int,
) []ChunkedECConfig {
	totalMul := len(allMulBasePoints)
	totalAdd := len(allAddP1s)
	totalOps := totalMul + totalAdd

	if totalOps == 0 {
		return nil
	}

	var chunks []ChunkedECConfig
	mulIdx := 0
	addIdx := 0

	for mulIdx < totalMul || addIdx < totalAdd {
		remaining := chunkSize
		chunkMul := 0
		chunkAdd := 0

		// Fill muls first
		if mulIdx < totalMul {
			canTake := totalMul - mulIdx
			if canTake > remaining {
				canTake = remaining
			}
			chunkMul = canTake
			mulIdx += canTake
			remaining -= canTake
		}

		// Fill adds with remaining capacity
		if addIdx < totalAdd && remaining > 0 {
			canTake := totalAdd - addIdx
			if canTake > remaining {
				canTake = remaining
			}
			chunkAdd = canTake
			addIdx += canTake
		}

		chunks = append(chunks, ChunkedECConfig{
			NumMulOps: chunkMul,
			NumAddOps: chunkAdd,
		})
	}

	return chunks
}

// MakeTestChunkedECAssignment creates a valid test assignment for the chunked circuit.
func MakeTestChunkedECAssignment(numMul, numAdd int, opsDigest *big.Int, chunkIndex, totalChunks int) *ChunkedECBatchCircuit {
	config := ChunkedECConfig{NumMulOps: numMul, NumAddOps: numAdd}
	assignment := AllocateChunkedECCircuit(config)

	assignment.TranscriptDigest = big.NewInt(42)
	assignment.CircuitHashLo = big.NewInt(1)
	assignment.CircuitHashHi = big.NewInt(2)
	assignment.OpsDigest = opsDigest
	assignment.ChunkIndex = big.NewInt(int64(chunkIndex))
	assignment.TotalChunks = big.NewInt(int64(totalChunks))

	_, _, g1, _ := bn254.Generators()

	for i := 0; i < numMul; i++ {
		baseMul := big.NewInt(int64(2*i + 3))
		s := big.NewInt(int64(i + 2))
		var base, result bn254.G1Affine
		base.ScalarMultiplication(&g1, baseMul)
		var combinedScalar big.Int
		combinedScalar.Mul(baseMul, s)
		result.ScalarMultiplication(&g1, &combinedScalar)
		assignment.MulBasePoints[i] = newG1PointAssignment(base)
		assignment.MulScalars[i] = s
		assignment.MulResults[i] = newG1PointAssignment(result)
	}

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

// buildTestOpsData builds coordinate arrays for test operations (matching MakeTestChunkedECAssignment).
// This is used to compute the OpsDigest for tests.
func buildTestOpsData(numMul, numAdd int) (
	mulBaseCoords [][2]*big.Int,
	mulScalars []*big.Int,
	mulResultCoords [][2]*big.Int,
	addP1Coords [][2]*big.Int,
	addP2Coords [][2]*big.Int,
	addResultCoords [][2]*big.Int,
) {
	_, _, g1, _ := bn254.Generators()

	mulBaseCoords = make([][2]*big.Int, numMul)
	mulScalars = make([]*big.Int, numMul)
	mulResultCoords = make([][2]*big.Int, numMul)

	for i := 0; i < numMul; i++ {
		baseMul := big.NewInt(int64(2*i + 3))
		s := big.NewInt(int64(i + 2))
		var base, result bn254.G1Affine
		base.ScalarMultiplication(&g1, baseMul)
		var combinedScalar big.Int
		combinedScalar.Mul(baseMul, s)
		result.ScalarMultiplication(&g1, &combinedScalar)

		var bx, by, rx, ry big.Int
		base.X.BigInt(&bx)
		base.Y.BigInt(&by)
		result.X.BigInt(&rx)
		result.Y.BigInt(&ry)
		mulBaseCoords[i] = [2]*big.Int{new(big.Int).Set(&bx), new(big.Int).Set(&by)}
		mulScalars[i] = new(big.Int).Set(s)
		mulResultCoords[i] = [2]*big.Int{new(big.Int).Set(&rx), new(big.Int).Set(&ry)}
	}

	addP1Coords = make([][2]*big.Int, numAdd)
	addP2Coords = make([][2]*big.Int, numAdd)
	addResultCoords = make([][2]*big.Int, numAdd)

	for j := 0; j < numAdd; j++ {
		s1 := big.NewInt(int64(2*j + 3))
		s2 := big.NewInt(int64(2*j + 5))
		var p1, p2, result bn254.G1Affine
		p1.ScalarMultiplication(&g1, s1)
		p2.ScalarMultiplication(&g1, s2)
		result.Add(&p1, &p2)

		var p1x, p1y, p2x, p2y, rx, ry big.Int
		p1.X.BigInt(&p1x)
		p1.Y.BigInt(&p1y)
		p2.X.BigInt(&p2x)
		p2.Y.BigInt(&p2y)
		result.X.BigInt(&rx)
		result.Y.BigInt(&ry)
		addP1Coords[j] = [2]*big.Int{new(big.Int).Set(&p1x), new(big.Int).Set(&p1y)}
		addP2Coords[j] = [2]*big.Int{new(big.Int).Set(&p2x), new(big.Int).Set(&p2y)}
		addResultCoords[j] = [2]*big.Int{new(big.Int).Set(&rx), new(big.Int).Set(&ry)}
	}

	return
}

// ComputeChunkedBatchChallengeOutOfCircuit computes the per-chunk batch challenge
// outside the circuit: MiMC(opsDigest || chunkIndex || chunk operation data).
// This matches the in-circuit computeChunkBatchChallenge exactly.
func ComputeChunkedBatchChallengeOutOfCircuit(
	opsDigest *big.Int,
	chunkIndex int,
	mulBasePoints [][2]*big.Int,
	mulScalars []*big.Int,
	mulResults [][2]*big.Int,
	addP1s [][2]*big.Int,
	addP2s [][2]*big.Int,
	addResults [][2]*big.Int,
) *big.Int {
	h := mimcbn254.NewMiMC()

	writeBigInt := func(v *big.Int) {
		var buf [32]byte
		b := v.Bytes()
		copy(buf[32-len(b):], b)
		h.Write(buf[:])
	}

	writeFpLimbs := func(v *big.Int) {
		limbs := fpToLimbs(v)
		for _, limb := range limbs {
			writeBigInt(limb)
		}
	}

	// Domain: bind to global ops digest and chunk index
	writeBigInt(opsDigest)
	writeBigInt(big.NewInt(int64(chunkIndex)))

	// Hash all ec_mul operation data in this chunk
	for i := range mulBasePoints {
		writeFpLimbs(mulBasePoints[i][0])
		writeFpLimbs(mulBasePoints[i][1])
		writeBigInt(mulScalars[i])
		writeFpLimbs(mulResults[i][0])
		writeFpLimbs(mulResults[i][1])
	}

	// Hash all ec_add operation data in this chunk
	for j := range addP1s {
		writeFpLimbs(addP1s[j][0])
		writeFpLimbs(addP1s[j][1])
		writeFpLimbs(addP2s[j][0])
		writeFpLimbs(addP2s[j][1])
		writeFpLimbs(addResults[j][0])
		writeFpLimbs(addResults[j][1])
	}

	digest := h.Sum(nil)
	result := new(big.Int).SetBytes(digest)
	frModulus := ecc.BN254.ScalarField()
	result.Mod(result, frModulus)
	return result
}

// compileChunkedECCircuit is a helper for tests that compiles the chunked circuit
// and returns the constraint count.
func compileChunkedECCircuit(config ChunkedECConfig) (int, error) {
	circuit := AllocateChunkedECCircuit(config)
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		return 0, err
	}
	return ccs.GetNbConstraints(), nil
}
