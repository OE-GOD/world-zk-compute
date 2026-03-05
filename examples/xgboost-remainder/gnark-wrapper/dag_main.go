package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"math/big"
	"os"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/backend/groth16"
	groth16bn254 "github.com/consensys/gnark/backend/groth16/bn254"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"
)

// DAGWitnessJSON is the JSON format for the DAG witness data from gen_dag_groth16_witness.rs.
type DAGWitnessJSON struct {
	Config struct {
		NumComputeLayers int `json:"num_compute_layers"`
		NumInputLayers   int `json:"num_input_layers"`
		OutputNumVars    int `json:"output_num_vars"`
		PubInputCount    int `json:"pub_input_count"`
		NumInputGroups   int `json:"num_input_groups"`
		NumPublicClaims  int `json:"num_public_claims"`
	} `json:"config"`
	PublicInputs struct {
		CircuitHash      [2]string `json:"circuit_hash"`
		OutputChallenges []string  `json:"output_challenges"`
		Layers           []struct {
			RlcCoeffs        []string `json:"rlc_coeffs"`
			Bindings         []string `json:"bindings"`
			Rhos             []string `json:"rhos"`
			Gammas           []string `json:"gammas"`
			PODPChallenge    string   `json:"podp_challenge"`
			PopChallenges    []string `json:"pop_challenges"`
			NumPopChallenges int      `json:"num_pop_challenges"`
		} `json:"layers"`
		InputGroups []struct {
			RlcCoeffs     []string `json:"rlc_coeffs"`
			PODPChallenge string   `json:"podp_challenge"`
			ZVector       []string `json:"z_vector"`
			ZDelta        string   `json:"z_delta"`
			ZBeta         string   `json:"z_beta"`
			LHalfBindings []string `json:"l_half_bindings"`
			RHalfBindings []string `json:"r_half_bindings"`
			NumRows       int      `json:"num_rows"`
		} `json:"input_groups"`
	} `json:"public_inputs"`
	PublicOutputs struct {
		RlcBetas      []string `json:"rlc_betas"`
		ZDotJStars    []string `json:"z_dot_jstars"`
		LTensorFlat   []string `json:"l_tensor_flat"`
		LTensorOffsets []int   `json:"l_tensor_offsets"`
		ZDotRs        []string `json:"z_dot_rs"`
		MleEvals      []string `json:"mle_evals"`
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
	} `json:"witness"`
	DAGCircuitDescription struct {
		NumComputeLayers  int    `json:"numComputeLayers"`
		NumInputLayers    int    `json:"numInputLayers"`
		LayerTypes        []int  `json:"layerTypes"`
		NumSumcheckRounds []int  `json:"numSumcheckRounds"`
		AtomOffsets       []int  `json:"atomOffsets"`
		AtomTargetLayers  []int  `json:"atomTargetLayers"`
		PtOffsets         []int  `json:"ptOffsets"`
		PtData            []int  `json:"ptData"`
		InputIsCommitted  []bool `json:"inputIsCommitted"`
		IncomingOffsets    []int  `json:"incomingOffsets"`
		IncomingAtomIdx    []int  `json:"incomingAtomIdx"`
	} `json:"dag_circuit_description"`
	DAGLayers []struct {
		NumRounds      int `json:"num_rounds"`
		Degree         int `json:"degree"`
		NumClaims      int `json:"num_claims"`
		NumCommitments int `json:"num_commitments"`
		NumPops        int `json:"num_pops"`
		NumAtoms       int `json:"num_atoms"`
	} `json:"dag_layers"`
	// On-chain data
	InnerProofHex   string `json:"inner_proof_hex"`
	GensHex         string `json:"gens_hex"`
	CircuitHashRaw  string `json:"circuit_hash_raw"`
	PublicValuesABI string `json:"public_values_abi"`
}

