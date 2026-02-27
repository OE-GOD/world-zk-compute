package main

import (
	"fmt"
	"os"

	"github.com/consensys/gnark-crypto/ecc"
	"github.com/consensys/gnark/backend/groth16"
	"github.com/consensys/gnark/frontend"
	"github.com/consensys/gnark/frontend/cs/r1cs"
)

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
	fmt.Println("  prove           Generate a Groth16 proof for a given witness")
	fmt.Println("  export-solidity Export Solidity verifier contract")
	fmt.Println("  info            Show circuit info (constraints, etc.)")
}

// cmdSetup compiles the circuit and runs Groth16 trusted setup.
func cmdSetup() {
	fmt.Println("Compiling circuit...")
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, &RemainderWrapperCircuit{})
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("Circuit compiled: %d constraints\n", ccs.GetNbConstraints())

	fmt.Println("Running Groth16 setup...")
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

	fmt.Println("Setup complete. Keys saved to proving_key.bin and verification_key.bin")
}

// cmdProve generates a Groth16 proof (placeholder — needs witness data).
func cmdProve() {
	fmt.Println("Proof generation not yet implemented.")
	fmt.Println("This will read a witness JSON file and generate a Groth16 proof.")
	os.Exit(0)
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

	fmt.Println("Solidity verifier exported to RemainderGroth16Verifier.sol")
}

// cmdInfo shows circuit statistics.
func cmdInfo() {
	fmt.Println("Compiling circuit...")
	ccs, err := frontend.Compile(ecc.BN254.ScalarField(), r1cs.NewBuilder, &RemainderWrapperCircuit{})
	if err != nil {
		fmt.Fprintf(os.Stderr, "compile error: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("Constraints: %d\n", ccs.GetNbConstraints())
	fmt.Printf("Public inputs: 5 (circuitHash[2], publicInputs[2], challenge)\n")
	fmt.Printf("Private witness: 8 (hashChains[4], commitPoints[4])\n")
}
