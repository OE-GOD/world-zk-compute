"""
Async TEE ML Verifier client and event watcher for on-chain TEE attestation.

Provides ``AsyncTEEVerifier`` and ``AsyncEventWatcher`` classes that mirror
their synchronous counterparts (``TEEVerifier`` and ``TEEEventWatcher``) but
use ``async``/``await`` with web3.py's ``AsyncWeb3`` and ``AsyncHTTPProvider``.

Requires the ``web3`` optional dependency: ``pip install worldzk[web3]``

Example usage::

    import asyncio
    from worldzk import AsyncTEEVerifier

    async def main():
        verifier = AsyncTEEVerifier(
            rpc_url="http://localhost:8545",
            contract_address="0x...",
            private_key="0x...",
        )
        valid = await verifier.is_result_valid(b"\\x00" * 32)
        print(f"Valid: {valid}")
        await verifier.close()

    asyncio.run(main())
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from typing import Any, Callable, List, Optional, Sequence, Union

from eth_account import Account
from eth_account.signers.local import LocalAccount
from web3 import AsyncWeb3
from web3.providers import AsyncHTTPProvider

from .tee_verifier import TEE_ML_VERIFIER_ABI, MLResult, _to_bytes, _to_bytes32
from .event_watcher import TEEEvent, parse_log, _EVENT_SIGNATURES, topic_hash


class AsyncTEEVerifier:
    """Async client for the TEEMLVerifier smart contract.

    Mirrors :class:`TEEVerifier` but all state-changing and query methods are
    coroutines that can be ``await``-ed.
    """

    def __init__(
        self,
        rpc_url: str,
        contract_address: str,
        private_key: str | None = None,
        gas_limit: int = 500_000,
    ):
        """Create an async TEE verifier.

        Args:
            rpc_url: JSON-RPC endpoint URL.
            contract_address: Deployed TEEMLVerifier contract address.
            private_key: Hex-encoded private key for signing (optional for read-only).
            gas_limit: Maximum gas per transaction (default 500000).
        """
        self._w3 = AsyncWeb3(AsyncHTTPProvider(rpc_url))
        self._account: LocalAccount | None = (
            Account.from_key(private_key) if private_key else None
        )
        self._contract = self._w3.eth.contract(
            address=AsyncWeb3.to_checksum_address(contract_address),
            abi=TEE_ML_VERIFIER_ABI,
        )
        self._gas_limit = gas_limit

    @property
    def address(self) -> str:
        """The contract address."""
        return self._contract.address

    @property
    def account_address(self) -> str | None:
        """The signer's address, or ``None`` if no private key was provided."""
        return self._account.address if self._account else None

    # -- Write methods -------------------------------------------------------

    async def submit_result(
        self,
        model_hash: bytes | str,
        input_hash: bytes | str,
        result: bytes | str,
        attestation: bytes | str,
        stake_wei: int = 0,
    ) -> str:
        """Submit a TEE-attested result. Returns the result ID as hex."""
        receipt = await self._send_tx(
            self._contract.functions.submitResult(
                _to_bytes32(model_hash),
                _to_bytes32(input_hash),
                _to_bytes(result),
                _to_bytes(attestation),
            ),
            value=stake_wei,
        )
        logs = self._contract.events.ResultSubmitted().process_receipt(receipt)
        if logs:
            rid = logs[0]["args"]["resultId"]
            if isinstance(rid, bytes):
                return "0x" + rid.hex()
            return str(rid)
        return self._tx_hash_hex(receipt)

    async def challenge_result(
        self, result_id: bytes | str, bond_wei: int = 0
    ) -> str:
        """Challenge a result. Returns tx hash."""
        receipt = await self._send_tx(
            self._contract.functions.challenge(_to_bytes32(result_id)),
            value=bond_wei,
        )
        return self._tx_hash_hex(receipt)

    async def finalize_result(self, result_id: bytes | str) -> str:
        """Finalize a result after challenge window. Returns tx hash."""
        receipt = await self._send_tx(
            self._contract.functions.finalize(_to_bytes32(result_id)),
        )
        return self._tx_hash_hex(receipt)

    async def register_enclave(self, enclave_key: str, image_hash: bytes | str) -> str:
        """Register an enclave signing key. Returns tx hash."""
        receipt = await self._send_tx(
            self._contract.functions.registerEnclave(
                AsyncWeb3.to_checksum_address(enclave_key),
                _to_bytes32(image_hash),
            )
        )
        return self._tx_hash_hex(receipt)

    async def revoke_enclave(self, enclave_key: str) -> str:
        """Revoke an enclave. Returns tx hash."""
        receipt = await self._send_tx(
            self._contract.functions.revokeEnclave(
                AsyncWeb3.to_checksum_address(enclave_key),
            )
        )
        return self._tx_hash_hex(receipt)

    async def resolve_dispute(
        self,
        result_id: bytes | str,
        proof: bytes | str,
        circuit_hash: bytes | str,
        public_inputs: bytes | str,
        gens_data: bytes | str,
    ) -> str:
        """Resolve a dispute with a ZK proof. Returns tx hash."""
        receipt = await self._send_tx(
            self._contract.functions.resolveDispute(
                _to_bytes32(result_id),
                _to_bytes(proof),
                _to_bytes32(circuit_hash),
                _to_bytes(public_inputs),
                _to_bytes(gens_data),
            )
        )
        return self._tx_hash_hex(receipt)

    async def resolve_dispute_by_timeout(self, result_id: bytes | str) -> str:
        """Resolve dispute in challenger's favor after timeout. Returns tx hash."""
        receipt = await self._send_tx(
            self._contract.functions.resolveDisputeByTimeout(
                _to_bytes32(result_id)
            ),
        )
        return self._tx_hash_hex(receipt)

    async def owner(self) -> str:
        """Get the contract owner address."""
        return await self._contract.functions.owner().call()

    async def pending_owner(self) -> str:
        """Get the pending owner address (for 2-step transfer)."""
        return await self._contract.functions.pendingOwner().call()

    async def transfer_ownership(self, new_owner: str) -> str:
        """Initiate ownership transfer (2-step). Returns tx hash."""
        receipt = await self._send_tx(
            self._contract.functions.transferOwnership(
                AsyncWeb3.to_checksum_address(new_owner),
            )
        )
        return self._tx_hash_hex(receipt)

    async def accept_ownership(self) -> str:
        """Accept pending ownership transfer. Returns tx hash."""
        receipt = await self._send_tx(
            self._contract.functions.acceptOwnership()
        )
        return self._tx_hash_hex(receipt)

    async def pause(self) -> str:
        """Pause the contract. Returns tx hash."""
        receipt = await self._send_tx(
            self._contract.functions.pause()
        )
        return self._tx_hash_hex(receipt)

    async def unpause(self) -> str:
        """Unpause the contract. Returns tx hash."""
        receipt = await self._send_tx(
            self._contract.functions.unpause()
        )
        return self._tx_hash_hex(receipt)

    async def paused(self) -> bool:
        """Check if the contract is paused."""
        return await self._contract.functions.paused().call()

    async def challenge_bond_amount(self) -> int:
        """Get the minimum challenge bond amount."""
        return await self._contract.functions.challengeBondAmount().call()

    async def prover_stake_amount(self) -> int:
        """Get the minimum prover stake amount."""
        return await self._contract.functions.proverStake().call()

    async def dispute_resolved(self, result_id: bytes | str) -> bool:
        """Check if a dispute has been resolved."""
        return await self._contract.functions.disputeResolved(
            _to_bytes32(result_id)
        ).call()

    async def dispute_prover_won(self, result_id: bytes | str) -> bool:
        """Check if the prover won the dispute."""
        return await self._contract.functions.disputeProverWon(
            _to_bytes32(result_id)
        ).call()

    # -- Read methods --------------------------------------------------------

    async def get_result(self, result_id: bytes | str) -> MLResult:
        """Fetch an on-chain result."""
        raw = await self._contract.functions.getResult(
            _to_bytes32(result_id)
        ).call()
        return MLResult(
            enclave=raw[0],
            submitter=raw[1],
            model_hash=raw[2],
            input_hash=raw[3],
            result_hash=raw[4],
            result=raw[5],
            submitted_at=raw[6],
            challenge_deadline=raw[7],
            dispute_deadline=raw[8],
            challenge_bond=raw[9],
            prover_stake_amount=raw[10],
            finalized=raw[11],
            challenged=raw[12],
            challenger=raw[13],
        )

    async def is_result_valid(self, result_id: bytes | str) -> bool:
        """Check if a result is valid (finalized or dispute resolved in prover's favor)."""
        return await self._contract.functions.isResultValid(
            _to_bytes32(result_id)
        ).call()

    # -- Cleanup -------------------------------------------------------------

    async def close(self) -> None:
        """Close the underlying async HTTP provider session."""
        provider = self._w3.provider
        if hasattr(provider, "_session") and provider._session is not None:
            await provider._session.close()

    # -- Internal helpers ----------------------------------------------------

    async def _send_tx(self, fn: Any, value: int = 0) -> Any:
        """Build, sign, and send a transaction."""
        if self._account is None:
            raise RuntimeError(
                "Cannot send transactions without a private key. "
                "Pass private_key to the constructor."
            )
        nonce = await self._w3.eth.get_transaction_count(self._account.address)
        gas_price = await self._w3.eth.gas_price
        tx = fn.build_transaction(
            {
                "from": self._account.address,
                "nonce": nonce,
                "gas": self._gas_limit,
                "gasPrice": gas_price,
                "value": value,
            }
        )
        try:
            estimated = await self._w3.eth.estimate_gas(tx)
            tx["gas"] = int(estimated * 1.1)
        except Exception:
            pass
        signed = self._account.sign_transaction(tx)
        tx_hash = await self._w3.eth.send_raw_transaction(signed.raw_transaction)
        return await self._w3.eth.wait_for_transaction_receipt(tx_hash)

    @staticmethod
    def _tx_hash_hex(receipt: Any) -> str:
        """Extract tx hash as hex string."""
        h = receipt["transactionHash"]
        if isinstance(h, bytes):
            return "0x" + h.hex()
        return str(h)


