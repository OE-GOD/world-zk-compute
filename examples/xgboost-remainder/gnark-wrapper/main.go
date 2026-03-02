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
	"github.com/rs/zerolog"
)

func init() {
	// Redirect gnark's zerolog output to stderr so stdout is clean for JSON output
	zerolog.SetGlobalLevel(zerolog.WarnLevel)
}

func main() {
	if len(os.Args) < 2 {
		printUsage()
		os.Exit(1)
	}

	switch os.Args[1] {
	case "setup":
		cmdSetup()
	case "prove":
		cmdProve()
	case "prove-json":
		cmdProveJSON()
	case "export-solidity":
		cmdExportSolidity()
	case "info":
		cmdInfo()
	default:
		fmt.Fprintf(os.Stderr, "unknown command: %s\n", os.Args[1])
		printUsage()
		os.Exit(1)
	}
}

func printUsage() {
	fmt.Println("Usage: gnark-wrapper <command>")
	fmt.Println()
	fmt.Println("Commands:")
	fmt.Println("  setup           Generate proving key and verification key")
	fmt.Println("  prove           Generate a Groth16 proof from witness JSON (stdin)")
	fmt.Println("  prove-json      Like prove, but output JSON fixture for Solidity tests")
	fmt.Println("  export-solidity Export Solidity verifier contract")
	fmt.Println("  info            Show circuit info (constraints, etc.)")
}

// newCircuit returns the circuit definition with correct sizes for the test proof.
func newCircuit() *RemainderWrapperCircuit {
	return AllocateCircuit()
}

// cmdSetup compiles the circuit and runs Groth16 trusted setup.
func cmdSetup() {
	fmt.Fprintln(os.Stderr, "Compiling circuit...")
	circuit := newCircuit()
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}
	fmt.Fprintf(os.Stderr, "Circuit compiled: %d constraints\n", ccs.GetNbConstraints())

	fmt.Fprintln(os.Stderr, "Running Groth16 setup...")
	pk, vk, err := groth16.Setup(ccs)
	if err != nil {
		fmt.Fprintf(os.Stderr, "setup error: %v\n", err)
		os.Exit(1)
	}

	// Save keys
	pkFile, err := os.Create("proving_key.bin")
	if err != nil {
		fmt.Fprintf(os.Stderr, "error creating pk file: %v\n", err)
		os.Exit(1)
	}
	defer pkFile.Close()
	pk.WriteTo(pkFile)

	vkFile, err := os.Create("verification_key.bin")
	if err != nil {
		fmt.Fprintf(os.Stderr, "error creating vk file: %v\n", err)
		os.Exit(1)
	}
	defer vkFile.Close()
	vk.WriteTo(vkFile)

	fmt.Fprintln(os.Stderr, "Setup complete. Keys saved to proving_key.bin and verification_key.bin")
}

// WitnessJSON is the JSON format for the witness data from the Rust generator.
type WitnessJSON struct {
	PublicInputs struct {
		CircuitHash     [2]string `json:"circuit_hash"`
		PublicValues    [2]string `json:"public_values"`
		OutputChallenge string    `json:"output_challenge"`
		ClaimAggCoeff   string    `json:"claim_agg_coeff"`
		InterLayerCoeff string    `json:"inter_layer_coeff"`
		Layer0          struct {
			Bindings      []string `json:"bindings"`
			Rhos          []string `json:"rhos"`
			Gammas        []string `json:"gammas"`
			PODPChallenge string   `json:"podp_challenge"`
		} `json:"layer_0"`
		Layer1 struct {
			Bindings      []string `json:"bindings"`
			Rhos          []string `json:"rhos"`
			Gammas        []string `json:"gammas"`
			PODPChallenge string   `json:"podp_challenge"`
			PopChallenge  string   `json:"pop_challenge"`
		} `json:"layer_1"`
		InputRLCCoeffs     [2]string `json:"input_rlc_coeffs"`
		InputPODPChallenge string    `json:"input_podp_challenge"`
	} `json:"public_inputs"`
	PublicOutputs struct {
		RlcBeta0   string `json:"rlc_beta_0"`
		RlcBeta1   string `json:"rlc_beta_1"`
		ZDotJStar0 string `json:"z_dot_jstar_0"`
		ZDotJStar1 string `json:"z_dot_jstar_1"`
		LTensor0   string `json:"l_tensor_0"`
		LTensor1   string `json:"l_tensor_1"`
		ZDotR      string `json:"z_dot_r"`
		MLEEval    string `json:"mle_eval"`
	} `json:"public_outputs"`
	Witness struct {
		Layer0PODP struct {
			ZVector []string `json:"z_vector"`
			ZDelta  string   `json:"z_delta"`
			ZBeta   string   `json:"z_beta"`
		} `json:"layer_0_podp"`
		Layer1PODP struct {
			ZVector []string `json:"z_vector"`
			ZDelta  string   `json:"z_delta"`
			ZBeta   string   `json:"z_beta"`
		} `json:"layer_1_podp"`
		Layer1Pop struct {
			Z1 string `json:"z1"`
			Z2 string `json:"z2"`
			Z3 string `json:"z3"`
			Z4 string `json:"z4"`
			Z5 string `json:"z5"`
		} `json:"layer_1_pop"`
		InputPODP struct {
			ZVector []string `json:"z_vector"`
			ZDelta  string   `json:"z_delta"`
			ZBeta   string   `json:"z_beta"`
		} `json:"input_podp"`
	} `json:"witness"`
}

