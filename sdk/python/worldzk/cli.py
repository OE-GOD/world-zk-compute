"""
Command-line interface for the World ZK Compute Python SDK.

Provides quick access to TEEMLVerifier contract operations without writing
Python code.  Uses only ``argparse`` from the standard library.

Usage::

    worldzk --rpc-url http://localhost:8545 \\
            --private-key 0x... \\
            --contract 0x... \\
            submit-result --model-hash 0x... --input-hash 0x... \\
                          --result 0xdeadbeef --attestation 0x... --stake 100000000000000000

    worldzk --rpc-url http://localhost:8545 \\
            --contract 0x... \\
            status --result-id 0x...

    worldzk --rpc-url http://localhost:8545 \\
            --contract 0x... \\
            is-valid --result-id 0x...

    worldzk --rpc-url http://localhost:8545 \\
            --contract 0x... \\
            watch --from-block 0 --interval 5
"""

from __future__ import annotations

import argparse
import signal
import sys
import time
from typing import List, Optional, Sequence


def _build_parser() -> argparse.ArgumentParser:
    """Build the top-level argument parser with all subcommands."""
    parser = argparse.ArgumentParser(
        prog="worldzk",
        description="World ZK Compute CLI -- interact with TEEMLVerifier contracts",
    )

    # Global options
    parser.add_argument(
        "--rpc-url",
        required=False,
        default=None,
        help="JSON-RPC endpoint URL (e.g. http://localhost:8545)",
    )
    parser.add_argument(
        "--private-key",
        required=False,
        default=None,
        help="Hex-encoded private key for signing transactions",
    )
    parser.add_argument(
        "--contract",
        required=False,
        default=None,
        help="TEEMLVerifier contract address",
    )

    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # -- submit-result -------------------------------------------------------
    submit_parser = subparsers.add_parser(
        "submit-result",
        help="Submit a TEE-attested ML result",
    )
    submit_parser.add_argument(
        "--model-hash", required=True, help="Model hash (32 bytes, hex)"
    )
    submit_parser.add_argument(
        "--input-hash", required=True, help="Input hash (32 bytes, hex)"
    )
    submit_parser.add_argument(
        "--result", required=True, help="Result bytes (hex)"
    )
    submit_parser.add_argument(
        "--attestation", required=True, help="Attestation bytes (hex)"
    )
    submit_parser.add_argument(
        "--stake",
        type=int,
        default=0,
        help="Stake amount in wei (default: 0)",
    )

    # -- challenge -----------------------------------------------------------
    challenge_parser = subparsers.add_parser(
        "challenge",
        help="Challenge a submitted result",
    )
    challenge_parser.add_argument(
        "--result-id", required=True, help="Result ID (32 bytes, hex)"
    )
    challenge_parser.add_argument(
        "--bond",
        type=int,
        default=0,
        help="Bond amount in wei (default: 0)",
    )

    # -- finalize ------------------------------------------------------------
    finalize_parser = subparsers.add_parser(
        "finalize",
        help="Finalize a result after the dispute window",
    )
    finalize_parser.add_argument(
        "--result-id", required=True, help="Result ID (32 bytes, hex)"
    )

    # -- status --------------------------------------------------------------
    status_parser = subparsers.add_parser(
        "status",
        help="Show details of a submitted result",
    )
    status_parser.add_argument(
        "--result-id", required=True, help="Result ID (32 bytes, hex)"
    )

    # -- is-valid ------------------------------------------------------------
    is_valid_parser = subparsers.add_parser(
        "is-valid",
        help="Check whether a result is valid (prints true/false)",
    )
    is_valid_parser.add_argument(
        "--result-id", required=True, help="Result ID (32 bytes, hex)"
    )

    # -- watch ---------------------------------------------------------------
    watch_parser = subparsers.add_parser(
        "watch",
        help="Poll and print contract events",
    )
    watch_parser.add_argument(
        "--from-block",
        type=int,
        default=0,
        help="Starting block number (default: 0)",
    )
    watch_parser.add_argument(
        "--interval",
        type=float,
        default=2.0,
        help="Polling interval in seconds (default: 2.0)",
    )

    return parser


def _require_globals(
    args: argparse.Namespace, need_private_key: bool = False
) -> None:
    """Validate that required global options are present."""
    if not args.rpc_url:
        print("Error: --rpc-url is required", file=sys.stderr)
        sys.exit(1)
    if not args.contract:
        print("Error: --contract is required", file=sys.stderr)
        sys.exit(1)
    if need_private_key and not args.private_key:
        print("Error: --private-key is required for this command", file=sys.stderr)
        sys.exit(1)


def _make_verifier(args: argparse.Namespace):
    """Create a TEEVerifier from CLI arguments.

    Imports web3-dependent modules lazily so that ``--help`` works without
    web3 installed.
    """
    try:
        from worldzk.tee_verifier import TEEVerifier
    except ImportError:
        print(
            "Error: web3 is required for contract interaction. "
            "Install with: pip install worldzk[web3]",
            file=sys.stderr,
        )
        sys.exit(1)
    return TEEVerifier(
        rpc_url=args.rpc_url,
        private_key=args.private_key or "0x" + "00" * 32,
        contract_address=args.contract,
    )


