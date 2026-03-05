package main

import (
	"github.com/consensys/gnark/frontend"
)

// DAGCircuitConfig defines the circuit dimensions for a DAG-topology GKR circuit.
// Supports variable claim counts per layer, point templates, and multiple input groups.
type DAGCircuitConfig struct {
	NumComputeLayers int   // number of compute layers (e.g., 88)
	LayerNumVars     []int // per-layer num_vars (binding count); len = NumComputeLayers
	LayerDegrees     []int // degree of each layer; len = NumComputeLayers
	LayerNumClaims   []int // number of incoming claims per layer; len = NumComputeLayers
	OutputNumVars    int   // output challenge count

	// Input groups (from committed input layer claim sorting/grouping)
	NumInputGroups      int   // total number of input groups
	InputGroupNumClaims []int // number of claims per input group
	InputGroupLHalfLen  []int // L-half binding count per group
	InputGroupRHalfLen  []int // R-half binding count per group

	// Public value claims (from public input layer)
	NumPublicClaims int   // number of public value claims
	PubInputCount   int   // number of embedded public input values (for MLE eval)
	PubClaimNumVars []int // num_vars per public claim point

	// Compute layer point templates: compile-time constants describing how to resolve claim points.
	// PointTemplates[layer][claim] is a slice of template entries.
	// Entry < 1000: binding ref from source layer (entry = binding index)
	// Entry >= 20000: fixed value (entry - 20000)
	// PointTemplateSources[layer][claim] = source layer index (-1 = output challenges)
	PointTemplates       [][][]int // [numComputeLayers][numClaims[layer]][]int
	PointTemplateSources [][]int   // [numComputeLayers][numClaims[layer]]
}

const FIXED_REF_BASE = 20000

// HasPoP returns true if the given layer has a ProofOfProduct challenge (degree > 2).
func (c DAGCircuitConfig) HasPoP(layerIdx int) bool {
	return c.LayerDegrees[layerIdx] > 2
}

// DAGLayerPublic holds per-layer public inputs for the DAG Groth16 circuit.
type DAGLayerPublic struct {
	RlcCoeffs     []frontend.Variable // numClaims per layer
	Bindings      []frontend.Variable // numVars per layer
	Rhos          []frontend.Variable // numVars+1
	Gammas        []frontend.Variable // numVars
	PODPChallenge frontend.Variable
	PopChallenges []frontend.Variable // variable length
}

// DAGInputGroupPublic holds per-input-group public inputs.
type DAGInputGroupPublic struct {
	RlcCoeffs     []frontend.Variable // numClaims per group
	PODPChallenge frontend.Variable
	LBindings     []frontend.Variable // L-half bindings (resolved, from claim point)
	RBindings     []frontend.Variable // R-half bindings (resolved, from claim point)
}

// DAGWrapperCircuit wraps DAG GKR algebraic verification inside a Groth16 SNARK.
//
// Public input layout matches Solidity buildDAGGroth16Inputs():
//
//	circuitHash[2] | outputChallenges[V] |
//	per-layer{ rlcCoeffs[numClaims], bindings[nv], rhos[nv+1], gammas[nv], podpChallenge, popChallenges[] } |
//	per-inputGroup{ rlcCoeffs[numClaims], podpChallenge, lBindings[lHalfLen], rBindings[rHalfLen] } |
//	per-pubClaim{ claimPoint[numVars] } |
//	publicInputValues[P] |
//	outputs{ rlcBeta[L], zDotJStar[L], lTensorFlat[...], zDotR[G], mleEval[P] }
type DAGWrapperCircuit struct {
	Config DAGCircuitConfig `gnark:"-"`

	// === Public inputs ===
	CircuitHash      [2]frontend.Variable `gnark:",public"`
	OutputChallenges []frontend.Variable  `gnark:",public"`

	// Per-layer challenges
	Layers []DAGLayerPublic `gnark:",public"`

	// Per-input-group challenges + bindings
	InputGroups []DAGInputGroupPublic `gnark:",public"`

	// Per-public-claim points (resolved from source layer bindings)
	PubClaimPoints [][]frontend.Variable `gnark:",public"`

	// Public input values for MLE eval
	PublicInputValues []frontend.Variable `gnark:",public"`

	// === Public outputs ===
	RlcBeta     []frontend.Variable `gnark:",public"` // one per compute layer
	ZDotJStar   []frontend.Variable `gnark:",public"` // one per compute layer
	LTensorFlat []frontend.Variable `gnark:",public"` // flattened across all input groups
	ZDotR       []frontend.Variable `gnark:",public"` // one per input group
	MleEval     []frontend.Variable `gnark:",public"` // one per public claim

	// === Private witness ===
	LayerPODPs      []PODPWitness
	LayerPops       []PopWitness
	InputGroupPODPs []PODPWitness
}

