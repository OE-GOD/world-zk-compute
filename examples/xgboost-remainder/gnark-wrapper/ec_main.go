package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"math/big"
	"os"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark-crypto/ecc/bn254"
	bn254fp "github.com/consensys/gnark-crypto/ecc/bn254/fp"
	"github.com/consensys/gnark/backend/groth16"
	groth16bn254 "github.com/consensys/gnark/backend/groth16/bn254"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"
	"github.com/consensys/gnark/std/math/emulated"
)

// ECWitnessJSON is the JSON format from gen_ec_groth16_witness.rs.
type ECWitnessJSON struct {
	TranscriptDigest string    `json:"transcriptDigest"`
	CircuitHash      [2]string `json:"circuitHash"`
	ECOperations     []ECOp    `json:"ecOperations"`
	Stats            struct {
		TotalEcMul    int `json:"totalEcMul"`
		TotalEcAdd    int `json:"totalEcAdd"`
		TotalMsm      int `json:"totalMsm"`
		TotalMsmPoints int `json:"totalMsmPoints"`
	} `json:"stats"`
}

// ECOp is a single EC operation from the witness.
type ECOp struct {
	Type    string          `json:"type"`
	Point   *ECPointJSON    `json:"point,omitempty"`   // for mul
	Scalar  string          `json:"scalar,omitempty"`  // for mul
	Result  *ECPointJSON    `json:"result"`
	Point1  *ECPointJSON    `json:"point1,omitempty"`  // for add
	Point2  *ECPointJSON    `json:"point2,omitempty"`  // for add
	Points  []ECPointJSON   `json:"points,omitempty"`  // for msm
	Scalars []string        `json:"scalars,omitempty"` // for msm
}

// ECPointJSON represents a BN254 G1 point in JSON.
type ECPointJSON struct {
	X string `json:"x"`
	Y string `json:"y"`
}

// flatMulOp is a decomposed ec_mul operation: result == scalar * point.
type flatMulOp struct {
	basePoint G1Point
	scalar    *big.Int
	result    G1Point
}

// flatAddOp is a decomposed ec_add operation: result == p1 + p2.
type flatAddOp struct {
	p1     G1Point
	p2     G1Point
	result G1Point
}

// decomposeECOps flattens all EC operations (including MSMs) into mul and add lists.
func decomposeECOps(ops []ECOp) ([]flatMulOp, []flatAddOp) {
	var muls []flatMulOp
	var adds []flatAddOp

	for _, op := range ops {
		switch op.Type {
		case "mul":
			muls = append(muls, flatMulOp{
				basePoint: parseECPoint(op.Point),
				scalar:    parseBigInt(op.Scalar),
				result:    parseECPoint(op.Result),
			})
		case "add":
			adds = append(adds, flatAddOp{
				p1:     parseECPoint(op.Point1),
				p2:     parseECPoint(op.Point2),
				result: parseECPoint(op.Result),
			})
		case "msm":
			// Decompose MSM into individual mul + sequential add:
			// result = s0*P0 + s1*P1 + ... + sN*PN
			// = mul(s0,P0) then add(partial, mul(si,Pi)) for i>0
			n := len(op.Points)
			if n == 0 {
				continue
			}

			// Compute intermediate results for verification
			// Each s_i * P_i is an ec_mul
			partialPoints := make([]bn254.G1Affine, n)
			for i := 0; i < n; i++ {
				pt := parseNativeG1(op.Points[i])
				sc := parseBigInt(op.Scalars[i])
				var res bn254.G1Affine
				res.ScalarMultiplication(&pt, sc)
				partialPoints[i] = res

				muls = append(muls, flatMulOp{
					basePoint: parseECPoint(&op.Points[i]),
					scalar:    sc,
					result:    nativeToG1Point(res),
				})
			}

			// Sequential additions: acc = partial[0]; acc += partial[i] for i>0
			if n > 1 {
				acc := partialPoints[0]
				for i := 1; i < n; i++ {
					var sum bn254.G1Affine
					sum.Add(&acc, &partialPoints[i])
					adds = append(adds, flatAddOp{
						p1:     nativeToG1Point(acc),
						p2:     nativeToG1Point(partialPoints[i]),
						result: nativeToG1Point(sum),
					})
					acc = sum
				}
			}
		}
	}

	return muls, adds
}