// dagConfigFromWitness builds a DAGCircuitConfig from the parsed witness JSON.
func dagConfigFromWitness(w *DAGWitnessJSON) DAGCircuitConfig {
	nLayers := w.Config.NumComputeLayers
	desc := w.DAGCircuitDescription

	layerNumVars := make([]int, nLayers)
	layerDegrees := make([]int, nLayers)
	layerNumClaims := make([]int, nLayers)
	for i := 0; i < nLayers; i++ {
		layerNumVars[i] = w.DAGLayers[i].NumRounds
		layerDegrees[i] = w.DAGLayers[i].Degree
		layerNumClaims[i] = w.DAGLayers[i].NumClaims
	}

	nGroups := w.Config.NumInputGroups
	inputGroupNumClaims := make([]int, nGroups)
	inputGroupLHalfLen := make([]int, nGroups)
	inputGroupRHalfLen := make([]int, nGroups)
	for g := 0; g < nGroups; g++ {
		inputGroupNumClaims[g] = len(w.PublicInputs.InputGroups[g].RlcCoeffs)
		inputGroupLHalfLen[g] = len(w.PublicInputs.InputGroups[g].LHalfBindings)
		inputGroupRHalfLen[g] = len(w.PublicInputs.InputGroups[g].RHalfBindings)
	}

	// Build point templates from DAG description
	pointTemplates := make([][][]int, nLayers)
	pointTemplateSources := make([][]int, nLayers)

	outputNumVars := w.Config.OutputNumVars
	for i := 0; i < nLayers; i++ {
		nc := layerNumClaims[i]
		pointTemplates[i] = make([][]int, nc)
		pointTemplateSources[i] = make([]int, nc)

		// Get incoming atoms for this layer
		inStart := desc.IncomingOffsets[i]
		inEnd := desc.IncomingOffsets[i+1]
		atomClaims := inEnd - inStart
		outputClaims := nc - atomClaims

		// Output claims first (from virtual output layer, source = -1)
		for k := 0; k < outputClaims; k++ {
			pt := make([]int, outputNumVars)
			for v := 0; v < outputNumVars; v++ {
				pt[v] = v // B0, B1, ..., B(V-1) referencing OutputChallenges
			}
			pointTemplates[i][k] = pt
			pointTemplateSources[i][k] = -1
		}

		// Compute layer atom claims
		for k := 0; k < atomClaims; k++ {
			globalAtomIdx := desc.IncomingAtomIdx[inStart+k]

			// Find source layer
			sourceLayer := 0
			for li := 0; li < nLayers; li++ {
				atomStart := desc.AtomOffsets[li]
				atomEnd := desc.AtomOffsets[li+1]
				if globalAtomIdx >= atomStart && globalAtomIdx < atomEnd {
					sourceLayer = li
					break
				}
			}

			// Get point template
			ptStart := desc.PtOffsets[globalAtomIdx]
			ptEnd := desc.PtOffsets[globalAtomIdx+1]
			pt := make([]int, ptEnd-ptStart)
			for j := 0; j < len(pt); j++ {
				pt[j] = desc.PtData[ptStart+j]
			}

			pointTemplates[i][outputClaims+k] = pt
			pointTemplateSources[i][outputClaims+k] = sourceLayer
		}
	}

	nPubClaims := w.Config.NumPublicClaims
	pubClaimNumVars := make([]int, nPubClaims)
	// Derive pub claim point lengths from atom routing data
	pubIdx := 0
	numInputLayers := len(desc.InputIsCommitted)
	for j := 0; j < numInputLayers; j++ {
		if desc.InputIsCommitted[j] {
			continue
		}
		targetIdx := nLayers + j
		if targetIdx >= len(desc.IncomingOffsets)-1 {
			continue
		}
		inStart := desc.IncomingOffsets[targetIdx]
		inEnd := desc.IncomingOffsets[targetIdx+1]
		for k := 0; k < inEnd-inStart && pubIdx < nPubClaims; k++ {
			globalAtom := desc.IncomingAtomIdx[inStart+k]
			ptStart := desc.PtOffsets[globalAtom]
			ptEnd := desc.PtOffsets[globalAtom+1]
			pubClaimNumVars[pubIdx] = ptEnd - ptStart
			pubIdx++
		}
	}
	// Fallback: if no routing data, use log2(PubInputCount)
	if pubIdx == 0 && nPubClaims > 0 {
		pubBits := 0
		n := w.Config.PubInputCount
		for n > 1 {
			n >>= 1
			pubBits++
		}
		for p := 0; p < nPubClaims; p++ {
			pubClaimNumVars[p] = pubBits
		}
	}

	return DAGCircuitConfig{
		NumComputeLayers:    nLayers,
		LayerNumVars:        layerNumVars,
		LayerDegrees:        layerDegrees,
		LayerNumClaims:      layerNumClaims,
		OutputNumVars:       w.Config.OutputNumVars,
		NumInputGroups:      nGroups,
		InputGroupNumClaims: inputGroupNumClaims,
		InputGroupLHalfLen:  inputGroupLHalfLen,
		InputGroupRHalfLen:  inputGroupRHalfLen,
		NumPublicClaims:     nPubClaims,
		PubInputCount:       w.Config.PubInputCount,
		PubClaimNumVars:     pubClaimNumVars,
		PointTemplates:      pointTemplates,
		PointTemplateSources: pointTemplateSources,
	}
}

