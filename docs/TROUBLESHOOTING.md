# Troubleshooting Guide

Error catalog and resolution guide for the world-zk-compute project.

---

## Table of Contents

- [Contract Errors: TEEMLVerifier](#contract-errors-teemlverifier)
- [Contract Errors: ExecutionEngine](#contract-errors-executionengine)
- [Contract Errors: ProgramRegistry](#contract-errors-programregistry)
- [Contract Errors: RemainderVerifier](#contract-errors-remainderverifier)
- [Contract Errors: DAG Batch Verifier](#contract-errors-dag-batch-verifier)
- [Contract Errors: GKR / Hyrax / Sumcheck](#contract-errors-gkr--hyrax--sumcheck)
- [Operator Service Errors](#operator-service-errors)
- [Enclave Errors](#enclave-errors)
- [Attestation Errors](#attestation-errors)
- [SDK Errors](#sdk-errors)
- [Deployment Errors](#deployment-errors)
- [Common Issues](#common-issues)

---

## Contract Errors: TEEMLVerifier

These are custom errors defined in `contracts/src/tee/ITEEMLVerifier.sol` and used via `revert` in `contracts/src/tee/TEEMLVerifier.sol`.

### Admin Functions

| Error | Function | Cause | Fix |
|---|---|---|---|
| `ZeroEnclaveKey()` | `registerEnclave` | Passed `address(0)` as enclave key | Provide a valid non-zero enclave signing address |
| `AlreadyRegistered()` | `registerEnclave` | Enclave key already registered | Use a different key, or revoke+re-register |
| `NotRegistered()` | `revokeEnclave` | Enclave key was never registered | Check the enclave address; register first |
| `AlreadyRevoked()` | `revokeEnclave` | Enclave already revoked | No action needed |
| `ZeroAddress()` | `setRemainderVerifier` | Passed `address(0)` as verifier | Provide a valid RemainderVerifier address |
| `ZeroAmount()` | `setChallengeBondAmount`, `setProverStake` | Amount is 0 | Set a positive amount |
| `AmountTooHigh()` | `setChallengeBondAmount`, `setProverStake` | Amount exceeds 100 ETH | Use a value <= 100 ether |
| `WindowTooShort()` | `setChallengeWindow`, `setDisputeWindow` | Duration below minimum | Use a longer duration (challenge >= 10 min, dispute >= 1 hour) |
| `WindowTooLong()` | `setChallengeWindow`, `setDisputeWindow` | Duration above maximum | Use a shorter duration (challenge <= 7 days, dispute <= 30 days) |
| `OwnableUnauthorizedAccount` | Any admin function | Caller is not the contract owner | Call from the owner address |
| `EnforcedPause` | `submitResult`, `challenge` | Contract is paused | Owner must call `unpause()` first |

### Submit / Challenge / Finalize

| Error | Function | Cause | Fix |
|---|---|---|---|
| `InsufficientStake()` | `submitResult` | `msg.value` < `proverStake` (default 0.1 ETH) | Send at least 0.1 ETH with the transaction. Check: `cast call $CONTRACT "proverStake()(uint256)"` |
| `UnregisteredEnclave()` | `submitResult` | ECDSA-recovered signer is not a registered enclave | Register the enclave key first, or check that the attestation signature is correct |
| `EnclaveNotActive()` | `submitResult` | The recovered signer is registered but revoked | Re-register the enclave or use a different active enclave |
| `ResultExists()` | `submitResult` | Same sender+model+input+block already submitted | Wait for the next block or change inputs |
| `ResultNotFound()` | `challenge`, `finalize` | Invalid `resultId` (never submitted) | Verify the result ID; compute it as `keccak256(sender, modelHash, inputHash, blockNumber)` |
| `AlreadyFinalized()` | `challenge`, `finalize` | Result was already finalized | No further action possible on this result |
| `AlreadyChallenged()` | `challenge` | Another challenger already challenged this result | Only one challenge per result is allowed |
| `InsufficientBond()` | `challenge` | `msg.value` < `challengeBondAmount` (default 0.1 ETH) | Send at least the required bond. Check: `cast call $CONTRACT "challengeBondAmount()(uint256)"` |
| `ChallengeWindowClosed()` | `challenge` | Challenge deadline has passed (1 hour after submission) | Challenge window is 1 hour; must challenge sooner |
| `ChallengeWindowNotPassed()` | `finalize` | Challenge window has not yet elapsed | Wait until `challengeDeadline` timestamp passes |
| `ResultIsChallenged()` | `finalize` | Result was challenged; must resolve dispute instead | Call `resolveDispute()` or wait for `resolveDisputeByTimeout()` |

### Dispute Resolution

| Error | Function | Cause | Fix |
|---|---|---|---|
| `NotChallenged()` | `resolveDispute`, `resolveDisputeByTimeout`, `extendDisputeWindow` | Result has no active challenge | Cannot resolve a dispute that does not exist |
| `AlreadyResolved()` | `resolveDispute`, `resolveDisputeByTimeout`, `extendDisputeWindow` | Dispute was already settled | Check `disputeResolved(resultId)` |
| `NoVerifierSet()` | `resolveDispute` | `remainderVerifier` is `address(0)` | Owner must call `setRemainderVerifier(address)` |
| `DeadlineNotReached()` | `resolveDisputeByTimeout` | Dispute window (24h) has not expired | Wait until `disputeDeadline` passes |
| `NotSubmitter()` | `extendDisputeWindow` | Caller is not the original submitter | Only the submitter can extend |
| `MaxExtensionsReached()` | `extendDisputeWindow` | Already used the 1 allowed extension | No more extensions available |
| `StakeReturnFailed()` | `finalize` | ETH transfer to submitter failed | Submitter address may be a contract that rejects ETH |
| `PayoutFailed()` | `_settleDispute` | ETH payout transfer failed | Winner address may be a contract that rejects ETH |

---

## Contract Errors: ExecutionEngine

Custom errors from `contracts/src/ExecutionEngine.sol`.

| Error | Function | Cause | Fix |
|---|---|---|---|
| `InsufficientTip()` | `requestExecution` | `msg.value` < `MIN_TIP` (0.0001 ETH) | Send at least 0.0001 ETH |
| `ProgramNotActive()` | `requestExecution` | Program not registered or deactivated | Register and activate the program in ProgramRegistry |
| `RequestNotFound()` | `cancelExecution`, `claimExecution`, `submitProof`, `getRequest` | Invalid request ID | Check that the request ID exists |
| `RequestNotPending()` | `cancelExecution`, `claimExecution` | Request is not in Pending status | Can only cancel/claim pending requests |
| `RequestNotClaimed()` | `submitProof` | Request has not been claimed | Claim the request first with `claimExecution()` |
| `NotRequester()` | `cancelExecution` | Caller is not the original requester | Only the requester can cancel |
| `NotClaimant()` | `submitProof` | Caller is not the prover who claimed | Only the claiming prover can submit proof |
| `ClaimNotExpired()` | `claimExecution` | Trying to reclaim a request whose claim is still active | Wait for the claim deadline to pass |
| `RequestExpired()` | `claimExecution` | Request expiration has passed | Request is no longer available |
| `ClaimDeadlinePassed()` | `submitProof` | Proof submitted after claim window (10 min) | Claim and prove within the time limit |
| `InvalidProof()` | `submitProof` | Proof verification failed | Regenerate the proof; check image ID matches |
| `EmptySeal()` | `submitProof` | Empty seal bytes provided | Provide valid proof seal data |
| `EmptyJournal()` | `submitProof` | Empty journal bytes provided | Provide valid journal/public outputs |
| `TransferFailed()` | `cancelExecution`, `submitProof` | ETH transfer failed | Receiving address may reject ETH transfers |
| `ZeroImageId()` | `requestExecution` | `imageId` is `bytes32(0)` | Provide a valid program image ID |
| `ExpirationInPast()` | `requestExecution` | Computed expiration overflows | Use a reasonable expiration value |
| `ZeroAddress()` | Constructor, `setFeeRecipient` | Address parameter is zero | Provide a valid non-zero address |
| `Fee too high` | `setProtocolFee` | Fee exceeds 1000 bps (10%) | Use a value <= 1000 |

---

## Contract Errors: ProgramRegistry

Custom errors from `contracts/src/ProgramRegistry.sol`.

| Error | Function | Cause | Fix |
|---|---|---|---|
| `ProgramAlreadyRegistered()` | `registerProgram` | Image ID already registered | Use a unique image ID |
| `ProgramNotFound()` | Various | Image ID not in registry | Register the program first |
| `NotProgramOwner()` | `updateProgramUrl`, `updateVerifier` | Caller does not own this program | Call from the program owner address |
| `InvalidImageId()` | `registerProgram` | Image ID is `bytes32(0)` | Provide a valid image ID |
| `ProgramAlreadyVerified()` | `verifyProgram` | Already verified by admin | No action needed |
| `ProgramNotVerified()` | `unverifyProgram` | Program was not verified | Cannot unverify a non-verified program |
| `ProgramAlreadyActive()` | `reactivateProgram` | Program is already active | No action needed |
| `ProgramAlreadyInactive()` | `deactivateProgram` | Program is already deactivated | No action needed |

---

## Contract Errors: RemainderVerifier

Custom errors and require messages from `contracts/src/remainder/RemainderVerifier.sol`.

| Error | When | Fix |
|---|---|---|
| `CircuitNotRegistered()` | Circuit hash not found in registry | Register the circuit with `registerDAGCircuit()` or `registerCircuit()` |
| `CircuitNotActive()` | Circuit is deactivated | Reactivate with `activateCircuit()` |
| `InvalidProofSelector()` | Proof does not start with `"REM1"` magic bytes | Ensure proof was generated by the Remainder prover |
| `InvalidProofLength()` | Proof is less than 4 bytes | Provide a complete proof blob |
| `ProofVerificationFailed()` | GKR/Hyrax verification returned false | Regenerate the proof; inputs may not match |
| `InvalidGenerators()` | `keccak256(gensData)` does not match registered `gensHash` | Use the correct generators data that matches what was registered |
| `ZeroAddress()` | Zero address passed to `setGroth16Verifier` | Provide a valid verifier address |
| `Circuit already registered` | `registerCircuit` / `registerDAGCircuit` | Circuit hash exists | Use a unique circuit hash or deactivate the old one |
| `Layer sizes/types/isCommitted length mismatch` | `registerCircuit` | Array lengths do not match `numLayers` | Ensure all arrays have exactly `numLayers` entries |
| `Groth16 verifier not set` | `verifyWithGroth16` | No Groth16 verifier registered for this circuit | Call `setCircuitGroth16Verifier(circuitHash, verifier, inputCount)` |
| `Hybrid: need >= 2 layers` | `verifyWithGroth16` | Circuit has fewer than 2 layers | Circuit must have at least input + compute layers |
| `Hybrid: layer 0 must be input` | `verifyWithGroth16` | First layer is not type 3 (input) | Check circuit registration configuration |

---

## Contract Errors: DAG Batch Verifier

Require messages from the batch verification flow in `RemainderVerifier.sol`.

| Error Message | Function | Cause | Fix |
|---|---|---|---|
| `Batch: session not found` | `continueDAGBatchVerify`, `finalizeDAGBatchVerify`, `cleanupDAGBatchSession` | Session ID does not exist | Call `startDAGBatchVerify()` first |
| `Batch: unauthorized` | `continueDAGBatchVerify`, `finalizeDAGBatchVerify`, `cleanupDAGBatchSession` | Caller is not the session creator | Use the same address that called `startDAGBatchVerify()` |
| `Batch: already finalized` | `continueDAGBatchVerify`, `finalizeDAGBatchVerify` | Session already completed | No further batches needed |
| `Batch: all compute batches done, call finalize` | `continueDAGBatchVerify` | All compute layers verified | Call `finalizeDAGBatchVerify()` instead |
| `Batch: compute batches not done` | `finalizeDAGBatchVerify` | Not all compute batches processed | Call `continueDAGBatchVerify()` first |
| `Batch: not finalized` | `cleanupDAGBatchSession` | Cannot clean up an incomplete session | Finalize or abandon the session |
| `Batch finalize: Pedersen opening invalid` | `finalizeDAGBatchVerify` | Input layer Pedersen commitment check failed | Proof data may be corrupted; regenerate |

---

## Contract Errors: GKR / Hyrax / Sumcheck

Internal verification errors from the library contracts. These appear when proof data is malformed.

| Error Message | Contract | Meaning |
|---|---|---|
| `GKRVerifier: wrong number of layer proofs` | GKRVerifier | Layer proof count does not match `numLayers - 1` |
| `GKRVerifier: no output claim commitments` | GKRVerifier | Empty output commitments in proof |
| `GKRVerifier: committed sumcheck failed` | GKRVerifier | Sumcheck round verification failed |
| `GKRVerifier: PoP failed` | GKRVerifier | Proof-of-product verification failed |
| `GKRVerifier: log2(0)` | GKRVerifier | Zero-sized layer in circuit |
| `GKRVerifier: data too large for point` | GKRVerifier | MLE data exceeds point dimensions |
| `HyraxVerifier: empty commitment` | HyraxVerifier | Zero-row commitment provided |
| `HyraxVerifier: L/R coeffs length mismatch` | HyraxVerifier | Coefficient arrays wrong size |
| `HyraxVerifier: PODP vector length mismatch` | HyraxVerifier | Inner product proof vector sizes disagree |
| `HyraxVerifier: ecAdd/ecMul failed` | HyraxVerifier | EVM precompile call for EC math failed |
| `HyraxVerifier: MSM length mismatch` | HyraxVerifier | Points and scalars arrays differ in length |
| `CommittedSumcheck: bindings length mismatch` | CommittedSumcheckVerifier | Bindings count != expected rounds |
| `CommittedSumcheck: inverse of zero` | CommittedSumcheckVerifier | Division by zero in field arithmetic |
| `DAGHybrid: PoP failed` | GKRDAGHybridVerifier | Proof-of-product check failed (Groth16 hybrid) |
| `DAGHybrid: PODP Eq1/Eq2 failed` | GKRDAGHybridVerifier | EC equation check failed in hybrid flow |
| `DAGHybrid: Pedersen opening invalid` | GKRDAGHybridVerifier | Pedersen commitment does not match |
| `DAGHybrid: mleEval mismatch` | GKRDAGHybridVerifier | MLE evaluation from Groth16 does not match claim |
| `DAGHybrid: on-chain MLE mismatch` | GKRDAGHybridVerifier | On-chain MLE computation disagrees with proof |

---

## Operator Service Errors

Errors from `services/operator/`.

### Configuration Errors

| Error | Cause | Fix |
|---|---|---|
| `OPERATOR_PRIVATE_KEY is required` | Missing private key in env or config | Set `OPERATOR_PRIVATE_KEY` env var or `private_key` in TOML config |
| `TEE_VERIFIER_ADDRESS is required` | Missing contract address | Set `TEE_VERIFIER_ADDRESS` env var or `tee_verifier_address` in TOML config |
| `Invalid contract address: ...` | Malformed hex address | Use a valid `0x`-prefixed Ethereum address |
| `Invalid RPC URL: ...` | RPC URL cannot be parsed | Check URL format (e.g., `http://127.0.0.1:8545`) |
| `Invalid stake: ...` | `PROVER_STAKE` is not a valid integer | Use wei amount as a decimal string (e.g., `100000000000000000`) |
| `Invalid features JSON: ...` | Features argument is not valid JSON array | Use format: `'[5.0, 3.5, 1.5, 0.3]'` |
| `Failed to parse TOML: ...` | TOML config file has syntax errors | Validate TOML syntax |
| `Failed to read config file '...': ...` | Config file path does not exist | Check `--config` path |
| `Model '...' not found in config` | `--model` name not in `[[models]]` config | Run `tee-operator models` to list available models |

### Watcher / Event Polling Errors

| Error | Cause | Fix |
|---|---|---|
| `Failed to poll for finalizeable results: ...` | RPC call to `eth_getLogs` failed | Check RPC endpoint connectivity; may be rate-limited |
| Connection timeout on `poll_events` | RPC node unresponsive | Verify RPC URL; try a different provider |
| `Proof not available after timeout for ...` | Pre-computed proof not generated in 60s | Check prover binary path; ensure model file exists |
| `Error reading proof: ...` | Proof file corrupted or unreadable | Check `PROOFS_DIR` permissions and disk space |
| `Failed to resolve dispute: ...` | On-chain `resolveDispute` tx reverted | Check proof data, circuit hash, gas limit |

### Proof Generation Errors

| Error | Cause | Fix |
|---|---|---|
| `Proof generation failed with status: ...` | Subprocess prover binary exited non-zero | Check `PRECOMPUTE_BIN` path; verify model file |
| `Prover HTTP request failed (status): ...` | Warm prover returned error response | Check `PROVER_URL` and prover service health |
| `Proof generation failed after retries` | All retry attempts exhausted | Check prover logs; increase `MAX_PROOF_RETRIES` |
| `Enclave is not healthy at ...` | Enclave `/health` endpoint returned error | Verify enclave is running at `ENCLAVE_URL` |
| `Enclave inference failed (status): ...` | Enclave `/infer` endpoint returned error | Check feature count matches model; check enclave logs |

### Enclave Client Errors

| Error | Cause | Fix |
|---|---|---|
| `Attestation fetch failed (status): ...` | Enclave `/attestation` returned error | Check enclave connectivity; may need nonce param |
| Connection refused to enclave URL | Enclave not running or wrong port | Verify `ENCLAVE_URL` (default: `http://127.0.0.1:8080`) |

---

## Enclave Errors

HTTP status codes and messages from `tee/enclave/src/main.rs`.

### Inference Errors (POST /infer)

| HTTP Status | Error | Cause | Fix |
|---|---|---|---|
| 429 | `rate limit exceeded` | Exceeded `MAX_REQUESTS_PER_MINUTE` (default: 60) | Wait `retry_after_secs` seconds; configure `MAX_REQUESTS_PER_MINUTE` env var |
| 400 | `Expected N features, got M` | Feature vector length does not match model | Check model's expected feature count at `GET /info` |
| 400 | `Requested chain_id X does not match enclave's configured chain_id Y` | Per-request `chain_id` override conflicts with enclave config | Set `CHAIN_ID=X` env var on the enclave, or omit the `chain_id` field |
| 500 | `Failed to serialize result: ...` | Internal serialization error | Bug -- report to maintainers |
| 500 | `internal lock error: model_state poisoned` | Previous panic poisoned the RwLock | Restart the enclave process |
| 500 | `internal lock error: nitro_attestor poisoned` | Previous panic poisoned the Mutex | Restart the enclave process |

### Attestation Errors (GET /attestation)

| HTTP Status | Error | Cause | Fix |
|---|---|---|---|
| 400 | `Invalid nonce hex: ...` | Nonce query parameter is not valid hex | Use hex-encoded string (with or without `0x` prefix) |
| 500 | `internal lock error: ...` | Lock poisoned | Restart enclave |
| 503 | `No attestation document available` | NitroAttestor has not generated a document yet | On non-Nitro environments, this is expected for dev mode |

### Admin Errors (POST /admin/reload-model)

| HTTP Status | Error | Cause | Fix |
|---|---|---|---|
| 403 | `Admin endpoint disabled: ADMIN_API_KEY not configured` | `ADMIN_API_KEY` env var not set | Set `ADMIN_API_KEY` to enable admin endpoints |
| 401 | `Invalid or missing admin API key` | Wrong or missing `Authorization: Bearer <key>` header | Provide correct API key |
| 400 | `Failed to read model file '...': ...` | Model file path does not exist | Check file path inside the enclave |
| 400 | `Failed to parse model '...': ...` | Model file is not valid XGBoost/LightGBM JSON | Verify model format and content |

---

## Attestation Errors

Errors from `services/operator/src/nitro.rs` -- `AttestationError` enum.

| Error | Cause | Fix |
|---|---|---|
| `Base64 decode error: ...` | Attestation document is not valid Base64 | Check that the enclave returns proper Base64-encoded CBOR |
| `CBOR decode error: ...` | Attestation payload is not valid CBOR | Ensure the attestation document is not truncated |
| `Invalid COSE_Sign1 structure: ...` | COSE structure malformed (wrong array size, missing fields) | Regenerate attestation; document may be corrupted |
| `Missing required field: ...` | Required field (`module_id`, `timestamp`, `public_key`, `pcrs[0]`) missing | Enclave may not be a real Nitro instance |
| `Certificate chain verification failed: ...` | P-384 certificate chain does not verify up to AWS root CA | Ensure running on genuine AWS Nitro hardware |
| `Root cert public key mismatch with AWS Nitro root CA` | Root certificate is not the AWS Nitro root CA | Attestation is not from a real Nitro enclave |
| `Cert signature verification failed: ...` | Certificate in chain has an invalid signature | Attestation document may be tampered with |
| `COSE signature verification failed: ...` | COSE_Sign1 signature does not verify with leaf cert | Document integrity compromised |
| `Attestation expired or too old` | Attestation timestamp is older than `max_age_secs` (default 600s for submit, 300s for register) | Request a fresh attestation |
| `PCR validation failed: Expected PCR0=..., got ...` | PCR0 hash does not match expected value | Enclave image may have changed; update `EXPECTED_PCR0` or rebuild enclave |
| `Nonce validation failed: Expected nonce=..., got ...` | Nonce in attestation does not match what was sent | Possible replay attack; fetch fresh attestation with new nonce |
| `No nonce present in attestation` | Attestation does not contain a nonce field | Enclave must include nonce in attestation generation |
| `attestation cache lock poisoned: ...` | Attestation cache mutex poisoned | Restart the operator process |

---

## SDK Errors

### Python SDK (`sdk/python/worldzk/tee_verifier.py`)

| Error | Cause | Fix |
|---|---|---|
| `Expected 32 bytes, got N` | `_to_bytes32()` received wrong-length input | Ensure model_hash, input_hash, result_id are exactly 32 bytes |
| `web3.exceptions.ContractLogicError: ...` | On-chain require/revert triggered | See [Contract Errors](#contract-errors-teemlverifier) for the specific message |
| `web3.exceptions.TransactionNotFound` | Transaction not mined | Increase gas price or check network congestion |
| `requests.exceptions.ConnectionError` | Cannot connect to RPC URL | Verify `rpc_url` parameter |
| Gas estimation failure (silent fallback) | `estimate_gas` raised an exception | SDK falls back to `gas_limit` constructor param (default 500,000); increase if needed |
| `ValueError: {'code': -32000, 'message': 'insufficient funds'}` | Account has no ETH | Fund the signer account |

```python
# Check account balance:
from web3 import Web3
w3 = Web3(Web3.HTTPProvider("http://localhost:8545"))
print(w3.eth.get_balance("0xYourAddress"))
```

### TypeScript SDK (`sdk/typescript/src/tee-verifier.ts`)

| Error | Cause | Fix |
|---|---|---|
| `ContractFunctionRevertedError` | On-chain revert | See [Contract Errors](#contract-errors-teemlverifier) |
| `TransactionReceiptNotFoundError` | Transaction not mined within timeout | Check network; increase gas price |
| `HttpRequestError: ... ECONNREFUSED` | Cannot connect to RPC | Verify `rpcUrl` in config |
| `InsufficientFundsError` | Account has no ETH for gas + value | Fund the signer account |
| Wrong `chainId` | SDK chain ID does not match network | Set `chainId` in `TEEVerifierConfig` (default: 31337 for Anvil) |

```typescript
// Default chainId is 31337 (Anvil). For other networks:
const verifier = new TEEVerifier({
  rpcUrl: "https://sepolia.infura.io/v3/...",
  privateKey: "0x...",
  contractAddress: "0x...",
  chainId: 11155111, // Sepolia
});
```

### Rust SDK (`sdk/src/tee.rs`)

The Rust SDK propagates errors via `anyhow::Result`. Common underlying errors:

| Error Pattern | Cause | Fix |
|---|---|---|
| `alloy::transports::TransportError` | RPC connection failure | Check RPC URL |
| `alloy::contract::Error` | Contract call reverted | See contract error tables above |
| `Failed to parse private key` | Invalid hex private key | Include `0x` prefix; must be 64 hex chars |

---

## Deployment Errors

### Foundry / forge

| Error | Cause | Fix |
|---|---|---|
| `Failed to deploy TEEMLVerifier` | `forge create` failed | Check constructor args; ensure Anvil/RPC is running |
| `EvmError: OutOfGas` | Contract deployment exceeds gas limit | Increase `--gas-limit` (default may be too low for large contracts) |
| `compiler error: Stack too deep` | Solidity compilation failure | `foundry.toml` has `via_ir` commented out intentionally; fix is function extraction, not IR |
| `script failed: ...` | Forge script reverted during execution | Check env vars: `DEPLOYER_KEY`, `REMAINDER_VERIFIER`, `ADMIN_ADDRESS` |
| `owner mismatch after deploy` | Deploy script post-condition check failed | Deployer address mismatch; check `DEPLOYER_KEY` |

```bash
# Deploy TEEMLVerifier to Anvil:
DEPLOYER_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
forge script script/DeployTEEMLVerifier.s.sol --broadcast --rpc-url http://127.0.0.1:8545

# Deploy with forge create (simpler):
forge create src/tee/TEEMLVerifier.sol:TEEMLVerifier \
  --rpc-url http://127.0.0.1:8545 \
  --private-key $DEPLOYER_KEY \
  --constructor-args $ADMIN_ADDR $REMAINDER_VERIFIER_ADDR
```

### Anvil Issues

| Issue | Fix |
|---|---|
| `anvil: command not found` | Install foundry: `curl -L https://foundry.paradigm.xyz \| bash && foundryup` |
| Port already in use | Kill existing Anvil: `lsof -ti:8545 \| xargs kill` or use `--port 8551` |
| Transactions not mining | Anvil auto-mines by default; if using `--no-mining`, call `anvil_mine` |
| Time-dependent tests failing | Use `cast rpc anvil_increaseTime 3601` to fast-forward |

---

## Common Issues

### "Transaction reverted without reason"

This typically means one of:
1. **Out of gas** -- Increase gas limit. DAG verification needs >250M gas (use `--gas-limit 500000000` on Anvil).
2. **Wrong function parameters** -- Verify ABI encoding. Use `cast calldata` to debug.
3. **Contract paused** -- Check: `cast call $CONTRACT "paused()(bool)" --rpc-url $RPC`.

```bash
# Debug a reverted transaction:
cast call $CONTRACT "functionName(args)" $ARGS --rpc-url $RPC --from $SENDER
# This gives the revert reason without spending gas
```

### Rate Limiting (429 Responses)

**Enclave rate limit:**
- Default: 60 requests/minute
- Response includes `retry_after_secs`
- Configure: `MAX_REQUESTS_PER_MINUTE` env var on enclave

**RPC provider rate limit:**
- Use a dedicated RPC node or paid plan
- Add retry logic with exponential backoff
- The operator service retries proof generation with backoff (configurable via `MAX_PROOF_RETRIES` and `PROOF_RETRY_DELAY_SECS`)

### Anvil vs Mainnet/Sepolia Differences

| Behavior | Anvil | Production |
|---|---|---|
| Gas limit | Configurable (`--gas-limit`) | 30M block limit |
| Time manipulation | `anvil_increaseTime` available | Not available; must wait real time |
| Accounts | Pre-funded with 10,000 ETH | Must fund manually |
| Chain ID | 31337 | Network-specific (1, 11155111, etc.) |
| DAG verification | Works in single tx with `--gas-limit 500000000` | Requires multi-tx batch verification (15 txs) |

### Insufficient Funds

```bash
# Check balance:
cast balance $ADDRESS --rpc-url $RPC

# Fund an account on Anvil:
cast send $ADDRESS --value 1ether --rpc-url http://127.0.0.1:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
```

### Nonce Issues

| Symptom | Fix |
|---|---|
| `nonce too low` | Another tx was mined; retry with updated nonce |
| `nonce too high` | Previous tx is still pending; wait or speed it up |
| `replacement transaction underpriced` | Resubmitting with same nonce but lower gas | Increase gas price |

```bash
# Get current nonce:
cast nonce $ADDRESS --rpc-url $RPC

# Reset nonce on Anvil:
cast rpc anvil_setNonce $ADDRESS 0x0 --rpc-url http://127.0.0.1:8545
```

### Operator Service Configuration Reference

All environment variables with defaults:

```bash
# Required
OPERATOR_PRIVATE_KEY=0x...           # No default
TEE_VERIFIER_ADDRESS=0x...           # No default

# Optional with defaults
OPERATOR_RPC_URL=http://127.0.0.1:8545
ENCLAVE_URL=http://127.0.0.1:8080
MODEL_PATH=./model.json
PROOFS_DIR=./proofs
PROVER_STAKE=100000000000000000      # 0.1 ETH in wei
PRECOMPUTE_BIN=precompute_proof
NITRO_VERIFICATION=false
EXPECTED_PCR0=                       # Optional
ATTESTATION_CACHE_TTL=300            # seconds
PROVER_URL=                          # Optional warm prover URL
MAX_PROOF_RETRIES=3
PROOF_RETRY_DELAY_SECS=10
WEBHOOK_URL=                         # Optional Slack webhook
RUST_LOG=info                        # Logging level
RUST_LOG_FORMAT=                     # Set to "json" for structured logs
```

### Enclave Configuration Reference

```bash
# Required
MODEL_PATH=/path/to/model.json

# Optional with defaults
ENCLAVE_PORT=8080
CHAIN_ID=1                           # Ethereum mainnet
MAX_REQUESTS_PER_MINUTE=60
ADMIN_API_KEY=                       # Required for /admin/* endpoints
MODEL_FORMAT=auto                    # "auto", "xgboost", or "lightgbm"
```

### Quick Diagnostic Commands

```bash
# Check if Anvil is running:
curl -s http://127.0.0.1:8545 -X POST -H "Content-Type: application/json" \
  --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'

# Check if enclave is healthy:
curl -s http://127.0.0.1:8080/health

# Get enclave info:
curl -s http://127.0.0.1:8080/info | jq .

# Check contract owner:
cast call $CONTRACT "owner()(address)" --rpc-url $RPC

# Check if contract is paused:
cast call $CONTRACT "paused()(bool)" --rpc-url $RPC

# Check challenge bond amount:
cast call $CONTRACT "challengeBondAmount()(uint256)" --rpc-url $RPC

# Check prover stake:
cast call $CONTRACT "proverStake()(uint256)" --rpc-url $RPC

# Get a result:
cast call $CONTRACT "getResult(bytes32)" $RESULT_ID --rpc-url $RPC

# Check if result is valid:
cast call $CONTRACT "isResultValid(bytes32)(bool)" $RESULT_ID --rpc-url $RPC

# Run Foundry tests:
cd contracts && forge test -vvv

# Run operator unit tests:
cd services/operator && cargo test

# Run E2E test:
bash scripts/tee-e2e.sh
```

---

## Service Health Checks

| Service | Endpoint | Expected Response | Common Failures |
|---------|----------|-------------------|-----------------|
| Enclave | `GET /health` | `{"status":"ok","model_loaded":true,"model_hash":"0x..."}` | Model not loaded, wrong model path |
| Warm Prover | `GET /health` | `{"status":"ok","num_features":N,"model":"..."}` | Binary not found, model parse error |
| Operator | `GET /health` | `{"status":"ok"}` | RPC unreachable, missing contract addresses |
| Indexer | `GET /health` | `{"status":"ok"}` | Database connection failed |
| Indexer | `GET /stats` | `{"total_results":N,"latest_block":N}` | Not synced, no events indexed |

### Health Check Commands

```bash
# Quick health check for all services
curl -s http://localhost:8080/health | jq .   # Enclave
curl -s http://localhost:3000/health | jq .   # Warm Prover
curl -s http://localhost:9090/health | jq .   # Operator
curl -s http://localhost:8081/health | jq .   # Indexer
```

---

## Load Test Troubleshooting

### HTTP 429 (Rate Limited)

The enclave enforces `MAX_REQUESTS_PER_MINUTE` (default: 60). Reduce load test concurrency
or increase the limit:

```bash
# Reduce concurrency
./scripts/load-test-enclave.sh --concurrency 2 --requests 10

# Or increase the limit on the enclave
MAX_REQUESTS_PER_MINUTE=120 docker compose up enclave
```

### Connection Refused

Service not running or wrong port. Verify with:

```bash
docker compose ps              # Check service status
docker compose logs enclave    # Check for startup errors
```

### WebSocket Test Failures

The `ws-scale` mode requires either `websocat` or Python 3 with `websockets`:

```bash
# Install websocat (macOS)
brew install websocat

# Or install Python websockets
pip3 install websockets
```
