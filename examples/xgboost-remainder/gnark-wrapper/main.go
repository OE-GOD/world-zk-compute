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
	fmt.Println("Usage: gnark-wrapper <command> [--config small|medium]")
	fmt.Println()
	fmt.Println("Commands:")
	fmt.Println("  setup           Generate proving key and verification key")
	fmt.Println("  prove           Generate a Groth16 proof from witness JSON (stdin)")
	fmt.Println("  prove-json      Like prove, but output JSON fixture for Solidity tests")
	fmt.Println("  export-solidity Export Solidity verifier contract")
	fmt.Println("  info            Show circuit info (constraints, etc.)")
	fmt.Println()
	fmt.Println("Options:")
	fmt.Println("  --config small   Use small config (num_vars=1, default)")
	fmt.Println("  --config medium  Use medium config (num_vars=4)")
}

// parseConfig reads --config from os.Args and returns the matching CircuitConfig.
func parseConfig() CircuitConfig {
	for i, arg := range os.Args {
		if arg == "--config" && i+1 < len(os.Args) {
			switch os.Args[i+1] {
			case "medium":
				return MediumConfig()
			case "small":
				return SmallConfig()
			default:
				fmt.Fprintf(os.Stderr, "unknown config: %s (use 'small' or 'medium')\n", os.Args[i+1])
				os.Exit(1)
			}
		}
	}
	return SmallConfig() // default
}