// buildDAGAssignment constructs a DAG circuit assignment from parsed witness JSON.
func buildDAGAssignment(config DAGCircuitConfig, w *DAGWitnessJSON) *DAGWrapperCircuit {
	nLayers := config.NumComputeLayers

	assignment := AllocateDAGCircuit(config)

	assignment.CircuitHash = [2]frontend.Variable{
		parseBigInt(w.PublicInputs.CircuitHash[0]),
		parseBigInt(w.PublicInputs.CircuitHash[1]),
	}
	for i := 0; i < config.OutputNumVars; i++ {
		assignment.OutputChallenges[i] = parseBigInt(w.PublicInputs.OutputChallenges[i])
	}

	// Per-layer
	zero := big.NewInt(0)
	for li := 0; li < nLayers; li++ {
		layer := w.PublicInputs.Layers[li]
		nc := config.LayerNumClaims[li]
		for i := 0; i < nc; i++ {
			assignment.Layers[li].RlcCoeffs[i] = parseBigInt(layer.RlcCoeffs[i])
		}
		nv := config.LayerNumVars[li]
		for i := 0; i < nv; i++ {
			assignment.Layers[li].Bindings[i] = parseBigInt(layer.Bindings[i])
			assignment.Layers[li].Gammas[i] = parseBigInt(layer.Gammas[i])
		}
		for i := 0; i <= nv; i++ {
			assignment.Layers[li].Rhos[i] = parseBigInt(layer.Rhos[i])
		}
		assignment.Layers[li].PODPChallenge = parseBigInt(layer.PODPChallenge)
		if config.HasPoP(li) && len(assignment.Layers[li].PopChallenges) > 0 {
			if len(layer.PopChallenges) > 0 {
				assignment.Layers[li].PopChallenges[0] = parseBigInt(layer.PopChallenges[0])
			} else {
				assignment.Layers[li].PopChallenges[0] = zero
			}
		}

		// PODP witness
		if nv > 0 {
			assignment.LayerPODPs[li] = PODPWitness{
				ZVector: parseVarSlice(w.Witness.LayerPODPs[li].ZVector),
				ZDelta:  parseBigInt(w.Witness.LayerPODPs[li].ZDelta),
				ZBeta:   parseBigInt(w.Witness.LayerPODPs[li].ZBeta),
			}
		} else {
			assignment.LayerPODPs[li] = PODPWitness{
				ZVector: []frontend.Variable{},
				ZDelta:  zero,
				ZBeta:   zero,
			}
		}
		assignment.LayerPops[li] = PopWitness{Z1: zero, Z2: zero, Z3: zero, Z4: zero, Z5: zero}
	}

	// Input groups
	for g := 0; g < config.NumInputGroups; g++ {
		ig := w.PublicInputs.InputGroups[g]
		nc := config.InputGroupNumClaims[g]
		for i := 0; i < nc; i++ {
			assignment.InputGroups[g].RlcCoeffs[i] = parseBigInt(ig.RlcCoeffs[i])
		}
		assignment.InputGroups[g].PODPChallenge = parseBigInt(ig.PODPChallenge)

		// L-half and R-half bindings (resolved, from witness JSON)
		for i := 0; i < config.InputGroupLHalfLen[g]; i++ {
			assignment.InputGroups[g].LBindings[i] = parseBigInt(ig.LHalfBindings[i])
		}
		for i := 0; i < config.InputGroupRHalfLen[g]; i++ {
			assignment.InputGroups[g].RBindings[i] = parseBigInt(ig.RHalfBindings[i])
		}

		// Input group PODP witness
		if len(ig.ZVector) > 0 {
			assignment.InputGroupPODPs[g] = PODPWitness{
				ZVector: parseVarSlice(ig.ZVector),
				ZDelta:  parseBigInt(ig.ZDelta),
				ZBeta:   parseBigInt(ig.ZBeta),
			}
		} else {
			rHalfLen := config.InputGroupRHalfLen[g]
			zLen := 1 << rHalfLen
			zVec := make([]frontend.Variable, zLen)
			for i := range zVec {
				zVec[i] = zero
			}
			assignment.InputGroupPODPs[g] = PODPWitness{
				ZVector: zVec,
				ZDelta:  zero,
				ZBeta:   zero,
			}
		}
	}

	// Public outputs
	for i := 0; i < nLayers; i++ {
		assignment.RlcBeta[i] = parseBigInt(w.PublicOutputs.RlcBetas[i])
		assignment.ZDotJStar[i] = parseBigInt(w.PublicOutputs.ZDotJStars[i])
	}
	for i := range w.PublicOutputs.LTensorFlat {
		assignment.LTensorFlat[i] = parseBigInt(w.PublicOutputs.LTensorFlat[i])
	}
	for g := 0; g < config.NumInputGroups; g++ {
		assignment.ZDotR[g] = parseBigInt(w.PublicOutputs.ZDotRs[g])
	}
	for p := 0; p < config.NumPublicClaims; p++ {
		assignment.MleEval[p] = parseBigInt(w.PublicOutputs.MleEvals[p])
	}

	// Public claim points: resolve from atom routing for non-committed input layers
	desc := w.DAGCircuitDescription
	pubIdx := 0
	numInputLayers := len(desc.InputIsCommitted)
	for j := 0; j < numInputLayers; j++ {
		if desc.InputIsCommitted[j] {
			continue // committed layers produce input groups, not pub claims
		}
		targetIdx := nLayers + j
		if targetIdx >= len(desc.IncomingOffsets)-1 {
			continue
		}
		inStart := desc.IncomingOffsets[targetIdx]
		inEnd := desc.IncomingOffsets[targetIdx+1]
		for k := 0; k < inEnd-inStart && pubIdx < config.NumPublicClaims; k++ {
			globalAtom := desc.IncomingAtomIdx[inStart+k]
			source := 0
			for li := 0; li < nLayers; li++ {
				if globalAtom >= desc.AtomOffsets[li] && globalAtom < desc.AtomOffsets[li+1] {
					source = li
					break
				}
			}
			ptStart := desc.PtOffsets[globalAtom]
			ptEnd := desc.PtOffsets[globalAtom+1]
			for pi := ptStart; pi < ptEnd; pi++ {
				entry := desc.PtData[pi]
				idx := pi - ptStart
				if idx < config.PubClaimNumVars[pubIdx] {
					if entry < 1000 {
						assignment.PubClaimPoints[pubIdx][idx] = parseBigInt(w.PublicInputs.Layers[source].Bindings[entry])
					} else if entry >= FIXED_REF_BASE {
						assignment.PubClaimPoints[pubIdx][idx] = big.NewInt(int64(entry - FIXED_REF_BASE))
					}
				}
			}
			pubIdx++
		}
	}

	// Public input values (from ABI-encoded data)
	if w.PublicValuesABI != "" && len(w.PublicValuesABI) > 2 {
		hexStr := w.PublicValuesABI
		if len(hexStr) > 2 && hexStr[:2] == "0x" {
			hexStr = hexStr[2:]
		}
		for i := 0; i < config.PubInputCount && i*64 < len(hexStr); i++ {
			start := i * 64
			end := start + 64
			if end > len(hexStr) {
				end = len(hexStr)
			}
			v := new(big.Int)
			v.SetString(hexStr[start:end], 16)
			assignment.PublicInputValues[i] = v
		}
	} else {
		for i := 0; i < config.PubInputCount; i++ {
			assignment.PublicInputValues[i] = zero
		}
	}

	return assignment
}

