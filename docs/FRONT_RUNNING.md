# Front-Running Protection Analysis

**Scope:** `ExecutionEngine.sol`, `TEEMLVerifier.sol`, `ProverReputation.sol`
**Related:** [THREAT_MODEL.md](./THREAT_MODEL.md) section 3.4

---

## Operations and Risk Assessment

### HIGH RISK: `claimExecution()` (ExecutionEngine, line 312)

**Vulnerability:** A prover broadcasts a `claimExecution(requestId)` transaction. An MEV bot sees it in the mempool, front-runs it, and claims the request first. The original prover's transaction reverts (request is no longer Pending). The bot earns the tip by submitting a proof.

**Current mitigations:** None. Any address can claim any pending request. There is no commit-reveal scheme, no allowlist, and no prover-request binding.

**Impact:** Provers lose work they discovered. Sophisticated MEV bots with proving infrastructure could systematically steal high-tip requests. The tip decay mechanism (`TIP_DECAY_PERIOD = 30 min`) reduces the incentive gap over time, but early claims are most profitable.

**Recommendations:**
1. **Commit-reveal claiming.** Prover commits `keccak256(requestId, proverAddress, secret)` in tx 1, then reveals in tx 2. The reveal window (e.g., 1 block) prevents front-running. Cost: extra transaction, ~1 block delay.
2. **Private mempool / Flashbots Protect.** Provers submit claims via Flashbots Protect or a private RPC. This hides the transaction from the public mempool. No contract changes needed. Works on Ethereum L1 and some L2s.
3. **Reputation-gated claiming.** Require `ProverReputation.isGoodStanding(msg.sender)` in `claimExecution()`. Raises the bar for MEV bots (they must maintain reputation). Already partially supported via the `reputation` field but not enforced.

### LOW RISK: `submitProof()` (ExecutionEngine, line 341)

**Vulnerability:** A bot observes a `submitProof()` transaction, extracts the proof from calldata, and front-runs with the same proof.

**Current mitigations:** Strong. Line 349 enforces `req.claimedBy != msg.sender` -- only the address that claimed the request can submit the proof. Front-running `submitProof()` with a different sender always reverts.

**Impact:** None under current design. The claim-then-prove two-step binds proof submission to a specific prover address.

### LOW RISK: `challenge()` (TEEMLVerifier, line 243)

**Vulnerability:** A watcher broadcasts `challenge(resultId)`. An MEV bot front-runs to become the challenger, capturing the potential bond + stake payout (0.2 ETH).

**Current mitigations:** Partial. The front-runner must post `challengeBondAmount` (0.1 ETH) and takes on the risk of losing it if the prover produces a valid ZK proof. The 1-hour `CHALLENGE_WINDOW` means there is no urgency advantage -- multiple parties have ample time to challenge.

**Impact:** Low. The front-runner assumes the same economic risk as the original challenger. If the result is genuinely incorrect, the front-runner wins 0.2 ETH but also performed a useful service (challenging a bad result). If the result is correct, the front-runner loses 0.1 ETH.

**Recommendation:** Accept the risk. Front-running challenges is economically neutral to the protocol since the bad result still gets challenged. If desired, a commit-reveal challenge scheme could be added, but it would delay challenges and reduce the effective challenge window.

### NO RISK: `resolveDispute()` / `resolveDisputeByTimeout()` (TEEMLVerifier, lines 264, 293)

**Current mitigations:** Strong. Payouts go to `r.submitter` or `r.challenger` (fixed at submission/challenge time). Front-running these calls cannot redirect funds. Anyone can call `resolveDispute()` and the payout destination is the same regardless of `msg.sender`.

### NO RISK: `submitResult()` (TEEMLVerifier, line 200)

**Current mitigations:** Strong. The `resultId` is derived from `keccak256(msg.sender, modelHash, inputHash, block.number)`. A front-runner using a different `msg.sender` would produce a different `resultId`. The attestation signature is bound to the specific `(modelHash, inputHash, resultHash)` tuple, and the enclave key cannot be reused by a different submitter.

### LOW RISK: `finalize()` (TEEMLVerifier, line 322)

**Vulnerability:** An MEV bot front-runs a `finalize()` call. Since `finalize()` returns the prover stake to `r.submitter` (not `msg.sender`), the front-runner gains nothing except wasting gas.

**Impact:** None. The function is permissionless by design. Anyone calling it triggers the same payout to the same address.

---

## Summary Table

| Operation | Contract | Risk | Front-Run Profitable? | Mitigation Status |
|---|---|---|---|---|
| `claimExecution()` | ExecutionEngine | **HIGH** | Yes -- steal high-tip jobs | None |
| `submitProof()` | ExecutionEngine | Low | No -- sender check | Mitigated |
| `challenge()` | TEEMLVerifier | Low | Marginal -- must post bond | Acceptable |
| `resolveDispute()` | TEEMLVerifier | None | No -- fixed payout address | Mitigated |
| `submitResult()` | TEEMLVerifier | None | No -- sender in resultId | Mitigated |
| `finalize()` | TEEMLVerifier | None | No -- fixed payout address | Mitigated |

---

## Would Flashbots / MEV Protection Help?

**Yes, specifically for `claimExecution()`.** Flashbots Protect (or similar private transaction services) would prevent MEV bots from observing claim transactions in the public mempool. This is the lowest-friction mitigation since it requires no contract changes -- provers simply use a private RPC endpoint.

For L2 deployments (Arbitrum, World Chain), sequencer ordering is typically FIFO rather than gas-price auction, which significantly reduces MEV opportunities. On these chains, front-running `claimExecution()` is already harder.

**Recommendation for production:** Use Flashbots Protect for L1 deployments. On L2, the sequencer ordering provides adequate protection. If stronger guarantees are needed, implement commit-reveal claiming in a future contract upgrade.