// parseECPoint converts a JSON point to gnark G1Point assignment.
func parseECPoint(p *ECPointJSON) G1Point {
	x := parseBigInt(p.X)
	y := parseBigInt(p.Y)
	return G1Point{
		X: emulated.ValueOf[emulated.BN254Fp](x),
		Y: emulated.ValueOf[emulated.BN254Fp](y),
	}
}

// parseNativeG1 converts a JSON point to a native bn254.G1Affine.
func parseNativeG1(p ECPointJSON) bn254.G1Affine {
	var pt bn254.G1Affine
	xBig := parseBigInt(p.X)
	yBig := parseBigInt(p.Y)
	pt.X.SetBigInt(xBig)
	pt.Y.SetBigInt(yBig)
	return pt
}

// nativeToG1Point converts a native bn254.G1Affine to gnark G1Point assignment.
func nativeToG1Point(p bn254.G1Affine) G1Point {
	var xBig, yBig big.Int
	p.X.BigInt(&xBig)
	p.Y.BigInt(&yBig)
	return G1Point{
		X: emulated.ValueOf[emulated.BN254Fp](&xBig),
		Y: emulated.ValueOf[emulated.BN254Fp](&yBig),
	}
}

// buildECAssignment creates a circuit assignment from decomposed EC operations.
// The batch challenge is computed in-circuit via MiMC hash — no external input needed.
func buildECAssignment(config ECCircuitConfig, muls []flatMulOp, adds []flatAddOp, w *ECWitnessJSON) *ECBatchCircuit {
	assignment := AllocateECCircuit(config)

	assignment.TranscriptDigest = parseBigInt(w.TranscriptDigest)
	assignment.CircuitHashLo = parseBigInt(w.CircuitHash[0])
	assignment.CircuitHashHi = parseBigInt(w.CircuitHash[1])

	for i := 0; i < config.NumMulOps; i++ {
		assignment.MulBasePoints[i] = muls[i].basePoint
		assignment.MulScalars[i] = muls[i].scalar
		assignment.MulResults[i] = muls[i].result
	}

	for j := 0; j < config.NumAddOps; j++ {
		assignment.AddP1s[j] = adds[j].p1
		assignment.AddP2s[j] = adds[j].p2
		assignment.AddResults[j] = adds[j].result
	}

	return assignment
}

// ecFpToHex converts a bn254 base field element to a 0x-prefixed hex string.
func ecFpToHex(e *bn254fp.Element) string {
	var b big.Int
	e.BigInt(&b)
	return fmt.Sprintf("0x%064x", &b)
}