// cmdDAGProveJSON generates a Groth16 proof for a DAG circuit from witness JSON.
func cmdDAGProveJSON() {
	data, err := io.ReadAll(os.Stdin)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error reading stdin: %v\n", err)
		os.Exit(1)
	}

	var w DAGWitnessJSON
	if err := json.Unmarshal(data, &w); err != nil {
		fmt.Fprintf(os.Stderr, "error parsing DAG witness JSON: %v\n", err)
		os.Exit(1)
	}

	config := dagConfigFromWitness(&w)
	fmt.Fprintf(os.Stderr, "DAG config: %d compute layers, %d input groups, %d public claims\n",
		config.NumComputeLayers, config.NumInputGroups, config.NumPublicClaims)

	assignment := buildDAGAssignment(config, &w)

	// Redirect stdout to stderr to suppress gnark's fmt.Printf warnings during compile/witness
	origStdout := os.Stdout
	os.Stdout = os.Stderr

	// Compile
	fmt.Fprintf(os.Stderr, "Compiling DAG circuit...\n")
	circuit := AllocateDAGCircuit(config)
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}
	fmt.Fprintf(os.Stderr, "DAG circuit compiled: %d constraints\n", ccs.GetNbConstraints())

	// Setup
	fmt.Fprintln(os.Stderr, "Running inline Groth16 setup...")
	pk, vk, err := groth16.Setup(ccs)
	if err != nil {
		fmt.Fprintf(os.Stderr, "setup error: %v\n", err)
		os.Exit(1)
	}

	// Export Solidity verifier
	solFile, solErr := os.Create("DAGRemainderGroth16Verifier.sol")
	if solErr == nil {
		if exportErr := vk.ExportSolidity(solFile); exportErr == nil {
			fmt.Fprintln(os.Stderr, "Solidity verifier exported to DAGRemainderGroth16Verifier.sol")
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
	proofPoints[0] = fpToHex(&proofBn254.Ar.X)
	proofPoints[1] = fpToHex(&proofBn254.Ar.Y)
	proofPoints[2] = fpToHex(&proofBn254.Bs.X.A1)
	proofPoints[3] = fpToHex(&proofBn254.Bs.X.A0)
	proofPoints[4] = fpToHex(&proofBn254.Bs.Y.A1)
	proofPoints[5] = fpToHex(&proofBn254.Bs.Y.A0)
	proofPoints[6] = fpToHex(&proofBn254.Krs.X)
	proofPoints[7] = fpToHex(&proofBn254.Krs.Y)

	// Build public inputs matching buildDAGGroth16Inputs layout
	var pubInputs []string

	// CircuitHash [2]
	pubInputs = append(pubInputs, w.PublicInputs.CircuitHash[0], w.PublicInputs.CircuitHash[1])
	// OutputChallenges
	pubInputs = append(pubInputs, w.PublicInputs.OutputChallenges...)

	// Per-layer
	for li := 0; li < config.NumComputeLayers; li++ {
		layer := w.PublicInputs.Layers[li]
		pubInputs = append(pubInputs, layer.RlcCoeffs...)
		pubInputs = append(pubInputs, layer.Bindings...)
		pubInputs = append(pubInputs, layer.Rhos...)
		pubInputs = append(pubInputs, layer.Gammas...)
		pubInputs = append(pubInputs, layer.PODPChallenge)
		if config.HasPoP(li) {
			if len(layer.PopChallenges) > 0 {
				pubInputs = append(pubInputs, layer.PopChallenges[0])
			} else {
				pubInputs = append(pubInputs, "0x0")
			}
		}
	}

	// Per-input-group (rlcCoeffs + podpChallenge + lBindings + rBindings)
	for g := 0; g < config.NumInputGroups; g++ {
		ig := w.PublicInputs.InputGroups[g]
		pubInputs = append(pubInputs, ig.RlcCoeffs...)
		pubInputs = append(pubInputs, ig.PODPChallenge)
		pubInputs = append(pubInputs, ig.LHalfBindings...)
		pubInputs = append(pubInputs, ig.RHalfBindings...)
	}

	// Per-public-claim points (resolved from atom routing)
	dagDesc := w.DAGCircuitDescription
	numInputLayersCmd := len(dagDesc.InputIsCommitted)
	for j := 0; j < numInputLayersCmd; j++ {
		if dagDesc.InputIsCommitted[j] {
			continue
		}
		targetIdx := config.NumComputeLayers + j
		if targetIdx >= len(dagDesc.IncomingOffsets)-1 {
			continue
		}
		inStart := dagDesc.IncomingOffsets[targetIdx]
		inEnd := dagDesc.IncomingOffsets[targetIdx+1]
		for k := 0; k < inEnd-inStart; k++ {
			globalAtom := dagDesc.IncomingAtomIdx[inStart+k]
			source := 0
			for li := 0; li < config.NumComputeLayers; li++ {
				if globalAtom >= dagDesc.AtomOffsets[li] && globalAtom < dagDesc.AtomOffsets[li+1] {
					source = li
					break
				}
			}
			ptStart := dagDesc.PtOffsets[globalAtom]
			ptEnd := dagDesc.PtOffsets[globalAtom+1]
			for pi := ptStart; pi < ptEnd; pi++ {
				entry := dagDesc.PtData[pi]
				if entry < 1000 {
					pubInputs = append(pubInputs, w.PublicInputs.Layers[source].Bindings[entry])
				} else if entry >= FIXED_REF_BASE {
					val := big.NewInt(int64(entry - FIXED_REF_BASE))
					pubInputs = append(pubInputs, fmt.Sprintf("0x%x", val))
				}
			}
		}
	}

	// Public input values (from ABI-encoded data)
	if w.PublicValuesABI != "" && len(w.PublicValuesABI) > 2 {
		hexStr := w.PublicValuesABI
		if len(hexStr) > 2 && hexStr[:2] == "0x" {
			hexStr = hexStr[2:]
		}
		for i := 0; i < config.PubInputCount && i*64 < len(hexStr); i++ {
			start := i * 64
			end := start + 64
			if end > len(hexStr) {
				end = len(hexStr)
			}
			pubInputs = append(pubInputs, "0x"+hexStr[start:end])
		}
	}

	// Outputs
	pubInputs = append(pubInputs, w.PublicOutputs.RlcBetas...)
	pubInputs = append(pubInputs, w.PublicOutputs.ZDotJStars...)
	pubInputs = append(pubInputs, w.PublicOutputs.LTensorFlat...)
	pubInputs = append(pubInputs, w.PublicOutputs.ZDotRs...)
	pubInputs = append(pubInputs, w.PublicOutputs.MleEvals...)

	fixture := map[string]interface{}{
		"proof":         proofPoints,
		"public_inputs": pubInputs,
	}

	out, _ := json.MarshalIndent(fixture, "", "  ")
	fmt.Println(string(out))
	fmt.Fprintf(os.Stderr, "DAG JSON fixture written (%d proof points, %d public inputs)\n",
		len(proofPoints), len(pubInputs))
}

// cmdDAGInfo shows DAG circuit statistics from witness JSON on stdin.
func cmdDAGInfo() {
	data, err := io.ReadAll(os.Stdin)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error reading stdin: %v\n", err)
		os.Exit(1)
	}

	var w DAGWitnessJSON
	if err := json.Unmarshal(data, &w); err != nil {
		fmt.Fprintf(os.Stderr, "error parsing DAG witness JSON: %v\n", err)
		os.Exit(1)
	}

	config := dagConfigFromWitness(&w)
	fmt.Fprintf(os.Stderr, "Compiling DAG circuit (%d layers, %d input groups)...\n",
		config.NumComputeLayers, config.NumInputGroups)

	circuit := AllocateDAGCircuit(config)
	origStdout := os.Stdout
	os.Stdout = os.Stderr
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, circuit)
	os.Stdout = origStdout
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("DAG Config: %d compute layers, %d input groups, %d public claims\n",
		config.NumComputeLayers, config.NumInputGroups, config.NumPublicClaims)
	fmt.Printf("Constraints: %d\n", ccs.GetNbConstraints())
	fmt.Printf("Expected public inputs: %d\n", config.DAGExpectedPublicInputCount())
}
