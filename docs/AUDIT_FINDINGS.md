# Static Analysis & Security Review Findings

**Date**: 2026-03-26
**Reviewer**: builder-b (automated + manual)
**Scope**: All Solidity contracts in `contracts/src/`

## Tools Used

- Slither v0.11.4 (ran per-contract; full-project analysis failed on RemainderVerifier due to compilation size)
- Manual source code review of all contracts in `contracts/src/`

## Contracts Analyzed

| Contract | Slither | Manual |
|----------|---------|--------|
| `TEEMLVerifier.sol` | Yes (6 findings) | Yes |
| `ExecutionEngine.sol` | Yes (6 findings) | Yes |
| `ProgramRegistry.sol` | Yes (1 finding) | Yes |
| `ProverReputation.sol` | Yes (2 findings) | Yes |
| `Upgradeable.sol` | Yes (4 findings) | Yes |
| `RemainderVerifier.sol` | Failed (too large) | Yes |
| `DAGBatchVerifier.sol` | N/A (library) | Yes |
| `RemainderVerifierUpgradeable.sol` | Pending | Yes |
| `HybridStylusGroth16Verifier.sol` | N/A (library) | Yes |
| `ProverRegistry.sol` | N/A | Yes |

---

## High Severity

### H-1: Unprotected Implementation Contract Initialization (Upgradeable.sol, TEEMLVerifier.sol, RemainderVerifier.sol)

**Slither detector**: `unprotected-upgrade`

**Description**: The `initialize()` functions on `TEEMLVerifier`, `RemainderVerifier`, and `UpgradeableExecutionEngine` are protected by the `initializer` modifier, which prevents re-initialization. However, the implementation contract itself (not the proxy) can be initialized by anyone immediately after deployment and before the proxy is set up. An attacker who initializes the implementation contract directly could call `upgradeTo()` on it (since they would be the admin) and point it to a `selfdestruct` contract, bricking the implementation.

**Affected files**:
- `contracts/src/Upgradeable.sol` (lines 163-175, 404-423)
- `contracts/src/tee/TEEMLVerifier.sol` (lines 138-148)
- `contracts/src/remainder/RemainderVerifier.sol` (lines 187-189)

**Recommendation**: Add a constructor that disables initialization on the implementation contract:
```solidity
constructor() {
    _initialized = type(uint8).max; // Prevent initialization on implementation
}
```
Note: `RemainderVerifierUpgradeable.sol` already does this correctly (line 42-43). The base `RemainderVerifier` and `TEEMLVerifier` do not.

**Status**: FIXED. Added `constructor() { _initialized = type(uint8).max; }` to both `RemainderVerifier.sol` and `TEEMLVerifier.sol`. Tests updated and passing (1166/1166).

---

### H-2: Controlled Delegatecall in Upgrade Path (Upgradeable.sol)

**Slither detector**: `controlled-delegatecall`

**Description**: `UUPSUpgradeable._upgradeToAndCall()` performs a `delegatecall` with user-supplied `data` to a user-specified `newImplementation` address. While the function is gated by `onlyTimelocked`, the delegatecall target and calldata are fully controllable. If the timelock or admin key is compromised, an attacker can delegatecall arbitrary code in the proxy's context, draining all funds and corrupting all state.

**Affected file**: `contracts/src/Upgradeable.sol` (line 193)

**Recommendation**: This is inherent to the UUPS pattern and acceptable given the `onlyTimelocked` guard. Mitigations:
1. Always use a timelock with sufficient delay (currently supported via `setTimelock()`)
2. Use a multi-sig as the timelock proposer
3. Consider adding an implementation whitelist for production deployments

**Status**: Accepted risk (standard UUPS pattern). Ensure timelock is always configured in production.

---

## Medium Severity

### M-1: Missing Zero-Address Check in TEEMLVerifier.initialize (TEEMLVerifier.sol)

**Slither detector**: `missing-zero-check`

**Description**: `TEEMLVerifier.initialize()` accepts `_remainderVerifier` without checking it is non-zero. If initialized with `address(0)`, all dispute resolution calls will fail at the `remainderVerifier.call(...)` step, but the contract would be stuck because `setRemainderVerifier()` requires `onlyTimelocked` and the timelock may not be set yet.

**Affected file**: `contracts/src/tee/TEEMLVerifier.sol` (line 138-140)

**Recommendation**: Add `if (_remainderVerifier == address(0)) revert ZeroAddress();` in `initialize()`.

**Status**: FIXED. Added zero-address check in `TEEMLVerifier.initialize()`. Tests updated (4 test call sites changed from `address(0)` to `address(0xDEAD)`). All tests passing.