def _make_watcher(args: argparse.Namespace):
    """Create a TEEEventWatcher from CLI arguments (lazy import)."""
    try:
        from worldzk.event_watcher import TEEEventWatcher
    except ImportError:
        print(
            "Error: web3 is required for event watching. "
            "Install with: pip install worldzk[web3]",
            file=sys.stderr,
        )
        sys.exit(1)
    return TEEEventWatcher(
        rpc_url=args.rpc_url,
        contract_address=args.contract,
    )


def _format_bytes(value: bytes) -> str:
    """Format bytes as a hex string with 0x prefix."""
    return "0x" + value.hex()


def _cmd_submit_result(args: argparse.Namespace) -> None:
    _require_globals(args, need_private_key=True)
    verifier = _make_verifier(args)
    result_id = verifier.submit_result(
        model_hash=args.model_hash,
        input_hash=args.input_hash,
        result=args.result,
        attestation=args.attestation,
        stake_wei=args.stake,
    )
    print(f"Result ID: {result_id}")


def _cmd_challenge(args: argparse.Namespace) -> None:
    _require_globals(args, need_private_key=True)
    verifier = _make_verifier(args)
    tx_hash = verifier.challenge(
        result_id=args.result_id,
        bond_wei=args.bond,
    )
    print(f"Challenge tx: {tx_hash}")


def _cmd_finalize(args: argparse.Namespace) -> None:
    _require_globals(args, need_private_key=True)
    verifier = _make_verifier(args)
    tx_hash = verifier.finalize(result_id=args.result_id)
    print(f"Finalize tx: {tx_hash}")


def _cmd_status(args: argparse.Namespace) -> None:
    _require_globals(args, need_private_key=False)
    verifier = _make_verifier(args)
    result = verifier.get_result(result_id=args.result_id)
    print(f"Submitter:          {result.submitter}")
    print(f"Enclave:            {result.enclave}")
    print(f"Model hash:         {_format_bytes(result.model_hash)}")
    print(f"Input hash:         {_format_bytes(result.input_hash)}")
    print(f"Result hash:        {_format_bytes(result.result_hash)}")
    print(f"Result:             {_format_bytes(result.result)}")
    print(f"Submitted at:       {result.submitted_at}")
    print(f"Challenge deadline: {result.challenge_deadline}")
    print(f"Dispute deadline:   {result.dispute_deadline}")
    print(f"Challenge bond:     {result.challenge_bond}")
    print(f"Prover stake:       {result.prover_stake_amount}")
    print(f"Finalized:          {result.finalized}")
    print(f"Challenged:         {result.challenged}")
    print(f"Challenger:         {result.challenger}")


def _cmd_is_valid(args: argparse.Namespace) -> None:
    _require_globals(args, need_private_key=False)
    verifier = _make_verifier(args)
    valid = verifier.is_result_valid(result_id=args.result_id)
    print("true" if valid else "false")


def _cmd_watch(args: argparse.Namespace) -> None:
    _require_globals(args, need_private_key=False)
    watcher = _make_watcher(args)

    from_block = args.from_block
    interval = args.interval

    # Handle Ctrl+C gracefully
    stop = False

    def _handle_signal(signum, frame):
        nonlocal stop
        stop = True

    signal.signal(signal.SIGINT, _handle_signal)
    signal.signal(signal.SIGTERM, _handle_signal)

    print(f"Watching events from block {from_block} (interval={interval}s)...")
    print("Press Ctrl+C to stop.\n")

    current_block = from_block
    while not stop:
        try:
            events, next_block = watcher.poll_events(
                from_block=current_block, to_block="latest"
            )
            for event in events:
                _print_event(event)
            current_block = next_block
        except Exception as exc:
            print(f"[poll error] {exc}", file=sys.stderr)
        time.sleep(interval)

    print("\nStopped.")


def _print_event(event) -> None:
    """Print a TEE event to stdout in a human-readable format."""
    name = getattr(event, "event_name", type(event).__name__)
    block = getattr(event, "block_number", "?")
    tx = getattr(event, "transaction_hash", b"")
    tx_str = _format_bytes(tx) if tx else "?"

    parts = [f"[block {block}] {name}"]

    if hasattr(event, "result_id"):
        parts.append(f"result_id={_format_bytes(event.result_id)}")
    if hasattr(event, "submitter"):
        parts.append(f"submitter={event.submitter}")
    if hasattr(event, "challenger"):
        parts.append(f"challenger={event.challenger}")
    if hasattr(event, "prover_won"):
        parts.append(f"prover_won={event.prover_won}")
    if hasattr(event, "enclave_key"):
        parts.append(f"enclave_key={event.enclave_key}")

    parts.append(f"tx={tx_str}")
    print("  ".join(parts))


_COMMAND_DISPATCH = {
    "submit-result": _cmd_submit_result,
    "challenge": _cmd_challenge,
    "finalize": _cmd_finalize,
    "status": _cmd_status,
    "is-valid": _cmd_is_valid,
    "watch": _cmd_watch,
}


def main(argv: Optional[Sequence[str]] = None) -> None:
    """Entry point for the ``worldzk`` CLI."""
    parser = _build_parser()
    args = parser.parse_args(argv)

    if args.command is None:
        parser.print_help()
        sys.exit(0)

    handler = _COMMAND_DISPATCH.get(args.command)
    if handler is None:
        parser.print_help()
        sys.exit(1)

    handler(args)


if __name__ == "__main__":
    main()
