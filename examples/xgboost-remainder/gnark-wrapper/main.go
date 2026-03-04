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
	fmt.Println("Usage: gnark-wrapper <command> [--config small|medium] [--config-json]")
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
	fmt.Println("  --config-json    Derive config from witness JSON (per-layer num_vars)")
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

// configFromWitness builds a CircuitConfig from the witness JSON config section.
// If LayerNumVars is present, builds a per-layer config; otherwise falls back to
// uniform NumVars for backward compatibility.
func configFromWitness(w *WitnessJSON) CircuitConfig {
	if len(w.Config.LayerNumVars) > 0 {
		numLayers := w.Config.NumLayers
		if numLayers == 0 {
			numLayers = len(w.Config.LayerNumVars)
		}
		// Infer layer degrees from witness data (z_vector_len / num_vars_i = degree+1)
		layerDegrees := make([]int, numLayers)
		for i := 0; i < numLayers; i++ {
			nv := w.Config.LayerNumVars[i]
			zLen := len(w.Witness.LayerPODPs[i].ZVector)
			if nv > 0 {
				layerDegrees[i] = zLen/nv - 1
			} else {
				layerDegrees[i] = 2 // default for zero-var layers
			}
		}
		outputNumVars := w.Config.OutputNumVars
		if outputNumVars == 0 {
			outputNumVars = len(w.PublicInputs.OutputChallenges)
		}
		pubInputCount := w.Config.PubInputCount
		if pubInputCount == 0 {
			pubInputCount = len(w.PublicInputs.PublicValues)
		}
		return CircuitConfig{
			NumLayers:       numLayers,
			LayerNumVars:    w.Config.LayerNumVars[:numLayers],
			LayerDegrees:    layerDegrees,
			OutputNumVars:   outputNumVars,
			PubInputCount:   pubInputCount,
			MleEvalLayerIdx: w.Config.MleEvalLayerIdx,
		}
	}

	// Backward compat: uniform num_vars
	nv := w.Config.NumVars
	numLayers := w.Config.NumLayers
	if numLayers == 0 {
		numLayers = 2
	}
	// Infer layer degrees from witness data
	layerDegrees := make([]int, numLayers)
	layerNumVars := make([]int, numLayers)
	for i := 0; i < numLayers; i++ {
		layerNumVars[i] = nv
		zLen := len(w.Witness.LayerPODPs[i].ZVector)
		if nv > 0 {
			layerDegrees[i] = zLen/nv - 1
		} else {
			layerDegrees[i] = 2
		}
	}
	return CircuitConfig{
		NumLayers:     numLayers,
		LayerNumVars:  layerNumVars,
		LayerDegrees:  layerDegrees,
		OutputNumVars: nv,
		PubInputCount: 1 << nv,
	}
}

// hasConfigJSON returns true if --config-json flag is present.
func hasConfigJSON() bool {
	for _, arg := range os.Args {
		if arg == "--config-json" {
			return true
		}
	}
	return false
}

