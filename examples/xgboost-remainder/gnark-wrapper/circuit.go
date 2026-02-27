package main

import (
	"github.com/consensys/gnark/frontend"
)

// RemainderWrapperCircuit wraps a GKR+Hyrax proof verification inside a Groth16 SNARK.
//
// The circuit replays the Fiat-Shamir transcript (Poseidon sponge) and verifies
// the sumcheck + Hyrax polynomial commitment. The resulting Groth16 proof can be
// verified on-chain for ~200-240K gas regardless of the inner circuit complexity.
//
// Public inputs (visible on-chain):
//   - CircuitHashFq1, CircuitHashFq2: The circuit description hash split into 2 BN254 Fq elements
//   - PublicInputs: The actual public inputs of the inner circuit
//   - Challenge: The first Fiat-Shamir challenge (for output claim)
//
// Private witness (provided by the off-chain prover):
//   - All GKR+Hyrax proof elements
//   - Intermediate transcript values
type RemainderWrapperCircuit struct {
	// === Public inputs (on-chain) ===
	CircuitHashFq1 frontend.Variable `gnark:",public"`
	CircuitHashFq2 frontend.Variable `gnark:",public"`
	PublicInput0   frontend.Variable `gnark:",public"`
	PublicInput1   frontend.Variable `gnark:",public"`
	Challenge      frontend.Variable `gnark:",public"`

	// === Private witness (off-chain) ===
	// Hash chain values (SHA-256 digests split into 128-bit halves, absorbed into Poseidon)
	PubHashChain1 frontend.Variable
	PubHashChain2 frontend.Variable
	EcHashChain1  frontend.Variable
	EcHashChain2  frontend.Variable

	// Input commitment EC points (x, y coordinates)
	CommitPointX0 frontend.Variable
	CommitPointY0 frontend.Variable
	CommitPointX1 frontend.Variable
	CommitPointY1 frontend.Variable
}

// Define implements the gnark circuit interface.
func (c *RemainderWrapperCircuit) Define(api frontend.API) error {
	params := NewPoseidonParams()
	sponge := NewPoseidonSponge(api, params)

	// Replay the Fiat-Shamir transcript exactly as the Solidity verifier does:
	// 1. Circuit description hash (2 Fq elements)
	sponge.Absorb(c.CircuitHashFq1)
	sponge.Absorb(c.CircuitHashFq2)

	// 2. Public input values
	sponge.Absorb(c.PublicInput0)
	sponge.Absorb(c.PublicInput1)

	// 3. Public input hash chain (SHA-256 of public inputs, split into 128-bit halves)
	sponge.Absorb(c.PubHashChain1)
	sponge.Absorb(c.PubHashChain2)

	// 4. EC commitment points (Hyrax input commitments)
	sponge.Absorb(c.CommitPointX0)
	sponge.Absorb(c.CommitPointY0)
	sponge.Absorb(c.CommitPointX1)
	sponge.Absorb(c.CommitPointY1)

	// 5. EC commitment hash chain
	sponge.Absorb(c.EcHashChain1)
	sponge.Absorb(c.EcHashChain2)

	// 6. Squeeze and assert the challenge matches
	computedChallenge := sponge.Squeeze()
	api.AssertIsEqual(computedChallenge, c.Challenge)

	return nil
}