func parseBigInt(s string) *big.Int {
	v := new(big.Int)
	if len(s) > 2 && s[:2] == "0x" {
		v.SetString(s[2:], 16)
	} else {
		v.SetString(s, 10)
	}
	return v
}

// cmdProve generates a Groth16 proof from witness JSON on stdin.
func cmdProve() {
	// Read witness JSON from stdin
	data, err := io.ReadAll(os.Stdin)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error reading stdin: %v\n", err)
		os.Exit(1)
	}

	var w WitnessJSON
	if err := json.Unmarshal(data, &w); err != nil {
		fmt.Fprintf(os.Stderr, "error parsing witness JSON: %v\n", err)
		os.Exit(1)
	}

	// Build assignment
	assignment := &RemainderWrapperCircuit{
		CircuitHash0:    parseBigInt(w.PublicInputs.CircuitHash[0]),
		CircuitHash1:    parseBigInt(w.PublicInputs.CircuitHash[1]),
		PublicInput0:    parseBigInt(w.PublicInputs.PublicValues[0]),
		PublicInput1:    parseBigInt(w.PublicInputs.PublicValues[1]),
		OutputChallenge: parseBigInt(w.PublicInputs.OutputChallenge),
		ClaimAggCoeff:   parseBigInt(w.PublicInputs.ClaimAggCoeff),
		InterLayerCoeff: parseBigInt(w.PublicInputs.InterLayerCoeff),

		Layer0Bindings:      [1]frontend.Variable{parseBigInt(w.PublicInputs.Layer0.Bindings[0])},
		Layer0Rhos:          [2]frontend.Variable{parseBigInt(w.PublicInputs.Layer0.Rhos[0]), parseBigInt(w.PublicInputs.Layer0.Rhos[1])},
		Layer0Gammas:        [1]frontend.Variable{parseBigInt(w.PublicInputs.Layer0.Gammas[0])},
		Layer0PODPChallenge: parseBigInt(w.PublicInputs.Layer0.PODPChallenge),

		Layer1Bindings:      [1]frontend.Variable{parseBigInt(w.PublicInputs.Layer1.Bindings[0])},
		Layer1Rhos:          [2]frontend.Variable{parseBigInt(w.PublicInputs.Layer1.Rhos[0]), parseBigInt(w.PublicInputs.Layer1.Rhos[1])},
		Layer1Gammas:        [1]frontend.Variable{parseBigInt(w.PublicInputs.Layer1.Gammas[0])},
		Layer1PODPChallenge: parseBigInt(w.PublicInputs.Layer1.PODPChallenge),
		Layer1PopChallenge:  parseBigInt(w.PublicInputs.Layer1.PopChallenge),

		InputRLCCoeff0:     parseBigInt(w.PublicInputs.InputRLCCoeffs[0]),
		InputRLCCoeff1:     parseBigInt(w.PublicInputs.InputRLCCoeffs[1]),
		InputPODPChallenge: parseBigInt(w.PublicInputs.InputPODPChallenge),

		// Public outputs
		RlcBeta0:   parseBigInt(w.PublicOutputs.RlcBeta0),
		RlcBeta1:   parseBigInt(w.PublicOutputs.RlcBeta1),
		ZDotJStar0: parseBigInt(w.PublicOutputs.ZDotJStar0),
		ZDotJStar1: parseBigInt(w.PublicOutputs.ZDotJStar1),
		LTensor0:   parseBigInt(w.PublicOutputs.LTensor0),
		LTensor1:   parseBigInt(w.PublicOutputs.LTensor1),
		ZDotR:      parseBigInt(w.PublicOutputs.ZDotR),
		MLEEval:    parseBigInt(w.PublicOutputs.MLEEval),
	}

	// Parse PODP witnesses
	assignment.Layer0PODP = PODPWitness{
		ZVector: parseVarSlice(w.Witness.Layer0PODP.ZVector),
		ZDelta:  parseBigInt(w.Witness.Layer0PODP.ZDelta),
		ZBeta:   parseBigInt(w.Witness.Layer0PODP.ZBeta),
	}
	assignment.Layer1PODP = PODPWitness{
		ZVector: parseVarSlice(w.Witness.Layer1PODP.ZVector),
		ZDelta:  parseBigInt(w.Witness.Layer1PODP.ZDelta),
		ZBeta:   parseBigInt(w.Witness.Layer1PODP.ZBeta),
	}
	assignment.Layer1Pop = PopWitness{
		Z1: parseBigInt(w.Witness.Layer1Pop.Z1),
		Z2: parseBigInt(w.Witness.Layer1Pop.Z2),
		Z3: parseBigInt(w.Witness.Layer1Pop.Z3),
		Z4: parseBigInt(w.Witness.Layer1Pop.Z4),
		Z5: parseBigInt(w.Witness.Layer1Pop.Z5),
	}
	assignment.InputPODP = PODPWitness{
		ZVector: parseVarSlice(w.Witness.InputPODP.ZVector),
		ZDelta:  parseBigInt(w.Witness.InputPODP.ZDelta),
		ZBeta:   parseBigInt(w.Witness.InputPODP.ZBeta),
	}

	// Compile circuit
	fmt.Fprintln(os.Stderr, "Compiling circuit...")
	circuit := newCircuit()
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}
	fmt.Fprintf(os.Stderr, "Circuit compiled: %d constraints\n", ccs.GetNbConstraints())

	// Load proving key
	fmt.Fprintln(os.Stderr, "Loading proving key...")
	pkFile, err := os.Open("proving_key.bin")
	if err != nil {
		fmt.Fprintf(os.Stderr, "error opening pk: %v\n (run 'setup' first)\n", err)
		os.Exit(1)
	}
	defer pkFile.Close()
	pk := groth16.NewProvingKey(ecc.BN254)
	_, err = pk.ReadFrom(pkFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error reading pk: %v\n", err)
		os.Exit(1)
	}

	// Create witness
	fmt.Fprintln(os.Stderr, "Creating witness...")
	witness, err := frontend.NewWitness(assignment, ecc.BN254.ScalarField())
	if err != nil {
		fmt.Fprintf(os.Stderr, "error creating witness: %v\n", err)
		os.Exit(1)
	}

	// Generate proof
	fmt.Fprintln(os.Stderr, "Generating Groth16 proof...")
	proof, err := groth16.Prove(ccs, pk, witness)
	if err != nil {
		fmt.Fprintf(os.Stderr, "prove error: %v\n", err)
		os.Exit(1)
	}

	// Verify locally
	fmt.Fprintln(os.Stderr, "Verifying proof locally...")
	vkFile, err := os.Open("verification_key.bin")
	if err != nil {
		fmt.Fprintf(os.Stderr, "error opening vk: %v\n", err)
		os.Exit(1)
	}
	defer vkFile.Close()
	vk := groth16.NewVerifyingKey(ecc.BN254)
	_, err = vk.ReadFrom(vkFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error reading vk: %v\n", err)
		os.Exit(1)
	}
	publicWitness, _ := witness.Public()
	if err := groth16.Verify(proof, vk, publicWitness); err != nil {
		fmt.Fprintf(os.Stderr, "verification failed: %v\n", err)
		os.Exit(1)
	}
	fmt.Fprintln(os.Stderr, "Proof verified locally!")

	// Write proof to stdout as binary
	_, err = proof.WriteTo(os.Stdout)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error writing proof: %v\n", err)
		os.Exit(1)
	}
}

