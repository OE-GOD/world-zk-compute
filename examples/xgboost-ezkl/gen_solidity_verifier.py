"""Generate a Solidity verifier contract from EZKL proving artifacts."""

import asyncio
import os

import ezkl

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))

SETTINGS_PATH = os.path.join(SCRIPT_DIR, "settings.json")
VK_PATH = os.path.join(SCRIPT_DIR, "vk.key")
SRS_PATH = os.path.join(SCRIPT_DIR, "kzg.srs")
SOL_PATH = os.path.join(SCRIPT_DIR, "Verifier.sol")
ABI_PATH = os.path.join(SCRIPT_DIR, "Verifier.abi")


async def main():
    for path, label in [(SETTINGS_PATH, "settings.json"), (VK_PATH, "vk.key"), (SRS_PATH, "kzg.srs")]:
        if not os.path.exists(path):
            print(f"Missing {label}. Run prove.py first to generate proving artifacts.")
            return

    print("Generating Solidity verifier contract...")
    await ezkl.create_evm_verifier(VK_PATH, SETTINGS_PATH, SOL_PATH, ABI_PATH, SRS_PATH)

    if os.path.exists(SOL_PATH):
        size = os.path.getsize(SOL_PATH)
        print(f"Verifier contract saved to {SOL_PATH} ({size:,} bytes)")
        print("\nThis contract can be deployed on any EVM chain to verify EZKL proofs on-chain.")
        print("Comparable to risc0's Groth16 verifier router, but specific to this model's circuit.")
    else:
        print("Failed to generate verifier contract.")


if __name__ == "__main__":
    asyncio.run(main())
