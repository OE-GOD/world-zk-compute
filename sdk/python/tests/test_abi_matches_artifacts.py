"""Compare every ABI the SDK declares with the compiled contracts.

The SDK's ABIs are written by hand. An entry whose input types differ from the
contract's has a different selector (function) or topic (event): the call
reverts, or the event is never matched. The Foundry artifacts in
``contracts/out`` are the reference (``cd contracts && forge build``); the tests
are skipped when they are not there.
"""

from __future__ import annotations

import json
import os

import pytest

from worldzk.abi import DAG_BATCH_VERIFIER_ABI
from worldzk.execution_engine import EXECUTION_ENGINE_ABI
from worldzk.prover_registry import PROVER_REGISTRY_ABI
from worldzk.tee_verifier import TEE_ML_VERIFIER_ABI

_OUT_DIR = os.path.abspath(
    os.path.join(os.path.dirname(__file__), "..", "..", "..", "contracts", "out")
)

_CASES = [
    ("RemainderVerifier.sol/RemainderVerifier.json", DAG_BATCH_VERIFIER_ABI),
    ("ExecutionEngine.sol/ExecutionEngine.json", EXECUTION_ENGINE_ABI),
    ("ProverRegistry.sol/ProverRegistry.json", PROVER_REGISTRY_ABI),
    ("TEEMLVerifier.sol/TEEMLVerifier.json", TEE_ML_VERIFIER_ABI),
]

_KINDS = ("function", "event", "error")


def _type(param: dict) -> str:
    """Canonical type of an ABI parameter, with tuples expanded."""
    ty = param["type"]
    if ty.startswith("tuple"):
        inner = ",".join(_type(c) for c in param.get("components", []))
        return f"({inner}){ty[len('tuple'):]}"
    return ty


def _signature(entry: dict) -> tuple:
    """Kind, name and input types: what the selector or topic is computed from."""
    return (
        entry["type"],
        entry.get("name", ""),
        tuple(_type(i) for i in entry.get("inputs", [])),
    )


def _details(entry: dict) -> dict:
    """What has to agree besides the signature for calls and decoding to work."""
    if entry["type"] == "function":
        return {
            "outputs": [_type(o) for o in entry.get("outputs", [])],
            "stateMutability": entry.get("stateMutability"),
        }
    if entry["type"] == "event":
        return {
            "indexed": [bool(i.get("indexed", False)) for i in entry.get("inputs", [])],
            "anonymous": bool(entry.get("anonymous", False)),
        }
    return {}


@pytest.mark.parametrize(
    ("artifact", "sdk_abi"), _CASES, ids=[case[0].split("/")[0] for case in _CASES]
)
def test_sdk_abi_matches_artifact(artifact: str, sdk_abi: list[dict]) -> None:
    path = os.path.join(_OUT_DIR, artifact)
    if not os.path.exists(path):
        pytest.skip(f"{path} not found; run `forge build` in contracts/")
    with open(path) as f:
        compiled = {_signature(e): e for e in json.load(f)["abi"] if e["type"] in _KINDS}

    problems = []
    for entry in sdk_abi:
        if entry["type"] not in _KINDS:
            continue
        kind, name, inputs = signature = _signature(entry)
        if signature not in compiled:
            problems.append(f"{kind} {name}({','.join(inputs)}) is not in the contract")
            continue
        declared, real = _details(entry), _details(compiled[signature])
        if declared.get("stateMutability") is None:
            # Not every hand-written entry states it; compare it only when given.
            real.pop("stateMutability", None)
            declared.pop("stateMutability", None)
        if declared != real:
            problems.append(f"{kind} {name}: SDK declares {declared}, contract has {real}")
    assert not problems, "\n".join(problems)