// cmdProveJSON generates a Groth16 proof and outputs a JSON fixture for Solidity tests.
// Output format: { "proof": [8 uint256 hex strings], "public_inputs": [29 uint256 hex strings] }
func cmdProveJSON() {
	// Read witness JSON from stdin
	data, err := io.ReadAll(os.Stdin)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error reading stdin: %v\n", err)
		os.Exit(1)
	}

	var w WitnessJSON
	if err := json.Unmarshal(data, &w); err != nil {
		fmt.Fprintf(os.Stderr, "error parsing witness JSON: %v\n", err)
		os.Exit(1)
	}

	// Build assignment (same as cmdProve)
	assignment := &RemainderWrapperCircuit{
		CircuitHash0:    parseBigInt(w.PublicInputs.CircuitHash[0]),
		CircuitHash1:    parseBigInt(w.PublicInputs.CircuitHash[1]),
		PublicInput0:    parseBigInt(w.PublicInputs.PublicValues[0]),
		PublicInput1:    parseBigInt(w.PublicInputs.PublicValues[1]),
		OutputChallenge: parseBigInt(w.PublicInputs.OutputChallenge),
		ClaimAggCoeff:   parseBigInt(w.PublicInputs.ClaimAggCoeff),
		InterLayerCoeff: parseBigInt(w.PublicInputs.InterLayerCoeff),

		Layer0Bindings:      [1]frontend.Variable{parseBigInt(w.PublicInputs.Layer0.Bindings[0])},
		Layer0Rhos:          [2]frontend.Variable{parseBigInt(w.PublicInputs.Layer0.Rhos[0]), parseBigInt(w.PublicInputs.Layer0.Rhos[1])},
		Layer0Gammas:        [1]frontend.Variable{parseBigInt(w.PublicInputs.Layer0.Gammas[0])},
		Layer0PODPChallenge: parseBigInt(w.PublicInputs.Layer0.PODPChallenge),

		Layer1Bindings:      [1]frontend.Variable{parseBigInt(w.PublicInputs.Layer1.Bindings[0])},
		Layer1Rhos:          [2]frontend.Variable{parseBigInt(w.PublicInputs.Layer1.Rhos[0]), parseBigInt(w.PublicInputs.Layer1.Rhos[1])},
		Layer1Gammas:        [1]frontend.Variable{parseBigInt(w.PublicInputs.Layer1.Gammas[0])},
		Layer1PODPChallenge: parseBigInt(w.PublicInputs.Layer1.PODPChallenge),
		Layer1PopChallenge:  parseBigInt(w.PublicInputs.Layer1.PopChallenge),

		InputRLCCoeff0:     parseBigInt(w.PublicInputs.InputRLCCoeffs[0]),
		InputRLCCoeff1:     parseBigInt(w.PublicInputs.InputRLCCoeffs[1]),
		InputPODPChallenge: parseBigInt(w.PublicInputs.InputPODPChallenge),

		RlcBeta0:   parseBigInt(w.PublicOutputs.RlcBeta0),
		RlcBeta1:   parseBigInt(w.PublicOutputs.RlcBeta1),
		ZDotJStar0: parseBigInt(w.PublicOutputs.ZDotJStar0),
		ZDotJStar1: parseBigInt(w.PublicOutputs.ZDotJStar1),
		LTensor0:   parseBigInt(w.PublicOutputs.LTensor0),
		LTensor1:   parseBigInt(w.PublicOutputs.LTensor1),
		ZDotR:      parseBigInt(w.PublicOutputs.ZDotR),
		MLEEval:    parseBigInt(w.PublicOutputs.MLEEval),
	}
	assignment.Layer0PODP = PODPWitness{
		ZVector: parseVarSlice(w.Witness.Layer0PODP.ZVector),
		ZDelta:  parseBigInt(w.Witness.Layer0PODP.ZDelta),
		ZBeta:   parseBigInt(w.Witness.Layer0PODP.ZBeta),
	}
	assignment.Layer1PODP = PODPWitness{
		ZVector: parseVarSlice(w.Witness.Layer1PODP.ZVector),
		ZDelta:  parseBigInt(w.Witness.Layer1PODP.ZDelta),
		ZBeta:   parseBigInt(w.Witness.Layer1PODP.ZBeta),
	}
	assignment.Layer1Pop = PopWitness{
		Z1: parseBigInt(w.Witness.Layer1Pop.Z1),
		Z2: parseBigInt(w.Witness.Layer1Pop.Z2),
		Z3: parseBigInt(w.Witness.Layer1Pop.Z3),
		Z4: parseBigInt(w.Witness.Layer1Pop.Z4),
		Z5: parseBigInt(w.Witness.Layer1Pop.Z5),
	}
	assignment.InputPODP = PODPWitness{
		ZVector: parseVarSlice(w.Witness.InputPODP.ZVector),
		ZDelta:  parseBigInt(w.Witness.InputPODP.ZDelta),
		ZBeta:   parseBigInt(w.Witness.InputPODP.ZBeta),
	}

	// Compile circuit
	fmt.Fprintln(os.Stderr, "Compiling circuit...")
	circuit := newCircuit()
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}
	fmt.Fprintf(os.Stderr, "Circuit compiled: %d constraints\n", ccs.GetNbConstraints())

	// Load proving key
	fmt.Fprintln(os.Stderr, "Loading proving key...")
	pkFile, err := os.Open("proving_key.bin")
	if err != nil {
		fmt.Fprintf(os.Stderr, "error opening pk: %v\n (run 'setup' first)\n", err)
		os.Exit(1)
	}
	defer pkFile.Close()
	pk := groth16.NewProvingKey(ecc.BN254)
	_, err = pk.ReadFrom(pkFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error reading pk: %v\n", err)
		os.Exit(1)
	}

	// Create witness
	fmt.Fprintln(os.Stderr, "Creating witness...")
	witness, err := frontend.NewWitness(assignment, ecc.BN254.ScalarField())
	if err != nil {
		fmt.Fprintf(os.Stderr, "error creating witness: %v\n", err)
		os.Exit(1)
	}

	// Generate proof
	fmt.Fprintln(os.Stderr, "Generating Groth16 proof...")
	proof, err := groth16.Prove(ccs, pk, witness)
	if err != nil {
		fmt.Fprintf(os.Stderr, "prove error: %v\n", err)
		os.Exit(1)
	}

	// Verify locally
	fmt.Fprintln(os.Stderr, "Verifying proof locally...")
	vkFile, err := os.Open("verification_key.bin")
	if err != nil {
		fmt.Fprintf(os.Stderr, "error opening vk: %v\n", err)
		os.Exit(1)
	}
	defer vkFile.Close()
	vk := groth16.NewVerifyingKey(ecc.BN254)
	_, err = vk.ReadFrom(vkFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error reading vk: %v\n", err)
		os.Exit(1)
	}
	publicWitness, _ := witness.Public()
	if err := groth16.Verify(proof, vk, publicWitness); err != nil {
		fmt.Fprintf(os.Stderr, "verification failed: %v\n", err)
		os.Exit(1)
	}
	fmt.Fprintln(os.Stderr, "Proof verified locally!")

	// Extract proof points (A, B, C) from the bn254-typed proof
	var proofBuf bytes.Buffer
	proof.WriteTo(&proofBuf)
	proofBn254 := proof.(*groth16bn254.Proof)

	// Groth16 proof: A (G1), B (G2), C (G1)
	// Solidity expects: [A.x, A.y, B.x1, B.x0, B.y1, B.y0, C.x, C.y]
	proofPoints := make([]string, 8)
	proofPoints[0] = fpToHex(&proofBn254.Ar.X)
	proofPoints[1] = fpToHex(&proofBn254.Ar.Y)
	// B is a G2 point: X = (X.A0, X.A1), Y = (Y.A0, Y.A1)
	// Solidity expects [X.A1, X.A0, Y.A1, Y.A0] (big-endian order for Fp2)
	proofPoints[2] = fpToHex(&proofBn254.Bs.X.A1)
	proofPoints[3] = fpToHex(&proofBn254.Bs.X.A0)
	proofPoints[4] = fpToHex(&proofBn254.Bs.Y.A1)
	proofPoints[5] = fpToHex(&proofBn254.Bs.Y.A0)
	proofPoints[6] = fpToHex(&proofBn254.Krs.X)
	proofPoints[7] = fpToHex(&proofBn254.Krs.Y)

	// Extract public inputs in the order gnark expects (matches circuit struct field order)
	// The gnark verifier's publicInputMSM processes them in struct field declaration order.
	pubInputs := []string{
		w.PublicInputs.CircuitHash[0],
		w.PublicInputs.CircuitHash[1],
		w.PublicInputs.PublicValues[0],
		w.PublicInputs.PublicValues[1],
		w.PublicInputs.OutputChallenge,
		w.PublicInputs.ClaimAggCoeff,
		w.PublicInputs.Layer0.Bindings[0],
		w.PublicInputs.Layer0.Rhos[0],
		w.PublicInputs.Layer0.Rhos[1],
		w.PublicInputs.Layer0.Gammas[0],
		w.PublicInputs.Layer0.PODPChallenge,
		w.PublicInputs.Layer1.Bindings[0],
		w.PublicInputs.Layer1.Rhos[0],
		w.PublicInputs.Layer1.Rhos[1],
		w.PublicInputs.Layer1.Gammas[0],
		w.PublicInputs.Layer1.PODPChallenge,
		w.PublicInputs.Layer1.PopChallenge,
		w.PublicInputs.InputRLCCoeffs[0],
		w.PublicInputs.InputRLCCoeffs[1],
		w.PublicInputs.InputPODPChallenge,
		w.PublicInputs.InterLayerCoeff,
		w.PublicOutputs.RlcBeta0,
		w.PublicOutputs.RlcBeta1,
		w.PublicOutputs.ZDotJStar0,
		w.PublicOutputs.ZDotJStar1,
		w.PublicOutputs.LTensor0,
		w.PublicOutputs.LTensor1,
		w.PublicOutputs.ZDotR,
		w.PublicOutputs.MLEEval,
	}

	fixture := map[string]interface{}{
		"proof":         proofPoints,
		"public_inputs": pubInputs,
	}

	out, _ := json.MarshalIndent(fixture, "", "  ")
	fmt.Println(string(out))
	fmt.Fprintf(os.Stderr, "JSON fixture written to stdout (%d proof points, %d public inputs)\n", len(proofPoints), len(pubInputs))
}