### M-2: Missing Zero-Address Check in setTimelock (Upgradeable.sol)

**Slither detector**: `missing-zero-check`

**Description**: `UUPSUpgradeable.setTimelock()` allows setting `timelock` to `address(0)`, which disables timelock protection and falls back to admin-only. While this is documented behavior, there is no event distinction between "intentionally disabled" and "accidentally set to zero". The `setTimelock` function also lacks a two-step pattern, meaning a typo could lock out upgrade capability if the admin intended to set a timelock but provided a wrong address.

**Affected file**: `contracts/src/Upgradeable.sol` (lines 226-229)

**Recommendation**: This is by design (address(0) = disable timelock), but consider:
1. Adding a separate `disableTimelock()` function for clarity
2. Requiring a confirmation step when setting timelock to a new non-zero address

**Status**: Accepted design decision. Document clearly.

### M-3: Reentrancy in ExecutionEngine.reportBadProof (ExecutionEngine.sol)

**Slither detector**: `reentrancy-no-eth`

**Description**: `reportBadProof()` calls `reputation.recordFailure()` (external call to potentially untrusted contract) before writing state changes (`req.status = Pending`, `req.claimedBy = address(0)`, etc.). While `reportBadProof()` has the `nonReentrant` modifier, if `reputation` is a malicious contract, it could call back into OTHER functions that read the stale request state (e.g., `getCurrentTip()`, `getPendingRequests()`).

However, the `nonReentrant` guard prevents re-entering any `nonReentrant` function, and the view functions that read stale state cannot cause fund loss. The reputation call is also wrapped in try/catch in `claimExecution`, but NOT in `reportBadProof`.

**Affected file**: `contracts/src/ExecutionEngine.sol` (lines 494-513)

**Recommendation**: Move the `reputation.recordFailure()` call after the state changes (checks-effects-interactions pattern), or wrap it in try/catch as done elsewhere.

**Status**: FIXED. State changes (`req.status`, `req.claimedBy`, etc.) now occur before the `reputation.recordFailure()` external call.

### M-4: Reentrancy in ExecutionEngine.claimExecution (ExecutionEngine.sol)

**Slither detector**: `reentrancy-no-eth`

**Description**: `claimExecution()` calls `reputation.recordAbandon()` (external call) before writing new claim state. Similar to M-3, the `nonReentrant` guard prevents direct re-entry but state could be read in stale form by view functions during the external call.

**Affected file**: `contracts/src/ExecutionEngine.sol` (lines 380-404)

**Recommendation**: Move `reputation.recordAbandon()` call after state updates.

**Status**: FIXED. `reputation.recordAbandon()` now called after state updates (`req.status`, `req.claimedBy`, `req.claimDeadline`). Previous claimant address saved to local variable before state changes.

### M-5: TEEMLVerifier resultId Collision Risk (TEEMLVerifier.sol)

**Description** (manual review): The `resultId` is computed as `keccak256(msg.sender, modelHash, inputHash, block.number)`. If the same sender submits two results for the same model and input in the same block, the second submission will revert with `ResultExists`. This is documented but creates a subtle issue: an attacker could front-run a legitimate submission by submitting with the same parameters in the same block, causing the victim's transaction to revert.

**Affected file**: `contracts/src/tee/TEEMLVerifier.sol` (line 288)

**Recommendation**: Include a nonce or `resultHash` in the `resultId` derivation to prevent same-block collisions:
```solidity
resultId = keccak256(abi.encodePacked(msg.sender, modelHash, inputHash, resultHash, block.number));
```

**Status**: Design consideration. Current behavior prevents duplicate submissions but is vulnerable to front-running griefing.

---

## Low Severity (Accepted)

