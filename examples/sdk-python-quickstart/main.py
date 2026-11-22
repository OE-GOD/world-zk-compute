#!/usr/bin/env python3
"""
World ZK Compute — Python SDK Quickstart

Submits one TEE-attested result to a TEEMLVerifier deployed on a local Anvil
chain and reads it back. See README.md for the three setup steps (install the
SDK, start Anvil, deploy the contract).

Usage:
  python main.py
"""

import os

from eth_account import Account

from worldzk import (
    IndexerClient,
    NetworkError,
    TEEVerifier,
    compute_input_hash,
    compute_model_hash_from_bytes,
)

# ---------------------------------------------------------------------------
# Configuration (override via env vars)
# ---------------------------------------------------------------------------
RPC_URL = os.environ.get("RPC_URL", "http://127.0.0.1:8545")
# Proxy address printed by DeployTEEMLVerifier.s.sol on a fresh Anvil chain
# (0x5FbDB... is the implementation behind it and must not be used directly).
CONTRACT_ADDRESS = os.environ.get(
    "CONTRACT_ADDRESS", "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"
)
# Anvil default account #0: the deployer, and therefore the contract admin
PRIVATE_KEY = os.environ.get(
    "PRIVATE_KEY",
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
)
CHAIN_ID = int(os.environ.get("CHAIN_ID", "31337"))
INDEXER_URL = os.environ.get("INDEXER_URL", "http://127.0.0.1:8081")


def sign_attestation(enclave_key, model_hash, input_hash, result):
    """Sign a result the way TEEMLVerifier.submitResult checks it (EIP-712).

    In a real deployment the enclave produces this signature; here a local
    key stands in for it.
    """
    signed = Account.sign_typed_data(
        enclave_key,
        full_message={
            "types": {
                "EIP712Domain": [
                    {"name": "name", "type": "string"},
                    {"name": "version", "type": "string"},
                    {"name": "chainId", "type": "uint256"},
                    {"name": "verifyingContract", "type": "address"},
                ],
                "TEEMLResult": [
                    {"name": "modelHash", "type": "bytes32"},
                    {"name": "inputHash", "type": "bytes32"},
                    {"name": "resultHash", "type": "bytes32"},
                ],
            },
            "primaryType": "TEEMLResult",
            "domain": {
                "name": "TEEMLVerifier",
                "version": "1",
                "chainId": CHAIN_ID,
                "verifyingContract": CONTRACT_ADDRESS,
            },
            "message": {
                "modelHash": bytes.fromhex(model_hash[2:]),
                "inputHash": bytes.fromhex(input_hash[2:]),
                "resultHash": bytes.fromhex(compute_model_hash_from_bytes(result)[2:]),
            },
        },
    )
    return signed.signature


def main():
    # --- Create the on-chain client ---
    tee = TEEVerifier(
        rpc_url=RPC_URL,
        private_key=PRIVATE_KEY,
        contract_address=CONTRACT_ADDRESS,
    )
    print(f"TEEVerifier connected (account: {tee.account_address})")
    print(f"Contract admin: {tee.admin()}")

    # --- Register an enclave signing key (admin only) ---
    # A fresh local key stands in for the enclave, so the example can be re-run.
    enclave = Account.create()
    tee.register_enclave(enclave.address, compute_model_hash_from_bytes(b"enclave-image-v1"))
    print(f"Registered enclave key: {enclave.address}")

    # --- Compute hashes using SDK utilities ---
    model_hash = compute_model_hash_from_bytes(b"my-model-v1")
    input_hash = compute_input_hash([1.0, 2.5, 3.7])
    print(f"Model hash: {model_hash}")
    print(f"Input hash: {input_hash}")

    # --- Submit a TEE result ---
    result_data = b"\x00" * 32  # placeholder result bytes
    attestation = sign_attestation(enclave.key, model_hash, input_hash, result_data)
    stake_wei = tee.prover_stake_amount()  # 0.1 ETH by default

    result_id = tee.submit_result(
        model_hash=model_hash,
        input_hash=input_hash,
        result=result_data,
        attestation=attestation,
        stake_wei=stake_wei,
    )
    print(f"submitResult result ID: {result_id}")

    # --- Query result ---
    ml_result = tee.get_result(result_id)
    print(f"Submitter: {ml_result.submitter}")
    print(f"Enclave: {ml_result.enclave}")
    print(f"Finalized: {ml_result.finalized}")
    print(f"Challenged: {ml_result.challenged}")

    is_valid = tee.is_result_valid(result_id)
    print(f"Is valid (before finalize): {is_valid}")

    # --- Optional: read the same data through the TEE indexer ---
    try:
        with IndexerClient(INDEXER_URL, max_retries=1) as indexer:
            health = indexer.health()
            print(f"Indexer status: {health.status}, block: {health.last_indexed_block}")

            results = indexer.list_results(limit=5)
            print(f"Found {len(results)} indexed results")
            for r in results:
                print(f"  {r.id}: {r.status.value}")

            stats = indexer.stats()
            print(f"Total submitted: {stats.total_submitted}")
            print(f"Total finalized: {stats.total_finalized}")
    except NetworkError:
        print(f"Indexer not reachable at {INDEXER_URL} (optional, skipped)")

    print("\nQuickstart complete!")
    print("To finalize, wait for the challenge window (1 hour), then call tee.finalize(result_id).")


if __name__ == "__main__":
    main()