class AsyncEventWatcher:
    """Async event watcher for the TEEMLVerifier contract.

    Uses ``asyncio.sleep``-based polling instead of threads.

    Parameters
    ----------
    contract_address:
        The deployed TEEMLVerifier contract address (hex string).
    rpc_url:
        An HTTP JSON-RPC endpoint URL.
    """

    def __init__(
        self,
        contract_address: str,
        rpc_url: str,
    ) -> None:
        """Create an async event watcher.

        Args:
            contract_address: Deployed TEEMLVerifier contract address.
            rpc_url: JSON-RPC endpoint URL.
        """
        self._w3 = AsyncWeb3(AsyncHTTPProvider(rpc_url))
        self._contract_address = AsyncWeb3.to_checksum_address(contract_address)
        self._rpc_url = rpc_url
        self._watching = False
        self._watch_task: asyncio.Task | None = None

    @property
    def contract_address(self) -> str:
        """The checksummed contract address being watched."""
        return self._contract_address

    @property
    def rpc_url(self) -> str:
        """The RPC URL this watcher is connected to."""
        return self._rpc_url

    @property
    def is_watching(self) -> bool:
        """Whether the background watch loop is currently running."""
        return self._watching and self._watch_task is not None and not self._watch_task.done()

    async def poll_events(
        self,
        from_block: int,
        to_block: Union[int, str] = "latest",
        event_types: Optional[Sequence[str]] = None,
    ) -> tuple:
        """Poll for events in a block range.

        Returns
        -------
        tuple[list[TEEEvent], int]
            ``(events, next_block)`` where *next_block* is the block number
            to pass as ``from_block`` on the next call.
        """
        topic0_filter = self._build_topic0_filter(event_types)

        if isinstance(to_block, str) and to_block == "latest":
            resolved_to_block = await self._w3.eth.get_block_number()
        else:
            resolved_to_block = int(to_block)

        if from_block > resolved_to_block:
            return ([], from_block)

        filter_params: dict = {
            "fromBlock": from_block,
            "toBlock": resolved_to_block,
            "address": self._contract_address,
        }
        if topic0_filter is not None:
            filter_params["topics"] = [topic0_filter]

        logs = await self._w3.eth.get_logs(filter_params)

        events: List[TEEEvent] = []
        for log_entry in logs:
            parsed = parse_log(log_entry)
            if parsed is not None:
                if event_types and parsed.event_name not in event_types:
                    continue
                events.append(parsed)

        next_block = resolved_to_block + 1
        return (events, next_block)

    async def watch(
        self,
        callback: Callable[[TEEEvent], Any],
        poll_interval: float = 2.0,
        from_block: Union[int, str] = "latest",
        event_types: Optional[Sequence[str]] = None,
    ) -> None:
        """Start an async background task that polls for events.

        Each discovered event is passed to *callback*.  The callback may be
        a regular function or an async coroutine.  Call :meth:`stop` to
        cancel the background task.
        """
        if self.is_watching:
            raise RuntimeError("Watcher is already running. Call stop() first.")

        if isinstance(from_block, str) and from_block == "latest":
            start_block = await self._w3.eth.get_block_number()
        else:
            start_block = int(from_block)

        self._watching = True

        async def _poll_loop() -> None:
            current_block = start_block
            while self._watching:
                try:
                    events, next_block = await self.poll_events(
                        from_block=current_block,
                        to_block="latest",
                        event_types=event_types,
                    )
                    for event in events:
                        if not self._watching:
                            return
                        result = callback(event)
                        # Support async callbacks
                        if asyncio.iscoroutine(result):
                            await result
                    current_block = next_block
                except asyncio.CancelledError:
                    return
                except Exception:
                    pass
                try:
                    await asyncio.sleep(poll_interval)
                except asyncio.CancelledError:
                    return

        self._watch_task = asyncio.ensure_future(_poll_loop())

    def stop(self) -> None:
        """Stop the background watch task.

        Safe to call even if the watcher is not running.
        """
        self._watching = False
        if self._watch_task is not None and not self._watch_task.done():
            self._watch_task.cancel()
        self._watch_task = None

    async def close(self) -> None:
        """Stop watching and close the underlying async HTTP provider session."""
        self.stop()
        provider = self._w3.provider
        if hasattr(provider, "_session") and provider._session is not None:
            await provider._session.close()

    @staticmethod
    def _build_topic0_filter(
        event_types: Optional[Sequence[str]],
    ) -> Optional[Any]:
        """Build a topic0 filter value for ``eth_getLogs``."""
        if not event_types:
            return None
        hashes = []
        for name in event_types:
            if name not in _EVENT_SIGNATURES:
                raise ValueError(
                    f"Unknown event type: {name!r}. "
                    f"Valid types: {', '.join(_EVENT_SIGNATURES.keys())}"
                )
            hashes.append(topic_hash(name))
        if len(hashes) == 1:
            return hashes[0]
        return hashes
