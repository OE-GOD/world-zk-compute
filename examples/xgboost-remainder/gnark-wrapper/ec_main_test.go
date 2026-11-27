package main

import (
	"fmt"
	"math/big"
	"testing"

	"github.com/consensys/gnark-crypto/ecc/bn254"
)

func ecPointJSON(p bn254.G1Affine) ECPointJSON {
	xy := g1XY(&p)
	return ECPointJSON{X: fmt.Sprintf("0x%x", xy[0]), Y: fmt.Sprintf("0x%x", xy[1])}
}

// msmOfGAnd2G is the MSM 1*G + 1*2G with the given claimed result.
func msmOfGAnd2G(result bn254.G1Affine) []ECOp {
	_, _, g, _ := bn254.Generators()
	var g2 bn254.G1Affine
	g2.ScalarMultiplication(&g, big.NewInt(2))
	res := ecPointJSON(result)
	return []ECOp{{
		Type:    "msm",
		Points:  []ECPointJSON{ecPointJSON(g), ecPointJSON(g2)},
		Scalars: []string{"0x1", "0x1"},
		Result:  &res,
	}}
}

func TestDecomposeECOpsAcceptsCorrectMSMResult(t *testing.T) {
	_, _, g, _ := bn254.Generators()
	var g3 bn254.G1Affine
	g3.ScalarMultiplication(&g, big.NewInt(3))

	muls, adds, err := decomposeECOps(msmOfGAnd2G(g3))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(muls) != 2 || len(adds) != 1 {
		t.Fatalf("got %d muls and %d adds, want 2 and 1", len(muls), len(adds))
	}
}

func TestDecomposeECOpsRejectsWrongMSMResult(t *testing.T) {
	_, _, g, _ := bn254.Generators()
	// 1*G + 1*2G is 3G, not G.
	if _, _, err := decomposeECOps(msmOfGAnd2G(g)); err == nil {
		t.Fatal("an MSM whose claimed result is not the sum of its terms was accepted")
	}
}

func TestDecomposeECOpsRejectsWrongSingleTermMSMResult(t *testing.T) {
	_, _, g, _ := bn254.Generators()
	res := ecPointJSON(g)
	// 2*G claimed to be G.
	ops := []ECOp{{Type: "msm", Points: []ECPointJSON{ecPointJSON(g)}, Scalars: []string{"0x2"}, Result: &res}}
	if _, _, err := decomposeECOps(ops); err == nil {
		t.Fatal("a one-term MSM with a wrong claimed result was accepted")
	}
}

func TestDecomposeECOpsRejectsMSMWithMissingScalar(t *testing.T) {
	_, _, g, _ := bn254.Generators()
	ops := msmOfGAnd2G(g)
	ops[0].Scalars = ops[0].Scalars[:1]
	if _, _, err := decomposeECOps(ops); err == nil {
		t.Fatal("an MSM with fewer scalars than points was accepted")
	}
}

func TestDecomposeECOpsRejectsMalformedNumbers(t *testing.T) {
	_, _, g, _ := bn254.Generators()
	p := ecPointJSON(g)
	ops := []ECOp{{Type: "mul", Point: &p, Scalar: "0xnothex", Result: &p}}
	if _, _, err := decomposeECOps(ops); err == nil {
		t.Fatal("a malformed scalar was accepted")
	}
}
