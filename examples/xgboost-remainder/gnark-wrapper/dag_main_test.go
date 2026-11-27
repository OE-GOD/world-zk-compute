package main

import (
	"encoding/json"
	"fmt"
	"math/big"
	"strings"
	"testing"

	"github.com/consensys/gnark-crypto/ecc"
	bn254 "github.com/consensys/gnark-crypto/ecc/bn254"
	"github.com/consensys/gnark-crypto/ecc/bn254/fp"
	"github.com/consensys/gnark/backend/groth16"
	groth16bn254 "github.com/consensys/gnark/backend/groth16/bn254"
	"github.com/consensys/gnark/backend/witness"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"
)

// oneLayerDAGConfig is the smallest DAG config buildDAGAssignment accepts: one
// compute layer with no sumcheck rounds and one public input value.
func oneLayerDAGConfig() DAGCircuitConfig {
	return DAGCircuitConfig{
		NumComputeLayers:     1,
		LayerNumVars:         []int{0},
		LayerDegrees:         []int{2},
		LayerNumClaims:       []int{1},
		PubInputCount:        1,
		PointTemplates:       [][][]int{{{}}},
		PointTemplateSources: [][]int{{-1}},
	}
}

// oneLayerDAGWitness returns witness JSON for oneLayerDAGConfig with one field
// replaced by a raw JSON value.
func oneLayerDAGWitness(t *testing.T, field, value string) *DAGWitnessJSON {
	t.Helper()
	fields := map[string]string{
		"circuit_hash":      `["0x1", "0x2"]`,
		"rlc_betas":         `["0x3"]`,
		"public_values_abi": `"0x` + strings.Repeat("0", 63) + `4"`,
	}
	if _, ok := fields[field]; !ok {
		t.Fatalf("unknown field %s", field)
	}
	fields[field] = value
	raw := fmt.Sprintf(`{
		"public_inputs": {
			"circuit_hash": %s,
			"layers": [{"rlc_coeffs": ["0x5"], "rhos": ["0x6"], "podp_challenge": "0x7"}]
		},
		"public_outputs": {"rlc_betas": %s, "z_dot_jstars": ["0x0"]},
		"public_values_abi": %s
	}`, fields["circuit_hash"], fields["rlc_betas"], fields["public_values_abi"])
	var w DAGWitnessJSON
	if err := json.Unmarshal([]byte(raw), &w); err != nil {
		t.Fatalf("test witness JSON: %v", err)
	}
	return &w
}