// Define implements the gnark circuit interface for DAG topology.
func (c *DAGWrapperCircuit) Define(api frontend.API) error {
	cfg := c.Config

	// ================================================================
	// Per compute layer: rlcBeta and zDotJStar
	// ================================================================
	for i := 0; i < cfg.NumComputeLayers; i++ {
		rlcBeta := c.computeDAGRlcBeta(api, i)
		api.AssertIsEqual(rlcBeta, c.RlcBeta[i])

		nv := cfg.LayerNumVars[i]
		if nv > 0 {
			jStar := computeJStar(api, c.Layers[i].Rhos, c.Layers[i].Gammas,
				c.Layers[i].Bindings, cfg.LayerDegrees[i])
			zDotJStar := innerProduct(api, c.LayerPODPs[i].ZVector, jStar)
			api.AssertIsEqual(zDotJStar, c.ZDotJStar[i])
		} else {
			api.AssertIsEqual(c.ZDotJStar[i], 0)
		}
	}

	// ================================================================
	// Per input group: lTensor and zDotR
	// ================================================================
	ltIdx := 0
	for g := 0; g < cfg.NumInputGroups; g++ {
		numClaims := cfg.InputGroupNumClaims[g]
		lHalfLen := cfg.InputGroupLHalfLen[g]

		// L-tensor: for each claim, scale tensor(lBindings) by RLC coeff
		lPerShred := computeTensorProduct(api, c.InputGroups[g].LBindings)
		lPerShredLen := 1 << lHalfLen
		for ci := 0; ci < numClaims; ci++ {
			for j := 0; j < lPerShredLen; j++ {
				expected := api.Mul(c.InputGroups[g].RlcCoeffs[ci], lPerShred[j])
				api.AssertIsEqual(expected, c.LTensorFlat[ltIdx])
				ltIdx++
			}
		}

		// R-tensor and zDotR = <z_vector, R_tensor>
		rTensor := computeTensorProduct(api, c.InputGroups[g].RBindings)
		zDotR := innerProduct(api, c.InputGroupPODPs[g].ZVector, rTensor)
		api.AssertIsEqual(zDotR, c.ZDotR[g])
	}

	// ================================================================
	// Per public claim: mleEval
	// ================================================================
	for p := 0; p < cfg.NumPublicClaims; p++ {
		mleEval := evaluateMLE(api, c.PublicInputValues, c.PubClaimPoints[p])
		api.AssertIsEqual(mleEval, c.MleEval[p])
	}

	return nil
}

// computeDAGRlcBeta computes rlcBeta for a DAG layer with multiple claims.
// rlcBeta = SUM_k(beta(bindings, claimPoint_k) * rlcCoeff_k)
func (c *DAGWrapperCircuit) computeDAGRlcBeta(api frontend.API, layerIdx int) frontend.Variable {
	cfg := c.Config
	numClaims := cfg.LayerNumClaims[layerIdx]
	bindings := c.Layers[layerIdx].Bindings

	rlcBeta := frontend.Variable(0)
	for k := 0; k < numClaims; k++ {
		claimPoint := c.resolvePoint(api, cfg.PointTemplates[layerIdx][k], cfg.PointTemplateSources[layerIdx][k])

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

		scaled := api.Mul(beta, c.Layers[layerIdx].RlcCoeffs[k])
		rlcBeta = api.Add(rlcBeta, scaled)
	}

	return rlcBeta
}

// resolvePoint resolves a point template to actual field values using source layer bindings.
// Template entries: < 1000 = binding ref, >= 20000 = fixed value (entry - 20000).
// sourceLayer = -1 means use OutputChallenges instead of layer bindings.
func (c *DAGWrapperCircuit) resolvePoint(api frontend.API, template []int, sourceLayer int) []frontend.Variable {
	point := make([]frontend.Variable, len(template))
	for i, entry := range template {
		if entry < 1000 {
			if sourceLayer == -1 {
				point[i] = c.OutputChallenges[entry]
			} else {
				point[i] = c.Layers[sourceLayer].Bindings[entry]
			}
		} else if entry >= FIXED_REF_BASE {
			point[i] = frontend.Variable(entry - FIXED_REF_BASE)
		}
	}
	return point
}