// cmdECProveJSON generates a Groth16 proof for EC operations from witness JSON.
func cmdECProveJSON() {
	data, err := io.ReadAll(os.Stdin)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error reading stdin: %v\n", err)
		os.Exit(1)
	}

	var w ECWitnessJSON
	if err := json.Unmarshal(data, &w); err != nil {
		fmt.Fprintf(os.Stderr, "error parsing EC witness JSON: %v\n", err)
		os.Exit(1)
	}

	fmt.Fprintf(os.Stderr, "EC witness: %d mul, %d add, %d msm (%d msm points)\n",
		w.Stats.TotalEcMul, w.Stats.TotalEcAdd, w.Stats.TotalMsm, w.Stats.TotalMsmPoints)

	// Decompose all operations (MSMs become mul+add)
	muls, adds := decomposeECOps(w.ECOperations)
	fmt.Fprintf(os.Stderr, "After MSM decomposition: %d mul, %d add\n", len(muls), len(adds))

	config := ECCircuitConfig{
		NumMulOps: len(muls),
		NumAddOps: len(adds),
	}

	assignment := buildECAssignment(config, muls, adds, &w)

	// Redirect stdout to stderr during gnark compilation
	origStdout := os.Stdout
	os.Stdout = os.Stderr

	// Compile
	fmt.Fprintf(os.Stderr, "Compiling EC circuit (%d mul, %d add)...\n", config.NumMulOps, config.NumAddOps)
	circuit := AllocateECCircuit(config)
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}
	fmt.Fprintf(os.Stderr, "EC circuit compiled: %d R1CS constraints\n", ccs.GetNbConstraints())

	// Setup
	fmt.Fprintln(os.Stderr, "Running inline Groth16 setup...")
	pk, vk, err := groth16.Setup(ccs)
	if err != nil {
		fmt.Fprintf(os.Stderr, "setup error: %v\n", err)
		os.Exit(1)
	}

	// Export Solidity verifier
	solFile, solErr := os.Create("ECRemainderGroth16Verifier.sol")
	if solErr == nil {
		if exportErr := vk.ExportSolidity(solFile); exportErr == nil {
			fmt.Fprintln(os.Stderr, "Solidity verifier exported to ECRemainderGroth16Verifier.sol")
		}
		solFile.Close()
	}

	// Create witness
	fmt.Fprintln(os.Stderr, "Creating witness...")
	witness, err := frontend.NewWitness(assignment, ecc.BN254.ScalarField())

	// Restore stdout
	os.Stdout = origStdout
	if err != nil {
		fmt.Fprintf(os.Stderr, "error creating witness: %v\n", err)
		os.Exit(1)
	}

	// Prove
	fmt.Fprintln(os.Stderr, "Generating Groth16 proof...")
	proof, err := groth16.Prove(ccs, pk, witness)
	if err != nil {
		fmt.Fprintf(os.Stderr, "prove error: %v\n", err)
		os.Exit(1)
	}

	// Verify
	fmt.Fprintln(os.Stderr, "Verifying proof locally...")
	publicWitness, _ := witness.Public()
	if err := groth16.Verify(proof, vk, publicWitness); err != nil {
		fmt.Fprintf(os.Stderr, "verification failed: %v\n", err)
		os.Exit(1)
	}
	fmt.Fprintln(os.Stderr, "Proof verified locally!")

	// Extract proof points
	var proofBuf bytes.Buffer
	proof.WriteTo(&proofBuf)
	proofBn254 := proof.(*groth16bn254.Proof)

	proofPoints := make([]string, 8)
	proofPoints[0] = ecFpToHex(&proofBn254.Ar.X)
	proofPoints[1] = ecFpToHex(&proofBn254.Ar.Y)
	proofPoints[2] = ecFpToHex(&proofBn254.Bs.X.A1)
	proofPoints[3] = ecFpToHex(&proofBn254.Bs.X.A0)
	proofPoints[4] = ecFpToHex(&proofBn254.Bs.Y.A1)
	proofPoints[5] = ecFpToHex(&proofBn254.Bs.Y.A0)
	proofPoints[6] = ecFpToHex(&proofBn254.Krs.X)
	proofPoints[7] = ecFpToHex(&proofBn254.Krs.Y)

	// Build public inputs: transcriptDigest, circuitHashLo, circuitHashHi
	pubInputs := []string{
		w.TranscriptDigest,
		w.CircuitHash[0],
		w.CircuitHash[1],
	}

	fixture := map[string]interface{}{
		"proof":         proofPoints,
		"public_inputs": pubInputs,
		"stats": map[string]interface{}{
			"num_mul_ops":    config.NumMulOps,
			"num_add_ops":    config.NumAddOps,
			"r1cs_constraints": ccs.GetNbConstraints(),
		},
	}

	out, _ := json.MarshalIndent(fixture, "", "  ")
	fmt.Println(string(out))
	fmt.Fprintf(os.Stderr, "EC JSON fixture written (%d proof points, %d public inputs, %d constraints)\n",
		len(proofPoints), len(pubInputs), ccs.GetNbConstraints())
}

// ChunkedECWitnessJSON is the JSON format for a single chunk of EC operations.
type ChunkedECWitnessJSON struct {
	TranscriptDigest string    `json:"transcriptDigest"`
	CircuitHash      [2]string `json:"circuitHash"`
	OpsDigest        string    `json:"opsDigest"`
	ChunkIndex       int       `json:"chunkIndex"`
	TotalChunks      int       `json:"totalChunks"`
	MulOps           []ECOp    `json:"mulOps"`
	AddOps           []ECOp    `json:"addOps"`
}