// WitnessJSON is the JSON format for the witness data from the Rust generator.
type WitnessJSON struct {
	Config struct {
		NumVars int `json:"num_vars"`
	} `json:"config"`
	PublicInputs struct {
		CircuitHash      [2]string `json:"circuit_hash"`
		PublicValues     []string  `json:"public_values"`
		OutputChallenges []string  `json:"output_challenges"`
		ClaimAggCoeff    string    `json:"claim_agg_coeff"`
		InterLayerCoeff  string    `json:"inter_layer_coeff"`
		Layers           []struct {
			Bindings      []string `json:"bindings"`
			Rhos          []string `json:"rhos"`
			Gammas        []string `json:"gammas"`
			PODPChallenge string   `json:"podp_challenge"`
			PopChallenge  string   `json:"pop_challenge,omitempty"`
		} `json:"layers"`
		InputRLCCoeffs     [2]string `json:"input_rlc_coeffs"`
		InputPODPChallenge string    `json:"input_podp_challenge"`
	} `json:"public_inputs"`
	PublicOutputs struct {
		RlcBetas   []string  `json:"rlc_betas"`
		ZDotJStars []string  `json:"z_dot_jstars"`
		LTensor    [2]string `json:"l_tensor"`
		ZDotR      string    `json:"z_dot_r"`
		MLEEval    string    `json:"mle_eval"`
	} `json:"public_outputs"`
	Witness struct {
		LayerPODPs []struct {
			ZVector []string `json:"z_vector"`
			ZDelta  string   `json:"z_delta"`
			ZBeta   string   `json:"z_beta"`
		} `json:"layer_podps"`
		LayerPops []struct {
			Z1 string `json:"z1"`
			Z2 string `json:"z2"`
			Z3 string `json:"z3"`
			Z4 string `json:"z4"`
			Z5 string `json:"z5"`
		} `json:"layer_pops"`
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

func parseVarSlice(strs []string) []frontend.Variable {
	vars := make([]frontend.Variable, len(strs))
	for i, s := range strs {
		vars[i] = parseBigInt(s)
	}
	return vars
}

// buildAssignment constructs a circuit assignment from the parsed witness JSON.
func buildAssignment(config CircuitConfig, w *WitnessJSON) *RemainderWrapperCircuit {
	nv := config.NumVars

	assignment := AllocateCircuit(config)

	assignment.CircuitHash = [2]frontend.Variable{
		parseBigInt(w.PublicInputs.CircuitHash[0]),
		parseBigInt(w.PublicInputs.CircuitHash[1]),
	}
	for i := 0; i < config.NumPublicInputs; i++ {
		assignment.PublicInputs[i] = parseBigInt(w.PublicInputs.PublicValues[i])
	}
	for i := 0; i < nv; i++ {
		assignment.OutputChallenges[i] = parseBigInt(w.PublicInputs.OutputChallenges[i])
	}
	assignment.ClaimAggCoeff = parseBigInt(w.PublicInputs.ClaimAggCoeff)
	assignment.InterLayerCoeff = parseBigInt(w.PublicInputs.InterLayerCoeff)

	// Layer 0
	l0 := w.PublicInputs.Layers[0]
	for i := 0; i < nv; i++ {
		assignment.Layer0Bindings[i] = parseBigInt(l0.Bindings[i])
		assignment.Layer0Gammas[i] = parseBigInt(l0.Gammas[i])
	}
	for i := 0; i <= nv; i++ {
		assignment.Layer0Rhos[i] = parseBigInt(l0.Rhos[i])
	}
	assignment.Layer0PODPChallenge = parseBigInt(l0.PODPChallenge)

	// Layer 1
	l1 := w.PublicInputs.Layers[1]
	for i := 0; i < nv; i++ {
		assignment.Layer1Bindings[i] = parseBigInt(l1.Bindings[i])
		assignment.Layer1Gammas[i] = parseBigInt(l1.Gammas[i])
	}
	for i := 0; i <= nv; i++ {
		assignment.Layer1Rhos[i] = parseBigInt(l1.Rhos[i])
	}
	assignment.Layer1PODPChallenge = parseBigInt(l1.PODPChallenge)
	assignment.Layer1PopChallenge = parseBigInt(l1.PopChallenge)

	assignment.InputRLCCoeffs = [2]frontend.Variable{
		parseBigInt(w.PublicInputs.InputRLCCoeffs[0]),
		parseBigInt(w.PublicInputs.InputRLCCoeffs[1]),
	}
	assignment.InputPODPChallenge = parseBigInt(w.PublicInputs.InputPODPChallenge)

	// Public outputs
	assignment.RlcBeta0 = parseBigInt(w.PublicOutputs.RlcBetas[0])
	assignment.RlcBeta1 = parseBigInt(w.PublicOutputs.RlcBetas[1])
	assignment.ZDotJStar0 = parseBigInt(w.PublicOutputs.ZDotJStars[0])
	assignment.ZDotJStar1 = parseBigInt(w.PublicOutputs.ZDotJStars[1])
	assignment.LTensor = [2]frontend.Variable{
		parseBigInt(w.PublicOutputs.LTensor[0]),
		parseBigInt(w.PublicOutputs.LTensor[1]),
	}
	assignment.ZDotR = parseBigInt(w.PublicOutputs.ZDotR)
	assignment.MLEEval = parseBigInt(w.PublicOutputs.MLEEval)

	// Private witnesses
	assignment.Layer0PODP = PODPWitness{
		ZVector: parseVarSlice(w.Witness.LayerPODPs[0].ZVector),
		ZDelta:  parseBigInt(w.Witness.LayerPODPs[0].ZDelta),
		ZBeta:   parseBigInt(w.Witness.LayerPODPs[0].ZBeta),
	}
	assignment.Layer1PODP = PODPWitness{
		ZVector: parseVarSlice(w.Witness.LayerPODPs[1].ZVector),
		ZDelta:  parseBigInt(w.Witness.LayerPODPs[1].ZDelta),
		ZBeta:   parseBigInt(w.Witness.LayerPODPs[1].ZBeta),
	}
	if len(w.Witness.LayerPops) > 0 {
		assignment.Layer1Pop = PopWitness{
			Z1: parseBigInt(w.Witness.LayerPops[0].Z1),
			Z2: parseBigInt(w.Witness.LayerPops[0].Z2),
			Z3: parseBigInt(w.Witness.LayerPops[0].Z3),
			Z4: parseBigInt(w.Witness.LayerPops[0].Z4),
			Z5: parseBigInt(w.Witness.LayerPops[0].Z5),
		}
	}
	assignment.InputPODP = PODPWitness{
		ZVector: parseVarSlice(w.Witness.InputPODP.ZVector),
		ZDelta:  parseBigInt(w.Witness.InputPODP.ZDelta),
		ZBeta:   parseBigInt(w.Witness.InputPODP.ZBeta),
	}

	return assignment
}

// cmdSetup compiles the circuit and runs Groth16 trusted setup.
func cmdSetup() {
	config := parseConfig()
	fmt.Fprintf(os.Stderr, "Compiling circuit (num_vars=%d)...\n", config.NumVars)
	circuit := AllocateCircuit(config)
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

// cmdProve generates a Groth16 proof from witness JSON on stdin.
func cmdProve() {
	config := parseConfig()

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

	assignment := buildAssignment(config, &w)

	// Compile circuit
	fmt.Fprintf(os.Stderr, "Compiling circuit (num_vars=%d)...\n", config.NumVars)
	circuit := AllocateCircuit(config)
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
func cmdProveJSON() {
	config := parseConfig()

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

	assignment := buildAssignment(config, &w)

	// Compile circuit
	fmt.Fprintf(os.Stderr, "Compiling circuit (num_vars=%d)...\n", config.NumVars)
	circuit := AllocateCircuit(config)
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
	proofPoints[2] = fpToHex(&proofBn254.Bs.X.A1)
	proofPoints[3] = fpToHex(&proofBn254.Bs.X.A0)
	proofPoints[4] = fpToHex(&proofBn254.Bs.Y.A1)
	proofPoints[5] = fpToHex(&proofBn254.Bs.Y.A0)
	proofPoints[6] = fpToHex(&proofBn254.Krs.X)
	proofPoints[7] = fpToHex(&proofBn254.Krs.Y)

	// Build public inputs in struct field declaration order.
	nv := config.NumVars
	var pubInputs []string

	// CircuitHash [2]
	pubInputs = append(pubInputs, w.PublicInputs.CircuitHash[0], w.PublicInputs.CircuitHash[1])
	// PublicInputs [2^nv]
	pubInputs = append(pubInputs, w.PublicInputs.PublicValues...)
	// OutputChallenges [nv]
	pubInputs = append(pubInputs, w.PublicInputs.OutputChallenges...)
	// ClaimAggCoeff
	pubInputs = append(pubInputs, w.PublicInputs.ClaimAggCoeff)

	// Layer 0: bindings[nv], rhos[nv+1], gammas[nv], podp_challenge
	l0 := w.PublicInputs.Layers[0]
	pubInputs = append(pubInputs, l0.Bindings[:nv]...)
	pubInputs = append(pubInputs, l0.Rhos[:nv+1]...)
	pubInputs = append(pubInputs, l0.Gammas[:nv]...)
	pubInputs = append(pubInputs, l0.PODPChallenge)

	// Layer 1: bindings[nv], rhos[nv+1], gammas[nv], podp_challenge, pop_challenge
	l1 := w.PublicInputs.Layers[1]
	pubInputs = append(pubInputs, l1.Bindings[:nv]...)
	pubInputs = append(pubInputs, l1.Rhos[:nv+1]...)
	pubInputs = append(pubInputs, l1.Gammas[:nv]...)
	pubInputs = append(pubInputs, l1.PODPChallenge)
	pubInputs = append(pubInputs, l1.PopChallenge)

	// InputRLCCoeffs [2], InputPODPChallenge, InterLayerCoeff
	pubInputs = append(pubInputs, w.PublicInputs.InputRLCCoeffs[0], w.PublicInputs.InputRLCCoeffs[1])
	pubInputs = append(pubInputs, w.PublicInputs.InputPODPChallenge)
	pubInputs = append(pubInputs, w.PublicInputs.InterLayerCoeff)

	// Public outputs: RlcBeta0, RlcBeta1, ZDotJStar0, ZDotJStar1, LTensor[2], ZDotR, MLEEval
	pubInputs = append(pubInputs, w.PublicOutputs.RlcBetas[0], w.PublicOutputs.RlcBetas[1])
	pubInputs = append(pubInputs, w.PublicOutputs.ZDotJStars[0], w.PublicOutputs.ZDotJStars[1])
	pubInputs = append(pubInputs, w.PublicOutputs.LTensor[0], w.PublicOutputs.LTensor[1])
	pubInputs = append(pubInputs, w.PublicOutputs.ZDotR)
	pubInputs = append(pubInputs, w.PublicOutputs.MLEEval)

	fixture := map[string]interface{}{
		"proof":         proofPoints,
		"public_inputs": pubInputs,
	}

	out, _ := json.MarshalIndent(fixture, "", "  ")
	fmt.Println(string(out))
	fmt.Fprintf(os.Stderr, "JSON fixture written to stdout (%d proof points, %d public inputs)\n", len(proofPoints), len(pubInputs))
	_ = nv
}

// fpToHex converts a bn254 base field element to a 0x-prefixed hex string
func fpToHex(e *bn254fp.Element) string {
	var b big.Int
	e.BigInt(&b)
	return fmt.Sprintf("0x%064x", &b)
}

// Ensure bn254 import is used
var _ = bn254.ID

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
	config := parseConfig()
	fmt.Fprintf(os.Stderr, "Compiling circuit (num_vars=%d)...\n", config.NumVars)
	circuit := AllocateCircuit(config)
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}
	expectedPubInputs := 7*config.NumVars + config.NumPublicInputs + 20
	fmt.Printf("Config: num_vars=%d, num_public_inputs=%d\n", config.NumVars, config.NumPublicInputs)
	fmt.Printf("Constraints: %d\n", ccs.GetNbConstraints())
	fmt.Printf("Expected public inputs: %d\n", expectedPubInputs)
}
