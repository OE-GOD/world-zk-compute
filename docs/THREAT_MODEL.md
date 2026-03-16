# Threat Model: World ZK Compute

**Last updated:** 2026-03-16
**Scope:** TEEMLVerifier, ExecutionEngine, TEE enclave, operator service
**Target audience:** Security auditors, protocol reviewers

---

## 1. System Overview

World ZK Compute uses a **TEE + ZK hybrid** architecture for verifiable ML inference:

- **Happy path (~3K gas):** An ML model runs inside a TEE enclave (AWS Nitro). The enclave signs the result with ECDSA. The `TEEMLVerifier` contract verifies the signature against a registered enclave key and accepts the result after a 1-hour challenge window.
- **Dispute path (~254M gas):** Anyone can challenge a result by posting a bond. The original submitter must then provide a ZK proof (GKR/Hyrax DAG verification via `RemainderVerifier`) within 24 hours. If the proof validates, the submitter keeps both stakes. If not (or if the deadline passes), the challenger wins both stakes.
- **Compute marketplace:** `ExecutionEngine` manages a request-claim-prove lifecycle for general zkVM programs (risc0), with tip-based pricing and protocol fees.

```
User --> Enclave (TEE) --> submitResult(attestation) --> Challenge? --yes--> ZK proof
                                   |                                          |
                                   v                                          v
                         1h window passes                          resolveDispute()
                                   |                                          |
                                   v                                          v
                            finalize() [stake returned]             Winner gets both stakes
```

---

## 2. Trust Assumptions

| Assumption | Basis | Failure mode |
|---|---|---|
| **Contract owner is honest** | Ownable2Step; multisig recommended | Owner can pause, change stakes, register rogue enclaves, swap verifier |
| **TEE enclave is trusted until compromised** | AWS Nitro hardware isolation; PCR0 image hash verified on registration | Key extraction would allow forging attestations |
| **ZK proofs are computationally sound** | GKR/Hyrax/Groth16 over BN254; 128-bit security | Soundness break would allow false dispute resolution |
| **Ethereum consensus is secure** | Standard blockchain assumption | Reorgs could invalidate challenge timing |
| **Operator has reliable chain access** | RPC provider availability | Eclipse attack or RPC downtime could cause missed deadlines |

---

## 3. Attack Vectors and Mitigations

### 3.1 Malicious Submitter (Fake Results)

**Attack:** Submitter provides an incorrect inference result with a valid TEE attestation (colluding with or spoofing the enclave).

**Mitigation:**
- Submitter must post `proverStake` (default 0.1 ETH) with every submission.
- Any observer can challenge within the 1-hour `CHALLENGE_WINDOW`.
- If challenged, submitter must produce a valid ZK proof within 24 hours or lose the stake to the challenger.
- The ZK proof verifies the actual computation, not just the TEE attestation, so a wrong result cannot be defended.

**Residual:** If no one challenges within 1 hour, the bad result finalizes unchecked.

### 3.2 Malicious Challenger (Griefing)

**Attack:** Challenger spam-challenges honest results to lock up prover stakes and force expensive ZK proof generation.

**Mitigation:**
- Challenger must post `challengeBondAmount` (default 0.1 ETH) per challenge.
- If the submitter produces a valid ZK proof, the challenger loses the entire bond.
- Only one challenge allowed per result (`require(!r.challenged)`).
- Submitter can request a 30-minute extension (`extendDisputeWindow`, max 1 per dispute).
- Operator service pre-computes proofs so dispute resolution is fast.

**Cost to grief:** 0.1 ETH lost per failed challenge.

### 3.3 Compromised TEE Enclave

**Attack:** Enclave signing key is extracted via hardware side-channel or software exploit, allowing forged attestations for arbitrary results.

**Mitigation:**
- Admin can `revokeEnclave(enclaveKey)` immediately, blocking new submissions from that key.
- Results already submitted can still be challenged; the ZK dispute path catches wrong results regardless of attestation validity.
- Enclave registration records `enclaveImageHash` (PCR0) for audit trail.
- Nitro attestation verification at registration time validates the enclave image.

**Response:** Revoke enclave, challenge any suspicious pending results, deploy new enclave with rotated key.

### 3.4 Front-Running

**Attack:** MEV bot observes a `challenge()` transaction in the mempool and front-runs it to claim the challenge bond opportunity, or front-runs a `resolveDispute()` to extract value.

**Mitigation:**
- `challenge()` assigns the caller as `r.challenger`; the bond and potential payout go to that specific address. Front-running the challenge means the front-runner takes on the risk of losing their bond.
- `resolveDispute()` pays out to `r.submitter` or `r.challenger` (fixed addresses); front-running this call has no economic benefit since the payout destination cannot change.
- The 1-hour challenge window provides ample time; there is no first-come advantage.

### 3.5 Reentrancy

**Attack:** Malicious contract receives ETH payout via `_settleDispute()` or `finalize()` and re-enters the contract.

