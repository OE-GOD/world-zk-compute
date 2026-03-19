package main

import (
	"testing"

	"github.com/consensys/gnark-crypto/ecc"
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
	// Measure constraint scaling for planning
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