// fpToHex converts a bn254 base field element to a 0x-prefixed hex string
func fpToHex(e *bn254fp.Element) string {
	var b big.Int
	e.BigInt(&b)
	return fmt.Sprintf("0x%064x", &b)
}

// Ensure bn254 import is used
var _ = bn254.ID

func parseVarSlice(strs []string) []frontend.Variable {
	vars := make([]frontend.Variable, len(strs))
	for i, s := range strs {
		vars[i] = parseBigInt(s)
	}
	return vars
}

// cmdExportSolidity exports the Groth16 verification key as a Solidity contract.
func cmdExportSolidity() {
	// Load verification key
	vkFile, err := os.Open("verification_key.bin")
	if err != nil {
		fmt.Fprintf(os.Stderr, "error opening vk: %v\n (run 'setup' first)\n", err)
		os.Exit(1)
	}
	defer vkFile.Close()

	vk := groth16.NewVerifyingKey(ecc.BN254)
	_, err = vk.ReadFrom(vkFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error reading vk: %v\n", err)
		os.Exit(1)
	}

	// Export Solidity
	outFile, err := os.Create("RemainderGroth16Verifier.sol")
	if err != nil {
		fmt.Fprintf(os.Stderr, "error creating output: %v\n", err)
		os.Exit(1)
	}
	defer outFile.Close()

	err = vk.ExportSolidity(outFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error exporting solidity: %v\n", err)
		os.Exit(1)
	}

	fmt.Fprintln(os.Stderr, "Solidity verifier exported to RemainderGroth16Verifier.sol")
}

// cmdInfo shows circuit statistics.
func cmdInfo() {
	fmt.Fprintln(os.Stderr, "Compiling circuit...")
	circuit := newCircuit()
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("Constraints: %d\n", ccs.GetNbConstraints())
	fmt.Printf("Public inputs: ~25 (circuitHash[2], pubInputs[2], challenges, bindings, rhos, gammas, etc.)\n")
	fmt.Printf("Private witness: PODP z-vectors, z-delta, z-beta, PoP z1-z5\n")
}