// AllocateDAGCircuit returns a circuit definition with correct slice sizes for the given DAG config.
func AllocateDAGCircuit(config DAGCircuitConfig) *DAGWrapperCircuit {
	nLayers := config.NumComputeLayers

	layers := make([]DAGLayerPublic, nLayers)
	layerPODPs := make([]PODPWitness, nLayers)
	layerPops := make([]PopWitness, nLayers)
	for i := 0; i < nLayers; i++ {
		nv := config.LayerNumVars[i]
		numClaims := config.LayerNumClaims[i]
		popLen := 0
		if config.HasPoP(i) {
			popLen = 1
		}
		layers[i] = DAGLayerPublic{
			RlcCoeffs:     make([]frontend.Variable, numClaims),
			Bindings:      make([]frontend.Variable, nv),
			Rhos:          make([]frontend.Variable, nv+1),
			Gammas:        make([]frontend.Variable, nv),
			PopChallenges: make([]frontend.Variable, popLen),
		}
		zLen := 0
		if nv > 0 {
			zLen = (config.LayerDegrees[i] + 1) * nv
		}
		layerPODPs[i] = PODPWitness{
			ZVector: make([]frontend.Variable, zLen),
		}
	}

	inputGroups := make([]DAGInputGroupPublic, config.NumInputGroups)
	inputGroupPODPs := make([]PODPWitness, config.NumInputGroups)
	totalLTensor := 0
	for g := 0; g < config.NumInputGroups; g++ {
		numClaims := config.InputGroupNumClaims[g]
		lHalfLen := config.InputGroupLHalfLen[g]
		rHalfLen := config.InputGroupRHalfLen[g]
		inputGroups[g] = DAGInputGroupPublic{
			RlcCoeffs: make([]frontend.Variable, numClaims),
			LBindings: make([]frontend.Variable, lHalfLen),
			RBindings: make([]frontend.Variable, rHalfLen),
		}
		inputGroupPODPs[g] = PODPWitness{
			ZVector: make([]frontend.Variable, 1<<rHalfLen),
		}
		totalLTensor += numClaims * (1 << lHalfLen)
	}

	pubClaimPoints := make([][]frontend.Variable, config.NumPublicClaims)
	for p := 0; p < config.NumPublicClaims; p++ {
		pubClaimPoints[p] = make([]frontend.Variable, config.PubClaimNumVars[p])
	}

	return &DAGWrapperCircuit{
		Config:            config,
		OutputChallenges:  make([]frontend.Variable, config.OutputNumVars),
		Layers:            layers,
		InputGroups:       inputGroups,
		PubClaimPoints:    pubClaimPoints,
		PublicInputValues: make([]frontend.Variable, config.PubInputCount),
		RlcBeta:           make([]frontend.Variable, nLayers),
		ZDotJStar:         make([]frontend.Variable, nLayers),
		LTensorFlat:       make([]frontend.Variable, totalLTensor),
		ZDotR:             make([]frontend.Variable, config.NumInputGroups),
		MleEval:           make([]frontend.Variable, config.NumPublicClaims),
		LayerPODPs:        layerPODPs,
		LayerPops:         layerPops,
		InputGroupPODPs:   inputGroupPODPs,
	}
}

// DAGExpectedPublicInputCount returns the expected number of Groth16 public inputs.
func (c DAGCircuitConfig) DAGExpectedPublicInputCount() int {
	total := 2 + c.OutputNumVars

	for i := 0; i < c.NumComputeLayers; i++ {
		total += c.LayerNumClaims[i] // rlcCoeffs
		nv := c.LayerNumVars[i]
		total += nv     // bindings
		total += nv + 1 // rhos
		total += nv     // gammas
		total += 1      // podpChallenge
		if c.HasPoP(i) {
			total += 1
		}
	}

	for g := 0; g < c.NumInputGroups; g++ {
		total += c.InputGroupNumClaims[g] // rlcCoeffs
		total += 1                         // podpChallenge
		total += c.InputGroupLHalfLen[g]  // lBindings
		total += c.InputGroupRHalfLen[g]  // rBindings
	}

	for p := 0; p < c.NumPublicClaims; p++ {
		total += c.PubClaimNumVars[p]
	}

	total += c.PubInputCount // publicInputValues

	total += c.NumComputeLayers // rlcBeta
	total += c.NumComputeLayers // zDotJStar
	for g := 0; g < c.NumInputGroups; g++ {
		lHalfLen := c.InputGroupLHalfLen[g]
		total += c.InputGroupNumClaims[g] * (1 << lHalfLen)
	}
	total += c.NumInputGroups  // zDotR
	total += c.NumPublicClaims // mleEval

	return total
}