func TestBuildDAGAssignmentAcceptsWellFormedNumbers(t *testing.T) {
	a, err := buildDAGAssignment(oneLayerDAGConfig(), oneLayerDAGWitness(t, "rlc_betas", `["0x3"]`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if got := a.PublicInputValues[0].(*big.Int); got.Cmp(big.NewInt(4)) != 0 {
		t.Fatalf("public input value = %v, want 4", got)
	}
}

func TestBuildDAGAssignmentRejectsMalformedNumbers(t *testing.T) {
	cases := map[string]string{
		"circuit_hash":      `["0xnothex", "0x2"]`,
		"rlc_betas":         `[""]`,
		"public_values_abi": `"0x` + strings.Repeat("z", 64) + `"`,
	}
	for field, value := range cases {
		if _, err := buildDAGAssignment(oneLayerDAGConfig(), oneLayerDAGWitness(t, field, value)); err == nil {
			t.Fatalf("%s = %s: want an error, got none", field, value)
		}
	}
}

// The fixture's public inputs must be the reduced values gnark verified, in
// circuit order, as 64-hex-digit strings.
func TestFixturePublicInputsAreTheVerifiedPublicWitness(t *testing.T) {
	config := smallDAGConfig()
	assignment := makeDAGDummyAssignment(config)
	// A circuit hash word above the field modulus is reduced by gnark; the
	// fixture must carry the reduced value that was verified.
	assignment.CircuitHash[0] = new(big.Int).Add(ecc.BN254.ScalarField(), big.NewInt(5))

	w, err := frontend.NewWitness(assignment, ecc.BN254.ScalarField(), frontend.PublicOnly())
	if err != nil {
		t.Fatalf("NewWitness: %v", err)
	}
	got, err := publicInputsHex(w)
	if err != nil {
		t.Fatalf("publicInputsHex: %v", err)
	}

	if len(got) != config.DAGExpectedPublicInputCount() {
		t.Fatalf("got %d public inputs, want %d", len(got), config.DAGExpectedPublicInputCount())
	}
	hex64 := func(v *big.Int) string { return fmt.Sprintf("0x%064x", v) }
	if want := hex64(big.NewInt(5)); got[0] != want {
		t.Fatalf("public input 0 = %s, want %s", got[0], want)
	}
	if want := hex64(assignment.CircuitHash[1].(*big.Int)); got[1] != want {
		t.Fatalf("public input 1 = %s, want %s", got[1], want)
	}
	last := assignment.MleEval[len(assignment.MleEval)-1].(*big.Int)
	if want := hex64(new(big.Int).Mod(last, ecc.BN254.ScalarField())); got[len(got)-1] != want {
		t.Fatalf("last public input = %s, want %s", got[len(got)-1], want)
	}
}

// fixtureTestCircuit is a tiny circuit with two public inputs, small enough to
// prove in a test: Root*Root == Square and Root+5 == Shifted.
type fixtureTestCircuit struct {
	Square  frontend.Variable `gnark:",public"`
	Shifted frontend.Variable `gnark:",public"`
	Root    frontend.Variable
}

func (c *fixtureTestCircuit) Define(api frontend.API) error {
	api.AssertIsEqual(api.Mul(c.Root, c.Root), c.Square)
	api.AssertIsEqual(api.Add(c.Root, 5), c.Shifted)
	return nil
}

// proveFixtureTestCircuit proves Root = 3 the way cmdDAGProveJSON proves the
// DAG circuit, with Shifted given as r+8 so gnark reduces it to 8.
func proveFixtureTestCircuit(t *testing.T) (groth16.VerifyingKey, groth16.Proof, witness.Witness) {
	t.Helper()
	field := ecc.BN254.ScalarField()
	ccs, err := frontend.Compile(field, r1cs.NewBuilder, &fixtureTestCircuit{})
	if err != nil {
		t.Fatalf("compile: %v", err)
	}
	pk, vk, err := groth16.Setup(ccs)
	if err != nil {
		t.Fatalf("setup: %v", err)
	}
	assignment := &fixtureTestCircuit{
		Square:  9,
		Shifted: new(big.Int).Add(field, big.NewInt(8)),
		Root:    3,
	}
	full, err := frontend.NewWitness(assignment, field)
	if err != nil {
		t.Fatalf("NewWitness: %v", err)
	}
	proof, err := groth16.Prove(ccs, pk, full)
	if err != nil {
		t.Fatalf("prove: %v", err)
	}
	public, err := full.Public()
	if err != nil {
		t.Fatalf("Public: %v", err)
	}
	if err := groth16.Verify(proof, vk, public); err != nil {
		t.Fatalf("verify: %v", err)
	}
	return vk, proof, public
}

// The fixture must carry exactly the public witness gnark verified (reduced,
// in declaration order) and proof points that verify against it.
func TestDAGFixtureCarriesTheVerifiedProofAndPublicWitness(t *testing.T) {
	vk, proof, public := proveFixtureTestCircuit(t)

	fixture, err := dagFixture(proof, public)
	if err != nil {
		t.Fatalf("dagFixture: %v", err)
	}

	hex64 := func(v int64) string { return fmt.Sprintf("0x%064x", big.NewInt(v)) }
	want := []string{hex64(9), hex64(8)}
	if len(fixture.PublicInputs) != len(want) {
		t.Fatalf("got %d public inputs %v, want %v", len(fixture.PublicInputs), fixture.PublicInputs, want)
	}
	for i := range want {
		if fixture.PublicInputs[i] != want[i] {
			t.Fatalf("public input %d = %s, want %s", i, fixture.PublicInputs[i], want[i])
		}
	}

	// Read the fixture back as a verifier would and check it verifies.
	if len(fixture.Proof) != 8 {
		t.Fatalf("got %d proof points, want 8", len(fixture.Proof))
	}
	var p groth16bn254.Proof
	coords := []*fp.Element{
		&p.Ar.X, &p.Ar.Y,
		&p.Bs.X.A1, &p.Bs.X.A0, &p.Bs.Y.A1, &p.Bs.Y.A0,
		&p.Krs.X, &p.Krs.Y,
	}
	for i, c := range coords {
		if _, err := c.SetString(fixture.Proof[i]); err != nil {
			t.Fatalf("proof point %d %q: %v", i, fixture.Proof[i], err)
		}
	}
	values := make([]*big.Int, len(fixture.PublicInputs))
	for i, s := range fixture.PublicInputs {
		v, ok := new(big.Int).SetString(s, 0)
		if !ok {
			t.Fatalf("public input %d %q is not a number", i, s)
		}
		values[i] = v
	}
	readBack, err := frontend.NewWitness(&fixtureTestCircuit{Square: values[0], Shifted: values[1]},
		ecc.BN254.ScalarField(), frontend.PublicOnly())
	if err != nil {
		t.Fatalf("NewWitness from fixture: %v", err)
	}
	if err := groth16.Verify(&p, vk, readBack); err != nil {
		t.Fatalf("fixture does not verify: %v", err)
	}
}

// The fixture has no field for Pedersen commitments, so a proof that has them
// must be refused rather than written out unverifiable.
func TestDAGFixtureRejectsProofWithCommitments(t *testing.T) {
	_, proof, public := proveFixtureTestCircuit(t)
	p := proof.(*groth16bn254.Proof)
	p.Commitments = append(p.Commitments, bn254.G1Affine{})

	if _, err := dagFixture(p, public); err == nil {
		t.Fatal("want an error for a proof with commitments, got none")
	}
}
