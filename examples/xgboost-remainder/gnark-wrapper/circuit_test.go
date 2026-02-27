package main

import (
	"math/big"
	"testing"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/backend"
	"github.com/consensys/gnark/test"
)

// TestWrapperCircuitEndToEnd verifies the full wrapper circuit with valid witness values.
// Uses the native Go Poseidon (over Fr) to compute the expected challenge, then
// proves the circuit and verifies the Groth16 proof.
func TestWrapperCircuitEndToEnd(t *testing.T) {
	params := NewPoseidonParams()

	// Dummy transcript values (matching the pattern from the E2E fixture)
	circuitHashFq1, _ := new(big.Int).SetString("5a9fc1776ad5b87f01d493983001d78f", 16)
	circuitHashFq2, _ := new(big.Int).SetString("5daa4b26c457652c4f9715758acff1bc", 16)
	pubInput0 := big.NewInt(6)
	pubInput1 := big.NewInt(20)
	pubHashChain1, _ := new(big.Int).SetString("a998b9d31f69d8ae8e48768cf8b8a5ff", 16)
	pubHashChain2, _ := new(big.Int).SetString("c06bcddbc1b4d72d89678361cd10177b", 16)
	commitX0, _ := new(big.Int).SetString("1be725444751be14b75a8d3f9635338b1c9a4bc6ec128dc038c1b3e3657b6751", 16)
	commitY0, _ := new(big.Int).SetString("29acc709c5e329e3a561a90c16d62ef1053900e2c06b8931293469b540d1309e", 16)
	commitX1, _ := new(big.Int).SetString("23385f96de594180eb38ae9f9d0d2aa9af09c467c78f4c17e700f6f5b3ae96fe", 16)
	commitY1, _ := new(big.Int).SetString("2ef6deb672454c927e50322143c9bdebc720f4df6001f4da6863eca6986bbb90", 16)
	ecHashChain1, _ := new(big.Int).SetString("d744e81c0ba8f5f2737a3a4ef940ace9", 16)
	ecHashChain2, _ := new(big.Int).SetString("c07203138670b777fcb4190942c7ac6e", 16)

	// Compute challenge using native Poseidon over Fr (NOT Fq)
	sponge := NewNativePoseidonSponge(params)
	sponge.Absorb(circuitHashFq1)
	sponge.Absorb(circuitHashFq2)
	sponge.Absorb(pubInput0)
	sponge.Absorb(pubInput1)
	sponge.Absorb(pubHashChain1)
	sponge.Absorb(pubHashChain2)
	sponge.Absorb(commitX0)
	sponge.Absorb(commitY0)
	sponge.Absorb(commitX1)
	sponge.Absorb(commitY1)
	sponge.Absorb(ecHashChain1)
	sponge.Absorb(ecHashChain2)
	challenge := sponge.Squeeze()

	t.Logf("Native Fr challenge: %s", challenge.Text(16))

	// Create the assignment with computed challenge
	assignment := &RemainderWrapperCircuit{
		CircuitHashFq1: circuitHashFq1,
		CircuitHashFq2: circuitHashFq2,
		PublicInput0:   pubInput0,
		PublicInput1:   pubInput1,
		Challenge:      challenge,
		PubHashChain1:  pubHashChain1,
		PubHashChain2:  pubHashChain2,
		EcHashChain1:   ecHashChain1,
		EcHashChain2:   ecHashChain2,
		CommitPointX0:  commitX0,
		CommitPointY0:  commitY0,
		CommitPointX1:  commitX1,
		CommitPointY1:  commitY1,
	}

	// Prove and verify
	assert := test.NewAssert(t)
	assert.ProverSucceeded(
		&RemainderWrapperCircuit{},
		assignment,
		test.WithCurves(ecc.BN254),
		test.WithBackends(backend.GROTH16),
	)
	t.Log("Wrapper circuit Groth16 proof verified successfully")
}