| Finding | File | Rationale for accepting |
|---------|------|------------------------|
| L-1: Strict equality checks for existence (`id == 0`, `registeredAt == 0`) | `ExecutionEngine.sol`, `ProgramRegistry.sol` | Standard pattern for checking uninitialized storage. Not exploitable because the first valid ID is 1 and `registeredAt` uses `block.timestamp` which is always > 0. |
| L-2: Strict equality in ProverReputation (`decayPeriods == 0`, `avgProofTimeMs == 0`) | `ProverReputation.sol` | Both are initialization checks on uint values. Safe usage pattern. |
| L-3: Assembly usage in storage (DAGBatchVerifier, PoseidonSponge, RemainderVerifier) | Multiple files | Assembly is used for gas optimization in storage packing, Poseidon permutation, and proof encoding. All assembly blocks are well-documented with offset comments and tested with 166+ Solidity tests. |
| L-4: Low-level `.call{value:}` for ETH transfers | `TEEMLVerifier.sol`, `ExecutionEngine.sol`, `Upgradeable.sol` | Used instead of `transfer()` for contract wallet compatibility. All sites check the return value and revert on failure. Protected by `nonReentrant`. |
| L-5: Generator hash bypass when `gensData.length == 0` | `RemainderVerifier.sol` (9 locations) | The condition `gensHash != 0 && gensData.length > 0` means empty `gensData` bypasses the hash check. However, `decodePedersenGens()` returns empty generators for empty data, and downstream PODP verification will fail without valid generators. Net effect: proof verification fails, not a security bypass. |
| L-6: UUPSProxy constructor missing zero-check on implementation | `Upgradeable.sol` (line 38) | If deployed with `address(0)`, the proxy would be non-functional. This is a deployment error, not a runtime vulnerability. Deployers are trusted. |
| L-7: No `tx.origin` usage | All contracts | Confirmed: no `tx.origin` usage found. |
| L-8: No `selfdestruct` usage | All contracts | Confirmed: no `selfdestruct` usage found. |
| L-9: `unchecked` blocks | `ProverRegistry.sol`, Groth16 verifiers | Used only for loop counter increments and field arithmetic in auto-generated Groth16 verifier contracts where overflow is mathematically impossible. Safe usage. |

---

## Informational

### I-1: DAGBatchVerifier Session Cleanup is Caller-Responsibility

The `cleanupDAGBatchSession()` function must be called by the session creator after finalization to reclaim storage slots and gas refunds. If not called, storage remains allocated indefinitely. This is documented in the contract but could lead to storage bloat in production.

### I-2: TEEMLVerifier Dispute Resolution Uses `.call()` Instead of `.staticcall()`

`resolveDispute()` uses `.call()` to invoke `RemainderVerifier.verifyDAGProof()`, which allows the callee to modify state. This is intentional (the verifier emits events) and protected by `nonReentrant`, but an upgraded or malicious `remainderVerifier` could modify TEEMLVerifier state via delegatecall if the address points to a proxy. The `remainderVerifier` address is set via `onlyTimelocked`, mitigating this risk.

### I-3: Batch Session ID Collision Within Same Block

`DAGBatchVerifier.generateSessionId()` uses `keccak256(sender, circuitHash, blockNum)`. Same sender + same circuit + same block = collision. This is documented and by-design (overwrites own session), but in a relayer/bundler context where multiple users share a sender address, collisions could occur.

### I-4: ProgramRegistry Allows Anyone to Register Programs

`registerProgram()` and `registerProgramWithVerifier()` are gated only by `whenNotPaused`, not by ownership. Any address can register a program. This is by design (permissionless registration) but means malicious programs can be registered. The `verifyProgram()` admin function provides a trust signal, and programs must pass proof verification regardless.

### I-5: No Event Indexed Field for Result Data in TEEMLVerifier

`ResultSubmitted` event does not include the `resultHash` as an indexed field, making it harder to filter for specific results off-chain. Consider adding `bytes32 indexed resultHash` to the event.

---

## Summary

| Severity | Count | Fixed | Accepted |
|----------|-------|-------|----------|
| High | 2 | 1 (H-1) | 1 (H-2, standard UUPS) |
| Medium | 5 | 3 (M-1, M-3, M-4) | 1 (M-2, by design) |
| Low | 9 | 0 | 9 (all accepted) |
| Informational | 5 | N/A | N/A |

### Fixes Applied

1. **H-1 FIXED**: Added `constructor() { _initialized = type(uint8).max; }` to `RemainderVerifier.sol` and `TEEMLVerifier.sol`
2. **M-1 FIXED**: Added `if (_remainderVerifier == address(0)) revert ZeroAddress()` in `TEEMLVerifier.initialize()`
3. **M-3 FIXED**: Reordered `reportBadProof()` to update state before `reputation.recordFailure()` call
4. **M-4 FIXED**: Reordered `claimExecution()` to update state before `reputation.recordAbandon()` call

### Remaining Action Items

1. **H-2**: Ensure timelock is always configured before mainnet deployment (accepted UUPS risk)
2. **M-2**: Document that `setTimelock(address(0))` disables timelock protection (accepted by design)
3. **M-5**: Consider adding `resultHash` to `resultId` derivation to prevent front-running griefing

### Test Results After Fixes

All 1166 tests passing (0 failures, 0 skipped). Three tests were updated to accommodate the new security checks:
- `RemainderVerifierUpgradeableTest.test_implementationCannotBeInitializedDirectly` (updated expectations)
- `TEEMLVerifierSecurityTest.setUp` (updated to use non-zero remainderVerifier)
- `SystemIntegrationTest.setUp` (updated to use non-zero remainderVerifier)