// WitnessJSON is the JSON format for the witness data from the Rust generator.
type WitnessJSON struct {
	Config struct {
		NumVars         int   `json:"num_vars"`
		NumLayers       int   `json:"num_layers,omitempty"`
		LayerNumVars    []int `json:"layer_num_vars,omitempty"`    // per-layer num_vars
		OutputNumVars   int   `json:"output_num_vars,omitempty"`   // output challenge count
		PubInputCount   int   `json:"pub_input_count,omitempty"`   // public input count
		MleEvalLayerIdx int   `json:"mle_eval_layer_idx,omitempty"` // MLE eval binding layer
	} `json:"config"`
	PublicInputs struct {
		CircuitHash      [2]string `json:"circuit_hash"`
		PublicValues     []string  `json:"public_values"`
		OutputChallenges []string  `json:"output_challenges"`
		ClaimAggCoeff    string    `json:"claim_agg_coeff"`
		InterLayerCoeff  string    `json:"inter_layer_coeff,omitempty"`  // old: single (backward compat)
		InterLayerCoeffs []string  `json:"inter_layer_coeffs,omitempty"` // new: array
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
		RlcBetas   []string `json:"rlc_betas"`
		ZDotJStars []string `json:"z_dot_jstars"`
		LTensor    []string `json:"l_tensor"`
		ZDotR      string   `json:"z_dot_r"`
		MLEEval    string   `json:"mle_eval"`
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

// getInterLayerCoeffs returns inter-layer coefficients from the witness JSON,
// supporting both the old single-value format and the new array format.
func getInterLayerCoeffs(w *WitnessJSON, numLayers int) []string {
	if len(w.PublicInputs.InterLayerCoeffs) > 0 {
		return w.PublicInputs.InterLayerCoeffs
	}
	// Backward compat: single inter_layer_coeff for 2-layer case
	if w.PublicInputs.InterLayerCoeff != "" && numLayers == 2 {
		return []string{w.PublicInputs.InterLayerCoeff}
	}
	return nil
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
	nLayers := config.NumLayers

	assignment := AllocateCircuit(config)

	assignment.CircuitHash = [2]frontend.Variable{
		parseBigInt(w.PublicInputs.CircuitHash[0]),
		parseBigInt(w.PublicInputs.CircuitHash[1]),
	}
	for i := 0; i < config.PubInputCount; i++ {
		assignment.PublicInputs[i] = parseBigInt(w.PublicInputs.PublicValues[i])
	}
	for i := 0; i < config.OutputNumVars; i++ {
		assignment.OutputChallenges[i] = parseBigInt(w.PublicInputs.OutputChallenges[i])
	}
	assignment.ClaimAggCoeff = parseBigInt(w.PublicInputs.ClaimAggCoeff)

	// Per-layer public inputs (each layer has its own num_vars)
	for li := 0; li < nLayers; li++ {
		nv := config.LayerNumVars[li]
		layer := w.PublicInputs.Layers[li]
		for i := 0; i < nv; i++ {
			assignment.Layers[li].Bindings[i] = parseBigInt(layer.Bindings[i])
			assignment.Layers[li].Gammas[i] = parseBigInt(layer.Gammas[i])
		}
		for i := 0; i <= nv; i++ {
			assignment.Layers[li].Rhos[i] = parseBigInt(layer.Rhos[i])
		}
		assignment.Layers[li].PODPChallenge = parseBigInt(layer.PODPChallenge)
		if config.HasPoP(li) && layer.PopChallenge != "" {
			assignment.Layers[li].PopChallenge[0] = parseBigInt(layer.PopChallenge)
		}
	}

	assignment.InputRLCCoeffs = [2]frontend.Variable{
		parseBigInt(w.PublicInputs.InputRLCCoeffs[0]),
		parseBigInt(w.PublicInputs.InputRLCCoeffs[1]),
	}
	assignment.InputPODPChallenge = parseBigInt(w.PublicInputs.InputPODPChallenge)

	// Inter-layer coefficients
	interLayerCoeffs := getInterLayerCoeffs(w, nLayers)
	for i := 0; i < len(assignment.InterLayerCoeffs); i++ {
		assignment.InterLayerCoeffs[i] = parseBigInt(interLayerCoeffs[i])
	}

	// Public outputs
	for i := 0; i < nLayers; i++ {
		assignment.RlcBeta[i] = parseBigInt(w.PublicOutputs.RlcBetas[i])
		assignment.ZDotJStar[i] = parseBigInt(w.PublicOutputs.ZDotJStars[i])
	}
	for i := 0; i < config.NumLTensorElems(); i++ {
		assignment.LTensor[i] = parseBigInt(w.PublicOutputs.LTensor[i])
	}
	assignment.ZDotR = parseBigInt(w.PublicOutputs.ZDotR)
	assignment.MLEEval = parseBigInt(w.PublicOutputs.MLEEval)

	// Private witnesses
	zero := big.NewInt(0)
	for i := 0; i < nLayers; i++ {
		assignment.LayerPODPs[i] = PODPWitness{
			ZVector: parseVarSlice(w.Witness.LayerPODPs[i].ZVector),
			ZDelta:  parseBigInt(w.Witness.LayerPODPs[i].ZDelta),
			ZBeta:   parseBigInt(w.Witness.LayerPODPs[i].ZBeta),
		}
		// Always set PopWitness (gnark requires all private fields to have values)
		if config.HasPoP(i) && i < len(w.Witness.LayerPops) {
			assignment.LayerPops[i] = PopWitness{
				Z1: parseBigInt(w.Witness.LayerPops[i].Z1),
				Z2: parseBigInt(w.Witness.LayerPops[i].Z2),
				Z3: parseBigInt(w.Witness.LayerPops[i].Z3),
				Z4: parseBigInt(w.Witness.LayerPops[i].Z4),
				Z5: parseBigInt(w.Witness.LayerPops[i].Z5),
			}
		} else {
			assignment.LayerPops[i] = PopWitness{Z1: zero, Z2: zero, Z3: zero, Z4: zero, Z5: zero}
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
	fmt.Fprintf(os.Stderr, "Compiling circuit (layer_num_vars=%v)...\n", config.LayerNumVars)
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

	var config CircuitConfig
	if hasConfigJSON() {
		config = configFromWitness(&w)
	} else {
		config = parseConfig()
	}

	assignment := buildAssignment(config, &w)

	// Compile circuit
	fmt.Fprintf(os.Stderr, "Compiling circuit (layer_num_vars=%v)...\n", config.LayerNumVars)
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

	var config CircuitConfig
	if hasConfigJSON() {
		config = configFromWitness(&w)
	} else {
		config = parseConfig()
	}

	assignment := buildAssignment(config, &w)

	// Compile circuit
	fmt.Fprintf(os.Stderr, "Compiling circuit (layer_num_vars=%v)...\n", config.LayerNumVars)
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

	// Build public inputs in struct field declaration order (matches Solidity buildGroth16Inputs).
	nLayers := config.NumLayers
	var pubInputs []string

	// CircuitHash [2]
	pubInputs = append(pubInputs, w.PublicInputs.CircuitHash[0], w.PublicInputs.CircuitHash[1])
	// PublicInputs [2^nv]
	pubInputs = append(pubInputs, w.PublicInputs.PublicValues...)
	// OutputChallenges [nv]
	pubInputs = append(pubInputs, w.PublicInputs.OutputChallenges...)
	// ClaimAggCoeff
	pubInputs = append(pubInputs, w.PublicInputs.ClaimAggCoeff)

	// Per-layer: bindings[nv], rhos[nv+1], gammas[nv], podp_challenge, pop_challenge?
	for i, layer := range w.PublicInputs.Layers {
		pubInputs = append(pubInputs, layer.Bindings...)
		pubInputs = append(pubInputs, layer.Rhos...)
		pubInputs = append(pubInputs, layer.Gammas...)
		pubInputs = append(pubInputs, layer.PODPChallenge)
		if config.HasPoP(i) && layer.PopChallenge != "" {
			pubInputs = append(pubInputs, layer.PopChallenge)
		}
	}

	// InputRLCCoeffs [2], InputPODPChallenge
	pubInputs = append(pubInputs, w.PublicInputs.InputRLCCoeffs[0], w.PublicInputs.InputRLCCoeffs[1])
	pubInputs = append(pubInputs, w.PublicInputs.InputPODPChallenge)

	// InterLayerCoeffs [L-1]
	interLayerCoeffs := getInterLayerCoeffs(&w, nLayers)
	pubInputs = append(pubInputs, interLayerCoeffs...)

	// Public outputs: RlcBeta[L], ZDotJStar[L], LTensor[variable], ZDotR, MLEEval
	pubInputs = append(pubInputs, w.PublicOutputs.RlcBetas...)
	pubInputs = append(pubInputs, w.PublicOutputs.ZDotJStars...)
	pubInputs = append(pubInputs, w.PublicOutputs.LTensor...)
	pubInputs = append(pubInputs, w.PublicOutputs.ZDotR)
	pubInputs = append(pubInputs, w.PublicOutputs.MLEEval)

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
	fmt.Fprintf(os.Stderr, "Compiling circuit (layer_num_vars=%v, num_layers=%d)...\n", config.LayerNumVars, config.NumLayers)
	circuit := AllocateCircuit(config)
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}
	expectedPubInputs := config.ExpectedPublicInputCount()
	fmt.Printf("Config: layer_num_vars=%v, num_layers=%d, layer_degrees=%v\n", config.LayerNumVars, config.NumLayers, config.LayerDegrees)
	fmt.Printf("Constraints: %d\n", ccs.GetNbConstraints())
	fmt.Printf("Expected public inputs: %d\n", expectedPubInputs)
}
