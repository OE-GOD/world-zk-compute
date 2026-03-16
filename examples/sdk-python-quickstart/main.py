#!/usr/bin/env python3
"""
World ZK Compute — Python SDK Quickstart

Prerequisites:
  1. pip install worldzk[web3]
  2. Start Anvil: anvil --block-time 1
  3. Deploy TEEMLVerifier contract and set CONTRACT_ADDRESS below

Usage:
  python main.py
"""

import os
import sys

from worldzk import (
    Client,
    TEEVerifier,
    compute_model_hash,
    compute_input_hash,
)

# ---------------------------------------------------------------------------
# Configuration (override via env vars)
# ---------------------------------------------------------------------------
RPC_URL = os.environ.get("RPC_URL", "http://127.0.0.1:8545")
CONTRACT_ADDRESS = os.environ.get(
    "CONTRACT_ADDRESS", "0x5FbDB2315678afecb367f032d93F642f64180aa3"
)
# Anvil default account #0
PRIVATE_KEY = os.environ.get(
    "PRIVATE_KEY",
    "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
)
INDEXER_URL = os.environ.get("INDEXER_URL", "http://127.0.0.1:8081")


def main():
    # --- Create SDK clients ---
    tee = TEEVerifier(
        rpc_url=RPC_URL,
        private_key=PRIVATE_KEY,
        contract_address=CONTRACT_ADDRESS,
    )
    print(f"TEEVerifier connected (account: {tee.account_address})")

    client = Client(base_url=INDEXER_URL)

    # --- Compute hashes using SDK utilities ---
    model_hash = compute_model_hash(b"my-model-v1")
    input_hash = compute_input_hash(b"sample-input-data")
    print(f"Model hash: {model_hash}")
    print(f"Input hash: {input_hash}")

    # --- Submit a TEE result ---
    result_data = b"\x00" * 32  # mock result
    attestation = b"\x00" * 65  # mock attestation
    stake_wei = 100_000_000_000_000_000  # 0.1 ETH

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
    print(f"Finalized: {ml_result.finalized}")
    print(f"Challenged: {ml_result.challenged}")

    is_valid = tee.is_result_valid(result_id)
    print(f"Is valid (before finalize): {is_valid}")

    # --- Check indexer health ---
    try:
        health = client.health()
        print(f"Indexer status: {health.status}, block: {health.last_indexed_block}")
    except Exception:
        print("Indexer not available (expected if not running)")

    # --- List results via indexer ---
    try:
        results = client.list_results(limit=5)
        print(f"Found {len(results)} indexed results")
        for r in results:
            print(f"  {r.id}: {r.status}")
    except Exception:
        print("Indexer not available (expected if not running)")

    # --- Get statistics ---
    try:
        stats = client.stats()
        print(f"Total submitted: {stats.total_submitted}")
        print(f"Total finalized: {stats.total_finalized}")
    except Exception:
        print("Indexer not available (expected if not running)")

    print("\nQuickstart complete!")
    print("To finalize, wait for the challenge window, then call tee.finalize().")


if __name__ == "__main__":
    main()
