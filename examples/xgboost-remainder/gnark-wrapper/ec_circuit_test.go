package main

import (
	"fmt"
	"math/big"
	"testing"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark-crypto/ecc/bn254"
	"github.com/consensys/gnark/backend"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"
	"github.com/consensys/gnark/test"
)

func TestECCircuit_Compile_1Mul(t *testing.T) {
	config := ECCircuitConfig{NumMulOps: 1, NumAddOps: 0}
	circuit := AllocateECCircuit(config)
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		t.Fatalf("compile error: %v", err)
	}
	t.Logf("EC circuit (1 mul, 0 add): %d R1CS constraints", ccs.GetNbConstraints())
}

func TestECCircuit_Compile_1Add(t *testing.T) {
	config := ECCircuitConfig{NumMulOps: 0, NumAddOps: 1}
	circuit := AllocateECCircuit(config)
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		t.Fatalf("compile error: %v", err)
	}
	t.Logf("EC circuit (0 mul, 1 add): %d R1CS constraints", ccs.GetNbConstraints())
}

func TestECCircuit_Prove_1Mul(t *testing.T) {
	config := ECCircuitConfig{NumMulOps: 1, NumAddOps: 0}
	circuit := AllocateECCircuit(config)
	assignment := MakeTestECAssignment(1, 0)

	assert := test.NewAssert(t)
	assert.ProverSucceeded(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("EC circuit (1 mul) proof verified")
}

func TestECCircuit_Prove_1Add(t *testing.T) {
	config := ECCircuitConfig{NumMulOps: 0, NumAddOps: 1}
	circuit := AllocateECCircuit(config)
	assignment := MakeTestECAssignment(0, 1)

	assert := test.NewAssert(t)
	assert.ProverSucceeded(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("EC circuit (1 add) proof verified")
}

func TestECCircuit_Prove_2Mul_1Add(t *testing.T) {
	config := ECCircuitConfig{NumMulOps: 2, NumAddOps: 1}
	circuit := AllocateECCircuit(config)
	assignment := MakeTestECAssignment(2, 1)

	assert := test.NewAssert(t)
	assert.ProverSucceeded(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("EC circuit (2 mul, 1 add) proof verified")
}

func TestECCircuit_ConstraintCount(t *testing.T) {
	testCases := []struct {
		name string
		nMul int
		nAdd int
	}{
		{"1_mul", 1, 0},
		{"2_mul", 2, 0},
		{"5_mul", 5, 0},
		{"10_mul", 10, 0},
		{"1_add", 0, 1},
		{"5_add", 0, 5},
		{"10_add", 0, 10},
		{"5_mul_5_add", 5, 5},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			config := ECCircuitConfig{NumMulOps: tc.nMul, NumAddOps: tc.nAdd}
			circuit := AllocateECCircuit(config)
			ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
			if err != nil {
				t.Fatalf("compile error: %v", err)
			}
			t.Logf("EC circuit (%d mul, %d add): %d constraints", tc.nMul, tc.nAdd, ccs.GetNbConstraints())
		})
	}
}

// TestBatchChallenge_DeterministicFromOps verifies that the same operations
// produce the same batch challenge hash.
func TestBatchChallenge_DeterministicFromOps(t *testing.T) {
	_, _, g1, _ := bn254.Generators()
	var p2 bn254.G1Affine
	p2.ScalarMultiplication(&g1, big.NewInt(3))

	digest := big.NewInt(42)

	mulBases := [][2]*big.Int{g1XY(&g1)}
	mulScalars := []*big.Int{big.NewInt(3)}
	mulResults := [][2]*big.Int{g1XY(&p2)}

	h1 := ComputeBatchChallengeOutOfCircuit(digest, mulBases, mulScalars, mulResults, nil, nil, nil)
	h2 := ComputeBatchChallengeOutOfCircuit(digest, mulBases, mulScalars, mulResults, nil, nil, nil)

	if h1.Cmp(h2) != 0 {
		t.Fatalf("batch challenge not deterministic: %s != %s", h1, h2)
	}
	t.Logf("Deterministic batch challenge: 0x%x", h1)
}

// TestBatchChallenge_ChangesWithOps verifies that different operations
// produce different batch challenges.
func TestBatchChallenge_ChangesWithOps(t *testing.T) {
	_, _, g1, _ := bn254.Generators()
	var p2, p3 bn254.G1Affine
	p2.ScalarMultiplication(&g1, big.NewInt(3))
	p3.ScalarMultiplication(&g1, big.NewInt(5))

	digest := big.NewInt(42)

	// Hash with scalar=3
	h1 := ComputeBatchChallengeOutOfCircuit(
		digest,
		[][2]*big.Int{g1XY(&g1)},
		[]*big.Int{big.NewInt(3)},
		[][2]*big.Int{g1XY(&p2)},
		nil, nil, nil,
	)

	// Hash with scalar=5 (different operation)
	h2 := ComputeBatchChallengeOutOfCircuit(
		digest,
		[][2]*big.Int{g1XY(&g1)},
		[]*big.Int{big.NewInt(5)},
		[][2]*big.Int{g1XY(&p3)},
		nil, nil, nil,
	)

	if h1.Cmp(h2) == 0 {
		t.Fatal("batch challenge should differ for different operations")
	}
	t.Logf("Different challenges: 0x%x vs 0x%x", h1, h2)
}

// g1XY extracts (x, y) big.Int pair from a G1Affine.
func g1XY(p *bn254.G1Affine) [2]*big.Int {
	var x, y big.Int
	p.X.BigInt(&x)
	p.Y.BigInt(&y)
	return [2]*big.Int{&x, &y}
}

// ============================================================================
// Chunked EC Circuit Tests
// ============================================================================

// TestChunkedEC_SmallChunk compiles and proves a single chunk with 5 mul + 5 add.
func TestChunkedEC_SmallChunk(t *testing.T) {
	numMul := 5
	numAdd := 5

	// Build ops data to compute OpsDigest
	mulBase, mulSc, mulRes, addP1, addP2, addRes := buildTestOpsData(numMul, numAdd)
	opsDigest := ComputeOpsDigest(big.NewInt(42), mulBase, mulSc, mulRes, addP1, addP2, addRes)

	config := ChunkedECConfig{NumMulOps: numMul, NumAddOps: numAdd}
	circuit := AllocateChunkedECCircuit(config)
	assignment := MakeTestChunkedECAssignment(numMul, numAdd, opsDigest, 0, 1)

	assert := test.NewAssert(t)
	assert.ProverSucceeded(
		circuit,
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Logf("Chunked EC circuit (5 mul, 5 add, single chunk) proof verified")
}

// TestChunkedEC_MultiChunk splits 6 mul + 4 add into 2 chunks (5 ops each)
// and proves each chunk independently.
func TestChunkedEC_MultiChunk(t *testing.T) {
	totalMul := 6
	totalAdd := 4

	// Build ALL ops data for OpsDigest computation
	mulBase, mulSc, mulRes, addP1, addP2, addRes := buildTestOpsData(totalMul, totalAdd)
	opsDigest := ComputeOpsDigest(big.NewInt(42), mulBase, mulSc, mulRes, addP1, addP2, addRes)
	t.Logf("OpsDigest for all operations: 0x%x", opsDigest)

	// Chunk into 2 chunks of 5 ops max:
	// Chunk 0: 5 mul, 0 add
	// Chunk 1: 1 mul, 4 add
	chunks := ChunkOperations(mulBase, mulSc, mulRes, addP1, addP2, addRes, 5)
	if len(chunks) != 2 {
		t.Fatalf("expected 2 chunks, got %d", len(chunks))
	}
	t.Logf("Chunk 0: %d mul, %d add", chunks[0].NumMulOps, chunks[0].NumAddOps)
	t.Logf("Chunk 1: %d mul, %d add", chunks[1].NumMulOps, chunks[1].NumAddOps)

	// Verify each chunk independently
	for idx, chunkCfg := range chunks {
		t.Run(fmt.Sprintf("chunk_%d", idx), func(t *testing.T) {
			circuit := AllocateChunkedECCircuit(chunkCfg)

			// Build chunk assignment with correct offset operations
			assignment := AllocateChunkedECCircuit(chunkCfg)
			assignment.TranscriptDigest = big.NewInt(42)
			assignment.CircuitHashLo = big.NewInt(1)
			assignment.CircuitHashHi = big.NewInt(2)
			assignment.OpsDigest = opsDigest
			assignment.ChunkIndex = big.NewInt(int64(idx))
			assignment.TotalChunks = big.NewInt(int64(len(chunks)))

			// Calculate mul/add offsets for this chunk
			mulOffset := 0
			addOffset := 0
			for i := 0; i < idx; i++ {
				mulOffset += chunks[i].NumMulOps
				addOffset += chunks[i].NumAddOps
			}

			_, _, g1, _ := bn254.Generators()
			for i := 0; i < chunkCfg.NumMulOps; i++ {
				gi := mulOffset + i
				baseMul := big.NewInt(int64(2*gi + 3))
				s := big.NewInt(int64(gi + 2))
				var base, result bn254.G1Affine
				base.ScalarMultiplication(&g1, baseMul)
				var combinedScalar big.Int
				combinedScalar.Mul(baseMul, s)
				result.ScalarMultiplication(&g1, &combinedScalar)
				assignment.MulBasePoints[i] = newG1PointAssignment(base)
				assignment.MulScalars[i] = s
				assignment.MulResults[i] = newG1PointAssignment(result)
			}

			for j := 0; j < chunkCfg.NumAddOps; j++ {
				gj := addOffset + j
				s1 := big.NewInt(int64(2*gj + 3))
				s2 := big.NewInt(int64(2*gj + 5))
				var p1, p2, result bn254.G1Affine
				p1.ScalarMultiplication(&g1, s1)
				p2.ScalarMultiplication(&g1, s2)
				result.Add(&p1, &p2)
				assignment.AddP1s[j] = newG1PointAssignment(p1)
				assignment.AddP2s[j] = newG1PointAssignment(p2)
				assignment.AddResults[j] = newG1PointAssignment(result)
			}

			assert := test.NewAssert(t)
			assert.ProverSucceeded(
				circuit,
				assignment,
				test.WithCurves(ecc.BN254),
				test.WithBackends(backend.GROTH16),
			)
			t.Logf("Chunk %d (%d mul, %d add) proof verified", idx, chunkCfg.NumMulOps, chunkCfg.NumAddOps)
		})
	}
}

// TestChunkedEC_ConstraintCount logs per-chunk constraint counts for planning.
func TestChunkedEC_ConstraintCount(t *testing.T) {
	testCases := []struct {
		name string
		nMul int
		nAdd int
	}{
		{"5_mul_5_add", 5, 5},
		{"10_mul_0_add", 10, 0},
		{"0_mul_10_add", 0, 10},
		{"3_mul_2_add", 3, 2},
	}

	for _, tc := range testCases {
		t.Run(tc.name, func(t *testing.T) {
			config := ChunkedECConfig{NumMulOps: tc.nMul, NumAddOps: tc.nAdd}
			circuit := AllocateChunkedECCircuit(config)
			ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
			if err != nil {
				t.Fatalf("compile error: %v", err)
			}
			t.Logf("Chunked EC circuit (%d mul, %d add): %d constraints", tc.nMul, tc.nAdd, ccs.GetNbConstraints())
		})
	}
}

// TestChunkOperations verifies the chunking logic.
func TestChunkOperations(t *testing.T) {
	// 10 mul + 5 add, chunk size 4
	mulBase := make([][2]*big.Int, 10)
	mulSc := make([]*big.Int, 10)
	mulRes := make([][2]*big.Int, 10)
	addP1 := make([][2]*big.Int, 5)
	addP2 := make([][2]*big.Int, 5)
	addRes := make([][2]*big.Int, 5)

	for i := range mulBase {
		mulBase[i] = [2]*big.Int{big.NewInt(1), big.NewInt(2)}
		mulSc[i] = big.NewInt(int64(i + 1))
		mulRes[i] = [2]*big.Int{big.NewInt(3), big.NewInt(4)}
	}
	for i := range addP1 {
		addP1[i] = [2]*big.Int{big.NewInt(1), big.NewInt(2)}
		addP2[i] = [2]*big.Int{big.NewInt(3), big.NewInt(4)}
		addRes[i] = [2]*big.Int{big.NewInt(5), big.NewInt(6)}
	}

	chunks := ChunkOperations(mulBase, mulSc, mulRes, addP1, addP2, addRes, 4)

	// 10 mul / 4 = 3 full mul chunks (4,4,2) + 5 add / (remaining cap) = more chunks
	totalMul := 0
	totalAdd := 0
	for i, c := range chunks {
		t.Logf("Chunk %d: %d mul, %d add", i, c.NumMulOps, c.NumAddOps)
		totalMul += c.NumMulOps
		totalAdd += c.NumAddOps
	}
	if totalMul != 10 {
		t.Fatalf("expected 10 total mul ops, got %d", totalMul)
	}
	if totalAdd != 5 {
		t.Fatalf("expected 5 total add ops, got %d", totalAdd)
	}
	t.Logf("ChunkOperations: %d chunks covering 10 mul + 5 add with chunk_size=4", len(chunks))
}

// TestOpsDigest_Deterministic verifies that OpsDigest is deterministic.
func TestOpsDigest_Deterministic(t *testing.T) {
	numMul := 3
	numAdd := 2
	mulBase, mulSc, mulRes, addP1, addP2, addRes := buildTestOpsData(numMul, numAdd)
	digest1 := ComputeOpsDigest(big.NewInt(42), mulBase, mulSc, mulRes, addP1, addP2, addRes)
	digest2 := ComputeOpsDigest(big.NewInt(42), mulBase, mulSc, mulRes, addP1, addP2, addRes)

	if digest1.Cmp(digest2) != 0 {
		t.Fatal("OpsDigest not deterministic")
	}
	t.Logf("OpsDigest: 0x%x", digest1)
}
