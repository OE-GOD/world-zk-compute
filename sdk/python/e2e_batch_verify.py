#!/usr/bin/env python3
"""CLI for on-chain DAG batch verification E2E testing.

Usage::

    python e2e_batch_verify.py \
        --rpc-url http://127.0.0.1:8545 \
        --private-key 0x... \
        --contract 0x... \
        --fixture ../../contracts/test/fixtures/phase1a_dag_fixture.json
"""

import argparse
import json
import sys
import time

try:
    from worldzk import BatchVerifier
except ImportError:
    print("Error: web3 dependency required. Install with: pip install worldzk[web3]")
    sys.exit(1)


def main() -> None:
    parser = argparse.ArgumentParser(description="DAG batch verification E2E")
    parser.add_argument("--rpc-url", required=True, help="JSON-RPC endpoint")
    parser.add_argument("--private-key", required=True, help="Deployer private key")
    parser.add_argument("--contract", required=True, help="RemainderVerifier address")
    parser.add_argument("--fixture", required=True, help="Path to DAG fixture JSON")
    parser.add_argument("--gas-limit", type=int, default=None, help="Gas limit per tx")
    parser.add_argument(
        "--skip-cleanup", action="store_true", help="Skip cleanup transaction"
    )
    args = parser.parse_args()

    # Load fixture
    with open(args.fixture) as f:
        fixture_data = json.load(f)
    fixture_input = BatchVerifier.load_fixture(fixture_data)

    # Estimate transactions
    num_compute = fixture_data.get("dag_circuit_description", {}).get(
        "numComputeLayers", 88
    )
    num_groups = fixture_data.get("config", {}).get("num_input_groups", 34)
    estimate = BatchVerifier.estimate_transaction_count(num_compute, num_groups)
    print(
        f"Estimated transactions: {estimate['total']} "
        f"(1 start + {estimate['continue']} continue + {estimate['finalize']} finalize)"
    )

    # Build verifier
    kwargs: dict = dict(
        rpc_url=args.rpc_url,
        private_key=args.private_key,
        contract_address=args.contract,
    )
    if args.gas_limit is not None:
        kwargs["gas_limit"] = args.gas_limit
    verifier = BatchVerifier(**kwargs)

    # Progress callback
    def on_progress(event):
        gas = f" | gas: {event.gas_used:,}" if event.gas_used else ""
        tx = f" | tx: {event.tx_hash[:10]}..." if event.tx_hash else ""
        step = f"{event.step}/{event.total_steps}"
        print(f"[{event.phase}] step {step}{gas}{tx}")

    # Run
    start = time.monotonic()
    result = verifier.verify_batch(
        fixture_input, on_progress=on_progress, skip_cleanup=args.skip_cleanup
    )
    duration = time.monotonic() - start

    tx_count = (
        1
        + len(result.continue_steps)
        + len(result.finalize_steps)
        + (1 if result.cleanup_step else 0)
    )

    print()
    print("--- Batch Verification Complete ---")
    print(f"Session ID:     {result.session_id}")
    print(f"Total gas used: {result.total_gas_used:,}")
    print(f"Duration:       {duration:.1f}s")
    print(f"Transactions:   {tx_count}")


if __name__ == "__main__":
    main()