// cmdECChunkedProveJSON generates a Groth16 proof for a single chunk of EC operations.
func cmdECChunkedProveJSON() {
	data, err := io.ReadAll(os.Stdin)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error reading stdin: %v\n", err)
		os.Exit(1)
	}

	var w ChunkedECWitnessJSON
	if err := json.Unmarshal(data, &w); err != nil {
		fmt.Fprintf(os.Stderr, "error parsing chunked EC witness JSON: %v\n", err)
		os.Exit(1)
	}

	fmt.Fprintf(os.Stderr, "Chunked EC witness: chunk %d/%d, %d mul ops, %d add ops\n",
		w.ChunkIndex, w.TotalChunks, len(w.MulOps), len(w.AddOps))

	// Parse mul ops
	numMul := len(w.MulOps)
	numAdd := len(w.AddOps)

	config := ChunkedECConfig{NumMulOps: numMul, NumAddOps: numAdd}
	assignment := AllocateChunkedECCircuit(config)

	assignment.TranscriptDigest = parseBigInt(w.TranscriptDigest)
	assignment.CircuitHashLo = parseBigInt(w.CircuitHash[0])
	assignment.CircuitHashHi = parseBigInt(w.CircuitHash[1])
	assignment.OpsDigest = parseBigInt(w.OpsDigest)
	assignment.ChunkIndex = big.NewInt(int64(w.ChunkIndex))
	assignment.TotalChunks = big.NewInt(int64(w.TotalChunks))

	for i, op := range w.MulOps {
		assignment.MulBasePoints[i] = parseECPoint(op.Point)
		assignment.MulScalars[i] = parseBigInt(op.Scalar)
		assignment.MulResults[i] = parseECPoint(op.Result)
	}

	for j, op := range w.AddOps {
		assignment.AddP1s[j] = parseECPoint(op.Point1)
		assignment.AddP2s[j] = parseECPoint(op.Point2)
		assignment.AddResults[j] = parseECPoint(op.Result)
	}

	// Redirect stdout to stderr during gnark compilation
	origStdout := os.Stdout
	os.Stdout = os.Stderr

	// Compile
	fmt.Fprintf(os.Stderr, "Compiling chunked EC circuit (chunk %d: %d mul, %d add)...\n",
		w.ChunkIndex, numMul, numAdd)
	circuit := AllocateChunkedECCircuit(config)
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}
	fmt.Fprintf(os.Stderr, "Chunked EC circuit compiled: %d R1CS constraints\n", ccs.GetNbConstraints())

	// Setup
	fmt.Fprintln(os.Stderr, "Running inline Groth16 setup...")
	pk, vk, err := groth16.Setup(ccs)
	if err != nil {
		fmt.Fprintf(os.Stderr, "setup error: %v\n", err)
		os.Exit(1)
	}

	// Export Solidity verifier
	solFileName := fmt.Sprintf("ChunkedECGroth16Verifier_chunk%d.sol", w.ChunkIndex)
	solFile, solErr := os.Create(solFileName)
	if solErr == nil {
		if exportErr := vk.ExportSolidity(solFile); exportErr == nil {
			fmt.Fprintf(os.Stderr, "Solidity verifier exported to %s\n", solFileName)
		}
		solFile.Close()
	}

	// Create witness
	fmt.Fprintln(os.Stderr, "Creating witness...")
	witness, err := frontend.NewWitness(assignment, ecc.BN254.ScalarField())

	// Restore stdout
	os.Stdout = origStdout
	if err != nil {
		fmt.Fprintf(os.Stderr, "error creating witness: %v\n", err)
		os.Exit(1)
	}

	// Prove
	fmt.Fprintln(os.Stderr, "Generating Groth16 proof...")
	proof, err := groth16.Prove(ccs, pk, witness)
	if err != nil {
		fmt.Fprintf(os.Stderr, "prove error: %v\n", err)
		os.Exit(1)
	}

	// Verify
	fmt.Fprintln(os.Stderr, "Verifying proof locally...")
	publicWitness, _ := witness.Public()
	if err := groth16.Verify(proof, vk, publicWitness); err != nil {
		fmt.Fprintf(os.Stderr, "verification failed: %v\n", err)
		os.Exit(1)
	}
	fmt.Fprintln(os.Stderr, "Proof verified locally!")

	// Extract proof points
	var proofBuf bytes.Buffer
	proof.WriteTo(&proofBuf)
	proofBn254 := proof.(*groth16bn254.Proof)

	proofPoints := make([]string, 8)
	proofPoints[0] = ecFpToHex(&proofBn254.Ar.X)
	proofPoints[1] = ecFpToHex(&proofBn254.Ar.Y)
	proofPoints[2] = ecFpToHex(&proofBn254.Bs.X.A1)
	proofPoints[3] = ecFpToHex(&proofBn254.Bs.X.A0)
	proofPoints[4] = ecFpToHex(&proofBn254.Bs.Y.A1)
	proofPoints[5] = ecFpToHex(&proofBn254.Bs.Y.A0)
	proofPoints[6] = ecFpToHex(&proofBn254.Krs.X)
	proofPoints[7] = ecFpToHex(&proofBn254.Krs.Y)

	// Build public inputs: transcriptDigest, circuitHashLo, circuitHashHi, opsDigest, chunkIndex, totalChunks
	pubInputs := []string{
		w.TranscriptDigest,
		w.CircuitHash[0],
		w.CircuitHash[1],
		w.OpsDigest,
		fmt.Sprintf("0x%x", w.ChunkIndex),
		fmt.Sprintf("0x%x", w.TotalChunks),
	}

	fixture := map[string]interface{}{
		"proof":         proofPoints,
		"public_inputs": pubInputs,
		"stats": map[string]interface{}{
			"chunk_index":      w.ChunkIndex,
			"total_chunks":     w.TotalChunks,
			"num_mul_ops":      numMul,
			"num_add_ops":      numAdd,
			"r1cs_constraints": ccs.GetNbConstraints(),
		},
	}

	out, _ := json.MarshalIndent(fixture, "", "  ")
	fmt.Println(string(out))
	fmt.Fprintf(os.Stderr, "Chunked EC JSON fixture written (chunk %d/%d, %d proof points, %d public inputs, %d constraints)\n",
		w.ChunkIndex, w.TotalChunks, len(proofPoints), len(pubInputs), ccs.GetNbConstraints())
}

