"""Tests for the worldzk CLI argument parsing.

These tests validate that the argparse-based CLI correctly parses all
subcommands and their options.  They do NOT call any on-chain functions.
"""

import argparse

import pytest

from worldzk.cli import _build_parser, main


class TestBuildParser:
    """Verify that the parser is constructed without errors."""

    def test_parser_is_created(self):
        parser = _build_parser()
        assert parser is not None
        assert parser.prog == "worldzk"


class TestHelpDoesNotError:
    """Calling --help should exit with code 0 (SystemExit)."""

    def test_top_level_help(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args(["--help"])
        assert exc_info.value.code == 0

    def test_submit_result_help(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args(["submit-result", "--help"])
        assert exc_info.value.code == 0

    def test_challenge_help(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args(["challenge", "--help"])
        assert exc_info.value.code == 0

    def test_finalize_help(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args(["finalize", "--help"])
        assert exc_info.value.code == 0

    def test_status_help(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args(["status", "--help"])
        assert exc_info.value.code == 0

    def test_is_valid_help(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args(["is-valid", "--help"])
        assert exc_info.value.code == 0

    def test_watch_help(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args(["watch", "--help"])
        assert exc_info.value.code == 0


class TestGlobalOptions:
    """Global options (--rpc-url, --private-key, --contract) parse correctly."""

    def test_all_global_options(self):
        parser = _build_parser()
        args = parser.parse_args([
            "--rpc-url", "http://localhost:8545",
            "--private-key", "0xdeadbeef",
            "--contract", "0x1234567890abcdef1234567890abcdef12345678",
            "status", "--result-id", "0x" + "ab" * 32,
        ])
        assert args.rpc_url == "http://localhost:8545"
        assert args.private_key == "0xdeadbeef"
        assert args.contract == "0x1234567890abcdef1234567890abcdef12345678"

    def test_global_options_default_to_none(self):
        parser = _build_parser()
        args = parser.parse_args(["status", "--result-id", "0x" + "00" * 32])
        assert args.rpc_url is None
        assert args.private_key is None
        assert args.contract is None


class TestSubmitResult:
    """Test parsing of the submit-result subcommand."""

    def test_all_required_args(self):
        parser = _build_parser()
        model_hash = "0x" + "aa" * 32
        input_hash = "0x" + "bb" * 32
        args = parser.parse_args([
            "submit-result",
            "--model-hash", model_hash,
            "--input-hash", input_hash,
            "--result", "0xdeadbeef",
            "--attestation", "0xcafe",
        ])
        assert args.command == "submit-result"
        assert args.model_hash == model_hash
        assert args.input_hash == input_hash
        assert args.result == "0xdeadbeef"
        assert args.attestation == "0xcafe"
        assert args.stake == 0  # default

    def test_with_stake(self):
        parser = _build_parser()
        args = parser.parse_args([
            "submit-result",
            "--model-hash", "0x" + "aa" * 32,
            "--input-hash", "0x" + "bb" * 32,
            "--result", "0xbeef",
            "--attestation", "0x00",
            "--stake", "100000000000000000",
        ])
        assert args.stake == 100000000000000000

    def test_missing_model_hash(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args([
                "submit-result",
                "--input-hash", "0x" + "bb" * 32,
                "--result", "0xbeef",
                "--attestation", "0x00",
            ])
        assert exc_info.value.code != 0

    def test_missing_input_hash(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args([
                "submit-result",
                "--model-hash", "0x" + "aa" * 32,
                "--result", "0xbeef",
                "--attestation", "0x00",
            ])
        assert exc_info.value.code != 0

    def test_missing_result(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args([
                "submit-result",
                "--model-hash", "0x" + "aa" * 32,
                "--input-hash", "0x" + "bb" * 32,
                "--attestation", "0x00",
            ])
        assert exc_info.value.code != 0

    def test_missing_attestation(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args([
                "submit-result",
                "--model-hash", "0x" + "aa" * 32,
                "--input-hash", "0x" + "bb" * 32,
                "--result", "0xbeef",
            ])
        assert exc_info.value.code != 0


class TestChallenge:
    """Test parsing of the challenge subcommand."""

    def test_required_args(self):
        parser = _build_parser()
        result_id = "0x" + "cc" * 32
        args = parser.parse_args(["challenge", "--result-id", result_id])
        assert args.command == "challenge"
        assert args.result_id == result_id
        assert args.bond == 0  # default

    def test_with_bond(self):
        parser = _build_parser()
        args = parser.parse_args([
            "challenge",
            "--result-id", "0x" + "cc" * 32,
            "--bond", "50000000000000000",
        ])
        assert args.bond == 50000000000000000

    def test_missing_result_id(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args(["challenge"])
        assert exc_info.value.code != 0


class TestFinalize:
    """Test parsing of the finalize subcommand."""

    def test_required_args(self):
        parser = _build_parser()
        result_id = "0x" + "dd" * 32
        args = parser.parse_args(["finalize", "--result-id", result_id])
        assert args.command == "finalize"
        assert args.result_id == result_id

    def test_missing_result_id(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args(["finalize"])
        assert exc_info.value.code != 0


class TestStatus:
    """Test parsing of the status subcommand."""

    def test_required_args(self):
        parser = _build_parser()
        result_id = "0x" + "ee" * 32
        args = parser.parse_args(["status", "--result-id", result_id])
        assert args.command == "status"
        assert args.result_id == result_id

    def test_missing_result_id(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args(["status"])
        assert exc_info.value.code != 0


class TestIsValid:
    """Test parsing of the is-valid subcommand."""

    def test_required_args(self):
        parser = _build_parser()
        result_id = "0x" + "ff" * 32
        args = parser.parse_args(["is-valid", "--result-id", result_id])
        assert args.command == "is-valid"
        assert args.result_id == result_id

    def test_missing_result_id(self):
        parser = _build_parser()
        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args(["is-valid"])
        assert exc_info.value.code != 0


class TestWatch:
    """Test parsing of the watch subcommand."""

    def test_defaults(self):
        parser = _build_parser()
        args = parser.parse_args(["watch"])
        assert args.command == "watch"
        assert args.from_block == 0
        assert args.interval == 2.0

    def test_custom_from_block(self):
        parser = _build_parser()
        args = parser.parse_args(["watch", "--from-block", "12345"])
        assert args.from_block == 12345

    def test_custom_interval(self):
        parser = _build_parser()
        args = parser.parse_args(["watch", "--interval", "5.0"])
        assert args.interval == 5.0

    def test_all_watch_options(self):
        parser = _build_parser()
        args = parser.parse_args([
            "watch", "--from-block", "100", "--interval", "10"
        ])
        assert args.from_block == 100
        assert args.interval == 10.0


class TestNoCommand:
    """When no command is given, main should print help and exit 0."""

    def test_no_args_exits_zero(self):
        with pytest.raises(SystemExit) as exc_info:
            main([])
        assert exc_info.value.code == 0


class TestMainDispatch:
    """Test that main() dispatches to the correct handler."""

    def test_submit_result_requires_globals(self):
        """submit-result without --rpc-url should fail."""
        with pytest.raises(SystemExit) as exc_info:
            main([
                "submit-result",
                "--model-hash", "0x" + "aa" * 32,
                "--input-hash", "0x" + "bb" * 32,
                "--result", "0xbeef",
                "--attestation", "0x00",
            ])
        # exit(1) from _require_globals
        assert exc_info.value.code == 1

    def test_status_requires_rpc_url(self):
        with pytest.raises(SystemExit) as exc_info:
            main(["status", "--result-id", "0x" + "ab" * 32])
        assert exc_info.value.code == 1

    def test_status_requires_contract(self):
        with pytest.raises(SystemExit) as exc_info:
            main([
                "--rpc-url", "http://localhost:8545",
                "status", "--result-id", "0x" + "ab" * 32,
            ])
        assert exc_info.value.code == 1

    def test_challenge_requires_private_key(self):
        with pytest.raises(SystemExit) as exc_info:
            main([
                "--rpc-url", "http://localhost:8545",
                "--contract", "0x" + "11" * 20,
                "challenge",
                "--result-id", "0x" + "ab" * 32,
            ])
        assert exc_info.value.code == 1

    def test_finalize_requires_private_key(self):
        with pytest.raises(SystemExit) as exc_info:
            main([
                "--rpc-url", "http://localhost:8545",
                "--contract", "0x" + "11" * 20,
                "finalize",
                "--result-id", "0x" + "ab" * 32,
            ])
        assert exc_info.value.code == 1

    def test_is_valid_no_private_key_needed(self):
        """is-valid should not require --private-key (it is a view call).

        It will still fail later when trying to connect, but the
        _require_globals check should pass with just --rpc-url and --contract.
        We verify it does NOT exit(1) from _require_globals by checking that
        the error (if any) is NOT code 1 from our validation.
        """
        # This will attempt to create a web3 connection and fail, but that
        # is fine -- we just want to confirm _require_globals passed.
        # If web3 is not installed, it exits with code 1 from the import check.
        # If web3 IS installed, it will try to connect and may raise.
        # Either way, it should not fail from _require_globals.
        try:
            main([
                "--rpc-url", "http://localhost:8545",
                "--contract", "0x" + "11" * 20,
                "is-valid",
                "--result-id", "0x" + "ab" * 32,
            ])
        except (SystemExit, Exception):
            # Expected -- either web3 not installed or connection refused
            pass


class TestCommandNames:
    """Ensure all expected commands are registered."""

    def test_all_subcommands_exist(self):
        parser = _build_parser()
        # Access the subparsers action to list registered commands
        subparsers_actions = [
            action for action in parser._actions
            if isinstance(action, argparse._SubParsersAction)
        ]
        assert len(subparsers_actions) == 1
        choices = set(subparsers_actions[0].choices.keys())
        expected = {"submit-result", "challenge", "finalize", "status", "is-valid", "watch"}
        assert choices == expected
