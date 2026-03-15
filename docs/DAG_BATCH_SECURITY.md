# DAG Batch Verification Security Model

This document describes the security model for multi-transaction DAG batch verification
in the `RemainderVerifier` / `DAGBatchVerifier` contracts. The batch verifier splits a
single ~252M gas GKR DAG verification into 15 sequential transactions, each fitting
within the 30M block gas limit.

## Overview

The batch verification pipeline consists of three phases:

1. **Start** (1 tx, ~17.5M gas): Decode proof, initialize transcript, squeeze output challenges, store initial session state.
2. **Continue** (11 txs, ~13-28M gas each): Process 8 compute layers per transaction via GKR sumcheck verification.
3. **Finalize** (3 txs, ~9-22M gas each): Verify committed input evaluations and public input claims, 16 eval groups per transaction.

Total: 15 transactions for the 88-layer XGBoost sample circuit.

---

## 1. Session Authentication

### Mechanism

When `startDAGBatchVerify()` is called, a `DAGBatchSession` is created with the caller's
address stored in the `verifier` field:

```solidity
session.verifier = msg.sender;
```

Every subsequent call to `continueDAGBatchVerify()`, `finalizeDAGBatchVerify()`, and
`cleanupDAGBatchSession()` checks that `msg.sender == session.verifier`. If the check
fails, the transaction reverts.

### Properties

- **Single-owner sessions**: Only the address that started a session can interact with it.
  There is no delegation or multi-sig support at the session level.
- **No privilege escalation**: The session stores a raw address, not a role. Even the
  contract owner cannot interact with another user's session.
- **Immutable after creation**: The `verifier` field is set once during `startDAGBatchVerify()`
  and never modified.

---

## 2. Ordering Guarantees

### Sequential Batch Processing

The session tracks the next expected batch index via `nextBatchIdx`:

```
Start:    sets nextBatchIdx = 0
Continue: requires batchIdx == session.nextBatchIdx, then increments nextBatchIdx
Finalize: requires nextBatchIdx == totalBatches (all compute batches done)
```

### Properties

- **No batch skipping**: Submitting batch index 3 when `nextBatchIdx == 1` will revert.
  Every compute batch must be processed in order.
- **No batch replay**: After processing batch N, `nextBatchIdx` advances to N+1. The same
  batch index cannot be submitted twice.
- **Finalize gating**: `finalizeDAGBatchVerify()` can only be called after all compute
  batches have been processed. It verifies `nextBatchIdx == totalBatches`.
- **Multi-step finalize**: Finalize itself is progressive. The session tracks
  `finalizeInputIdx` and `finalizeGroupsDone` so finalize can be split across multiple
  transactions. `finalizeDAGBatchVerify()` returns `false` while more groups remain,
  and `true` only when all input evaluations have been verified.
- **Deterministic transcript**: The Poseidon sponge state is persisted between batches.
  Because batches are processed in strict order, the transcript is deterministic and
  matches the single-transaction verification path exactly.

---

## 3. Gas Limit Considerations

### Design Constraint

Ethereum's block gas limit is 30M. The full DAG verification costs ~252M gas, requiring
the work to be split across multiple transactions.

### Constant Tuning

| Constant | Value | Rationale |
|---|---|---|
| `LAYERS_PER_BATCH` | 8 | 88 layers / 8 = 11 batches. Per-batch gas: 13-28M (all under 30M). |
| `GROUPS_PER_FINALIZE_BATCH` | 16 | 34 groups / 16 = 3 finalize txs. Per-finalize gas: 9-22M (all under 30M). |

### Gas Profile (88-layer XGBoost Circuit)

| Phase | Gas Range | Notes |
|---|---|---|
| Start | ~17.5M | Proof decoding + transcript init + output challenge squeeze |
| Continue (per batch) | ~13-28M | Varies by layer complexity (num_vars 0-5) |
| Finalize (per batch) | ~9-22M | Varies by group size and claim count |
| Cleanup | ~2-5M | Gas refunds from zeroing storage slots |

### Calldata vs Storage Trade-off

The full proof is re-supplied via calldata in each batch transaction rather than being
stored on-chain. This is because calldata costs ~16 gas per byte vs ~20,000 gas per
32-byte storage slot for a first write. For a ~50KB proof, calldata costs ~800K gas
while storage would cost ~31M gas just for the initial write.

---

## 4. Cleanup Semantics

### When Cleanup Occurs

- **After successful verification**: The caller should invoke `cleanupDAGBatchSession()`
  after `finalizeDAGBatchVerify()` returns `true`.
- **To abort a session**: A caller can invoke `cleanupDAGBatchSession()` at any time to
  discard an incomplete verification session.
- **Cleanup is not automatic**: The contract does not clean up after finalize. This is
  deliberate to allow the caller to read session state after verification if needed.