// cmdECInfo shows EC circuit statistics from witness JSON on stdin.
func cmdECInfo() {
	data, err := io.ReadAll(os.Stdin)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error reading stdin: %v\n", err)
		os.Exit(1)
	}

	var w ECWitnessJSON
	if err := json.Unmarshal(data, &w); err != nil {
		fmt.Fprintf(os.Stderr, "error parsing EC witness JSON: %v\n", err)
		os.Exit(1)
	}

	fmt.Fprintf(os.Stderr, "EC witness stats from Rust:\n")
	fmt.Fprintf(os.Stderr, "  ec_mul:  %d\n", w.Stats.TotalEcMul)
	fmt.Fprintf(os.Stderr, "  ec_add:  %d\n", w.Stats.TotalEcAdd)
	fmt.Fprintf(os.Stderr, "  msm:     %d (%d total points)\n", w.Stats.TotalMsm, w.Stats.TotalMsmPoints)

	muls, adds := decomposeECOps(w.ECOperations)
	fmt.Fprintf(os.Stderr, "\nAfter MSM decomposition:\n")
	fmt.Fprintf(os.Stderr, "  flat mul ops: %d\n", len(muls))
	fmt.Fprintf(os.Stderr, "  flat add ops: %d\n", len(adds))

	config := ECCircuitConfig{
		NumMulOps: len(muls),
		NumAddOps: len(adds),
	}

	fmt.Fprintf(os.Stderr, "\nCompiling EC circuit (%d mul, %d add)...\n", config.NumMulOps, config.NumAddOps)

	circuit := AllocateECCircuit(config)
	origStdout := os.Stdout
	os.Stdout = os.Stderr
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	os.Stdout = origStdout
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("EC Circuit Stats:\n")
	fmt.Printf("  Mul ops: %d\n", config.NumMulOps)
	fmt.Printf("  Add ops: %d\n", config.NumAddOps)
	fmt.Printf("  R1CS constraints: %d\n", ccs.GetNbConstraints())
	fmt.Printf("  Public inputs: 3 (transcriptDigest, circuitHashLo, circuitHashHi)\n")
}