**Mitigation:**
- `resolveDispute()`, `resolveDisputeByTimeout()`, and `finalize()` all use OpenZeppelin `ReentrancyGuard` (`nonReentrant` modifier).
- `cancelExecution()` and `submitProof()` in `ExecutionEngine` also use `nonReentrant`.
- State updates (`r.finalized = true`, `disputeResolved[resultId] = true`) happen before ETH transfers (checks-effects-interactions pattern).

### 3.6 Admin Key Compromise

**Attack:** Owner private key is stolen. Attacker can pause/unpause, register rogue enclaves, change stake amounts (up to 100 ETH), or swap the `remainderVerifier` to a contract that always returns true.

**Mitigation:**
- `Ownable2Step` requires the new owner to explicitly accept ownership, preventing accidental transfers.
- `Pausable` allows emergency stop but does not drain funds.
- Stake/bond amounts are capped at 100 ETH.
- Protocol fee in `ExecutionEngine` is capped at 10% (1000 bps).

**Recommendation:** Deploy with a multisig (e.g., Safe) as owner. Consider a timelock for `setRemainderVerifier()` since swapping the verifier is the highest-impact admin action.

### 3.7 Replay Attacks

**Attack:** Reuse a valid attestation from one chain on another, or replay an old attestation for a new submission.

**Mitigation (enclave side):**
- Attestation payload includes `chainId`, monotonic `nonce`, and `timestamp` in the signed message: `keccak256(modelHash || inputHash || resultHash || chainId || nonce || timestamp)`.
- Registration uses a chain-bound nonce: `keccak256(chainId || blockNumber || enclaveAddress)`.
- Nitro attestation freshness is validated (5-10 minute window).

**NOTE -- On-chain gap:** The current `TEEMLVerifier.submitResult()` verifies only `keccak256(modelHash, inputHash, resultHash)` without checking `chainId`, `nonce`, or `timestamp` on-chain. The enclave signs the extended payload, but the contract recovers against the shorter hash. This means cross-chain replay of the on-chain transaction is possible if the same enclave key is registered on multiple chains. **Fixing this requires updating the contract's signature verification to match the enclave's extended payload.**

### 3.8 Eclipse Attack on Operator

**Attack:** Operator's RPC endpoint is compromised or partitioned, causing it to see a stale view of the chain. The operator misses challenge events and fails to submit ZK proofs before the 24-hour dispute deadline.

**Mitigation:**
- Operator persists state to disk (`StateStore`) for crash recovery.
- Operator polls every 5 seconds with deduplication of processed events.
- Webhook notifications on challenge detection enable external monitoring.

**Recommendation:** Run redundant watcher instances with independent RPC endpoints. Monitor the `ResultChallenged` event from a separate system. The 24-hour dispute window plus 30-minute extension provides a large buffer.

### 3.9 Denial of Service

**Attack:** Flood the system with spam submissions or challenges to exhaust block space or drain operator resources.

**Mitigation:**
- `proverStake` (0.1 ETH) makes spam submissions expensive.
- `challengeBondAmount` (0.1 ETH) makes spam challenges expensive.
- `ExecutionEngine` requires `MIN_TIP` (0.0001 ETH) per request.
- Enclave rate-limits inference requests (sliding window, configurable max per minute).
- Enclave admin endpoints require API key authentication.

**Cost to spam:** 0.1 ETH per submission or challenge, non-recoverable if the spam is detected.

---

## 4. Economic Analysis

| Parameter | Default | Purpose |
|---|---|---|
| `proverStake` | 0.1 ETH | Submitter's skin-in-the-game; lost if ZK proof fails |
| `challengeBondAmount` | 0.1 ETH | Challenger's skin-in-the-game; lost if ZK proof succeeds |
| `CHALLENGE_WINDOW` | 1 hour | Time for observers to challenge |
| `DISPUTE_WINDOW` | 24 hours | Time for submitter to produce ZK proof |
| `protocolFeeBps` | 250 (2.5%) | ExecutionEngine protocol fee |

**Cost to submit a fake result:** Submitter loses `proverStake` (0.1 ETH) when challenged and unable to produce a valid ZK proof. There is no scenario where a submitter profits from a fake result -- the ZK fallback is deterministic.

**Cost to grief an honest prover:** Challenger loses `challengeBondAmount` (0.1 ETH) per challenge when the prover produces a valid ZK proof. The prover profits by the bond amount minus ZK proof generation cost.

**Break-even for sustained attack:** Never profitable. Each fake submission loses 0.1 ETH with certainty (assuming at least one honest challenger exists). Each griefing challenge loses 0.1 ETH with certainty (assuming the prover is honest and has proof infrastructure).

**ZK proof generation cost:** Approximately 6-7 minutes on x86_64 for Groth16 proving (computation cost). On-chain verification costs ~254M gas for direct GKR or ~300K gas for Groth16. The economic incentive (capturing 0.2 ETH total pot) outweighs proof generation cost.

---

## 5. Residual Risks

These are known, accepted risks not fully mitigated by the current design:

1. **Unchallenged bad results.** If no honest observer challenges within 1 hour, a fake result finalizes. Mitigation depends on external monitoring infrastructure, not protocol enforcement.

2. **On-chain replay of attestations (Section 3.7).** The contract does not verify `chainId`/`nonce`/`timestamp` in the attestation signature. A valid submission on chain A can be replayed on chain B if the same enclave key is registered on both.

3. **Gas limit constraints.** Direct GKR verification (~254M gas) exceeds Ethereum L1's 30M block gas limit. The batch verifier (15 transactions) works but adds latency. Single-tx verification is only viable on L2s (Arbitrum).

4. **Admin centralization.** A single owner key controls enclave registration, stake amounts, verifier address, and pause functionality. A compromised owner could register a rogue enclave or swap the verifier to bypass ZK checks.

5. **TEE hardware trust.** The entire happy path depends on Intel/AMD/AWS hardware security. A class break in TEE technology would require falling back to ZK-only mode (much higher cost per inference).

6. **Enclave nonce reset on restart.** The monotonic nonce counter resets to 0 when the enclave process restarts. This does not enable replay on-chain (the contract uses `block.number` in `resultId`), but weakens off-chain attestation freshness guarantees.

7. **Callback trust in ExecutionEngine.** The `onExecutionComplete` callback is called after proof verification. A malicious callback contract could consume excess gas, though the try/catch prevents reverting the proof acceptance.

---

## 6. Recent Security Hardening (Phases 74-77)

The following hardening measures were applied across the contract and service codebase in Phases 74-77 (March 2026):

- **Ownable2Step migration.** `ProverRegistry` and `ProverReputation` were upgraded from `Ownable` to `Ownable2Step`, requiring explicit `acceptOwnership()` to complete ownership transfers. All core contracts now use `Ownable2Step`.
- **nonReentrant additions.** `ProverRegistry` functions (`register`, `addStake`, `withdrawStake`, `slash`) and additional `TEEMLVerifier` functions (`challenge`, `extendDisputeWindow`) now carry the `nonReentrant` modifier.
- **Pausable guards.** `ProverRegistry`, `ProgramRegistry`, and `ProverReputation` now inherit `Pausable`. State-changing functions are gated with `whenNotPaused`.
- **Custom errors.** `TEEMLVerifier` was converted from 16 string-based `require()` calls to gas-efficient custom errors (defined in `ITEEMLVerifier.sol`). GKR verifier libraries also received custom error conversions.
- **SecretString for keys.** Operator and enclave services now wrap private keys in a `SecretString` type that prevents accidental logging and implements zeroization on drop.
- **Cached storage refs.** `RemainderVerifier` circuit management functions (`deactivateCircuit`, `reactivateCircuit`, etc.) now cache mapping lookups to reduce redundant SLOADs.
- **NatSpec documentation.** All `RemainderVerifier` DAG functions received `@param`/`@return` NatSpec annotations.

---

## 7. Emergency Procedures

### Scenario A: Compromised Enclave Key

1. **Immediate:** Call `revokeEnclave(enclaveKey)` from the owner multisig.
2. **Assess:** Query all `ResultSubmitted` events from the compromised enclave. Identify unfinalized results.
3. **Challenge:** Challenge any suspicious pending results within their challenge windows.
4. **Rotate:** Deploy a new enclave image, verify attestation, call `registerEnclave()` with the new key.
5. **Post-mortem:** Audit enclave logs, determine root cause, update PCR0 if image was compromised.

### Scenario B: Verifier Contract Bug

1. **Immediate:** Call `pause()` to block new submissions and challenges.
2. **Assess:** Determine if any disputes were incorrectly resolved.
3. **Deploy:** Deploy a corrected `RemainderVerifier` and call `setRemainderVerifier()`.
4. **Resume:** Call `unpause()` once the new verifier is validated.
5. **Note:** Already-finalized results cannot be reversed on-chain. Off-chain consumers should be notified.

### Scenario C: Operator Down During Active Dispute

1. **Detect:** Webhook notification or external monitor alerts on `ResultChallenged` event.
2. **Assess:** Check dispute deadline (`r.disputeDeadline`).
3. **Extend:** If the submitter's operator is reachable, call `extendDisputeWindow()` (adds 30 minutes, once only).
4. **Manual resolve:** Generate ZK proof manually and call `resolveDispute()` from any account.
5. **If deadline passes:** Challenger wins by calling `resolveDisputeByTimeout()`. Submitter loses stake.

### Scenario D: Admin Key Compromise

1. **Immediate:** If a secondary signer exists, initiate `transferOwnership()` to a new multisig.
2. **Monitor:** Watch for unauthorized `registerEnclave`, `setRemainderVerifier`, or `setChallengeBondAmount` calls.
3. **If paused maliciously:** The new owner (after 2-step transfer) can `unpause()`. Users can still call `cancelExecution()` while paused.
4. **Nuclear option:** If the attacker already transferred ownership, the contract cannot be recovered. Deploy a new contract and migrate.