### What Gets Cleaned

1. **Session struct** (14 storage slots): circuitHash, verifier, nextBatchIdx, totalBatches,
   numComputeLayers, finalized, sponge state (4 values), output eval (2 values),
   finalizeInputIdx, finalizeGroupsDone.
2. **Cross-batch arrays** (variable size):
   - `bindingsFlat`: All layer binding values (~300-400 uint256 for 88-layer circuit)
   - `bindingsOffsets`: Layer boundary indices (~89 uint256)
   - `outputChallenges`: Output challenge values from transcript

### Gas Refunds

Zeroing storage slots triggers EIP-3529 gas refunds (up to 4,800 gas per slot cleared,
capped at 20% of total transaction gas). For a typical 88-layer session, cleanup clears
~500 storage slots, yielding ~2.4M gas in refunds.

### Stale Session Risk

If a session is never cleaned up (e.g., the caller abandons verification mid-way),
the storage slots remain occupied indefinitely. This wastes state space but has no
security impact -- the stale session cannot be reused because:

- The session ID includes the block number, so a new session for the same circuit gets
  a different ID.
- Only the original sender can interact with the session.

---

## 5. Attack Vectors and Mitigations

### 5.1 Session Hijacking

**Attack**: An attacker observes a `startDAGBatchVerify()` transaction and attempts to
call `continueDAGBatchVerify()` with a tampered proof to trick the verifier into
accepting invalid state.

**Mitigation**: The `session.verifier` field is checked against `msg.sender` on every
continue/finalize call. Only the original sender can advance the session. An attacker
would need to compromise the sender's private key.

### 5.2 Session Replay

**Attack**: After a successful verification, an attacker replays the same sequence of
transactions to re-verify the same proof without submitting a new one.

**Mitigation**: Session IDs are derived from `keccak256(sender, circuitHash, blockNumber)`.
The `blockNumber` component ensures that a session started in block N produces a different
session ID than one started in block M, even for the same sender and circuit. A replayed
`startDAGBatchVerify()` in a new block creates a fresh, independent session.

**Residual risk**: If an attacker could submit a `startDAGBatchVerify()` transaction in
the exact same block as the original (same sender + same circuit), the session IDs would
collide. In practice this is prevented because the sender would be the one submitting
both transactions, and the second `start` would overwrite the first session's state.

### 5.3 Griefing (Cleanup Cost)

**Attack**: An attacker (who is also a legitimate verifier) starts many sessions across
different blocks without cleaning them up, consuming storage slots.

**Mitigation**: This is a self-griefing attack -- the attacker pays gas for each
`startDAGBatchVerify()` call (~17.5M gas). The storage cost is borne by the attacker.
No other users are affected because session IDs are namespaced by sender address.

**Additional consideration**: A contract using the batch verifier (e.g., `RemainderVerifier`)
can implement its own cleanup policies, such as requiring cleanup before starting a new
session, or allowing anyone to clean up sessions older than N blocks.

### 5.4 Proof Substitution (Mid-Session)

**Attack**: The attacker submits a valid proof in the start transaction, then substitutes
a different (invalid) proof in subsequent continue transactions, hoping that partial
verification of the tampered proof passes undetected.

**Mitigation**: The Poseidon transcript sponge state is persisted between batches. The
sponge absorbs proof data during each batch. If a different proof is supplied in a
subsequent batch, the sponge state will diverge from what the prover computed, causing
the sumcheck verification to fail. The transcript acts as a cryptographic binding between
the proof and the verification state.

### 5.5 Front-Running Start Transaction

**Attack**: An attacker front-runs the victim's `startDAGBatchVerify()` call with the
same parameters, creating a session that the victim cannot use.

**Mitigation**: Session IDs include the sender's address, so the attacker's session and
the victim's session have different IDs even if they verify the same circuit in the same
block. Front-running does not affect the victim.

### 5.6 Block Number Manipulation

**Attack**: A validator/miner manipulates block numbers to force session ID collisions.

**Mitigation**: Block numbers are monotonically increasing. A validator cannot produce a
block with a past block number. The session ID collision would require the same sender
address, same circuit hash, AND same block number, which cannot happen across different
blocks.

---

## Summary

| Property | Mechanism |
|---|---|
| Authentication | `session.verifier == msg.sender` check on every operation |
| Ordering | `nextBatchIdx` counter enforces sequential processing |
| Gas safety | Constants tuned so every batch fits within 30M block gas limit |
| Replay prevention | Session ID includes `block.number` |
| Namespace isolation | Session ID includes `msg.sender` |
| Transcript integrity | Poseidon sponge state persisted and verified across batches |
| Cleanup | Explicit `cleanupDAGBatchSession()` call; gas refunds via EIP-3529 |
