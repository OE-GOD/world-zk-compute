#!/usr/bin/env python3
"""
World ZK Compute — Python SDK Quickstart

Prerequisites:
  1. pip install web3 eth-account
  2. Start Anvil: anvil --block-time 1
  3. Deploy TEEMLVerifier contract and set CONTRACT_ADDRESS below

Usage:
  python main.py
"""

import os
import sys
import time

from eth_account import Account
from web3 import Web3

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

# Minimal ABI for TEEMLVerifier (submit, finalize, query)
TEE_ABI = [
    {
        "type": "function",
        "name": "submitResult",
        "inputs": [
            {"name": "modelHash", "type": "bytes32"},
            {"name": "inputHash", "type": "bytes32"},
            {"name": "result", "type": "bytes32"},
            {"name": "imageHash", "type": "bytes32"},
            {"name": "attestation", "type": "bytes"},
        ],
        "outputs": [],
        "stateMutability": "payable",
    },
    {
        "type": "function",
        "name": "finalizeResult",
        "inputs": [{"name": "resultId", "type": "bytes32"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "isResultValid",
        "inputs": [{"name": "resultId", "type": "bytes32"}],
        "outputs": [{"name": "", "type": "bool"}],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "proverStake",
        "inputs": [],
        "outputs": [{"name": "", "type": "uint256"}],
        "stateMutability": "view",
    },
    {
        "type": "event",
        "name": "ResultSubmitted",
        "inputs": [
            {"name": "resultId", "type": "bytes32", "indexed": True},
            {"name": "modelHash", "type": "bytes32", "indexed": False},
            {"name": "inputHash", "type": "bytes32", "indexed": False},
        ],
        "anonymous": False,
    },
]


def main():
    w3 = Web3(Web3.HTTPProvider(RPC_URL))
    if not w3.is_connected():
        print(f"Cannot connect to {RPC_URL}")
        sys.exit(1)
    print(f"Connected to {RPC_URL} (chain {w3.eth.chain_id})")

    account: Account = Account.from_key(PRIVATE_KEY)
    contract = w3.eth.contract(
        address=Web3.to_checksum_address(CONTRACT_ADDRESS), abi=TEE_ABI
    )

    # --- Submit a TEE result ---
    model_hash = b"\x00" * 31 + b"\x01"
    input_hash = b"\x00" * 31 + b"\x02"
    result = b"\x00" * 31 + b"\x03"
    image_hash = b"\x00" * 31 + b"\x04"
    attestation = b"\x00" * 65  # mock attestation

    stake = contract.functions.proverStake().call()
    print(f"Required stake: {stake} wei")

    tx = contract.functions.submitResult(
        model_hash, input_hash, result, image_hash, attestation
    ).build_transaction(
        {
            "from": account.address,
            "value": stake,
            "nonce": w3.eth.get_transaction_count(account.address),
            "gas": 500_000,
        }
    )
    signed = account.sign_transaction(tx)
    tx_hash = w3.eth.send_raw_transaction(signed.raw_transaction)
    receipt = w3.eth.wait_for_transaction_receipt(tx_hash)
    print(f"submitResult tx: {receipt['transactionHash'].hex()} (status={receipt['status']})")

    # Compute result ID
    result_id = Web3.solidity_keccak(
        ["bytes32", "bytes32"], [model_hash, input_hash]
    )
    print(f"Result ID: {result_id.hex()}")

    # --- Query result ---
    is_valid = contract.functions.isResultValid(result_id).call()
    print(f"Is valid (before finalize): {is_valid}")

    print("\nResult submitted successfully!")
    print("To finalize, wait for the challenge window to pass, then call finalizeResult().")


if __name__ == "__main__":
    main()
