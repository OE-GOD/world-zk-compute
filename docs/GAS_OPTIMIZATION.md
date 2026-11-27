# Contract Gas Optimization Report

Measured with `forge test --match-contract GasProfileTest -vv` on Solidity 0.8.20. Each figure is the value the test logs: `gasleft()` before the call minus `gasleft()` after it.

---

## 1. Current Gas Costs

### TEEMLVerifier (`contracts/src/tee/TEEMLVerifier.sol`)

| Operation              | Gas Used | ETH @ 30 gwei | USD @ $2500/ETH | Notes                                      |
|------------------------|----------|----------------|-----------------|---------------------------------------------|
| `submitResult`         | 249,780  | 0.00749        | $18.73          | ECDSA recover + struct write (via UUPS proxy) |
| `challenge`            | 101,963  | 0.00306        | $7.65           | Bond check + bond escrow                    |
| `finalize`             | 53,888   | 0.00162        | $4.04           | ETH transfers + state cleanup (via proxy)   |
| `resolveDisputeByTimeout` | 83,990 | 0.00252      | $6.30           | ETH transfers + dispute settlement          |
| `registerEnclave`      | 80,473   | 0.00241        | $6.04           | Cold SSTORE for EnclaveInfo struct          |

**Happy path total (submit + finalize): ~304K gas ($22.78 at 30 gwei L1, $0.076 at 0.1 gwei Arbitrum)**

> Note: Gas costs increased ~60% after UUPS proxy migration (delegatecall overhead + proxy storage reads).
> On L2 (Arbitrum/World Chain), absolute costs remain negligible (~$0.08 per submit + finalize at 0.1 gwei).

### ExecutionEngine (`contracts/src/ExecutionEngine.sol`)

| Operation            | Gas Used | ETH @ 30 gwei | USD @ $2500/ETH | Notes                                      |
|----------------------|----------|----------------|-----------------|---------------------------------------------|
| `requestExecution`   | 199,344  | 0.00598        | $14.95          | 8-slot struct write + registry lookup       |
| `claimExecution`     | 80,722   | 0.00242        | $6.05           | 3 warm SSTOREs + event                      |
| `submitProof`        | 173,509  | 0.00521        | $13.01          | Verify + status + 2 ETH transfers + stats   |

**Full lifecycle (request + claim + submit): ~454K gas ($34.02 at 30 gwei)**

---

## 2. Optimization Opportunities

### 2.1 Storage Packing -- MLResult Struct -- COMPLETED

The `MLResult` struct in `ITEEMLVerifier.sol` (lines 66-81) has 14 fields and is what
`getResult()` returns. It is not what the contract stores: results are stored as
`PackedMLResult` (`TEEMLVerifier.sol` lines 67-88).

**`MLResult` as declared (not used for storage; it would take 12 slots):**
```
Slot N+0: enclave (address, 20 bytes) -- 12 bytes wasted
Slot N+1: submitter (address, 20 bytes) -- 12 bytes wasted
Slot N+2: modelHash (bytes32)
Slot N+3: inputHash (bytes32)
Slot N+4: resultHash (bytes32)
Slot N+5: result (bytes, dynamic -- offset pointer)
Slot N+6: submittedAt (uint256, 32 bytes)
Slot N+7: challengeDeadline (uint256)
Slot N+8: disputeDeadline (uint256)
Slot N+9: challengeBond (uint256)
Slot N+10: proverStakeAmount (uint256)
Slot N+11: finalized (bool) + challenged (bool) + challenger (address)
```

**Current storage layout, `PackedMLResult` (9 slots, from `forge inspect TEEMLVerifier storage-layout`):**
```
Slot N+0: enclave (address 20) + submittedAt (uint40 5) + challengeDeadline (uint40 5) + finalized (bool 1) + challenged (bool 1) = 32
Slot N+1: submitter (address 20) + disputeDeadline (uint40 5) = 25
Slot N+2: challenger (address 20)
Slot N+3: modelHash (bytes32)
Slot N+4: inputHash (bytes32)
Slot N+5: resultHash (bytes32)
Slot N+6: result (bytes dynamic)
Slot N+7: challengeBond (uint256)
Slot N+8: proverStakeAmount (uint256)
```

The packing is already reflected in the `submitResult` figure in section 1.

### 2.2 EnclaveInfo Struct Packing -- COMPLETED

`EnclaveInfo` in `ITEEMLVerifier.sol` (lines 44-49) is what `enclaves()` returns. Storage uses
`PackedEnclaveInfo` (`TEEMLVerifier.sol` lines 49-54):
```solidity
struct PackedEnclaveInfo {
    bool registered;          // 1 byte
    bool active;              // 1 byte
    uint40 registeredAt;      // 5 bytes
    bytes32 enclaveImageHash; // 32 bytes
}
```
`registered`, `active`, and `registeredAt` (as uint40) share one slot and `enclaveImageHash`
takes a second slot: 2 slots instead of the 3 that `EnclaveInfo` would take.

### 2.3 Custom Errors in TEEMLVerifier -- COMPLETED (Phase 74)

All 16 string-based `require()` calls in `TEEMLVerifier.sol` have been converted to custom errors
defined in `ITEEMLVerifier.sol`. Custom errors use only 4 bytes of selector data instead of
ABI-encoded string payloads, saving approximately:

- **~200 gas per revert** (no ABI string encoding overhead)
- **~500 bytes of deployment bytecode** (no string literals compiled in)

The GKR verifier libraries (`GKRVerifier.sol`, `GKRDAGVerifier.sol`) also received custom error
conversions in Phase 78.

Example of the completed conversion:
```solidity
// ITEEMLVerifier.sol -- custom error definition
error InsufficientStake();

// TEEMLVerifier.sol -- usage
if (msg.value < proverStake) revert InsufficientStake();
```

### 2.4 Duplicate Event Emissions (save ~1,500 gas per call)

**TEEMLVerifier.sol lines 214-215, 224-225:** `setChallengeBondAmount` and `setProverStake` each
emit TWO events -- a specific event (`ChallengeBondUpdated`/`ProverStakeUpdated`) and a generic
`ConfigUpdated`. Remove the generic event or remove the specific ones. Each event costs ~375-750 gas.

**TEEMLVerifier.sol lines 514-515:** `finalize()` emits both `ResultExpired` and `ResultFinalized`.
The `ResultExpired` event is redundant -- it conveys the same information as `ResultFinalized`.
Removing it saves ~750 gas per finalize.

### 2.5 Short-Circuit Storage Reads in `isResultValid` (minor)

`TEEMLVerifier.sol` lines 544-553: the mappings `disputeResolved` and `disputeProverWon` (lines
112-115) are separate, each costing 2,100 gas cold. Packing these into a single mapping
(`mapping(bytes32 => uint8)` with bitflags) would save one SLOAD (~2,100 gas).

### 2.6 Batch Operations

Neither contract offers batch variants. Adding these saves 21,000 gas (base tx cost) per extra op:
- `batchRegisterEnclaves(address[], bytes32[])` -- amortize cold storage access
- `batchSubmitResults(...)` -- amortize enclave registry lookups (cold 2,100 -> warm 100 gas)
- `batchClaimExecution(uint256[])` -- save 21K gas per extra claim

### 2.7 Indexed Event Parameters

`ResultChallenged` (`ITEEMLVerifier.sol` line 104) has `challenger` non-indexed. Indexing it adds
~375 gas per emit but enables `eth_getLogs` filtering. `ResultSubmitted` (line 97) already uses
the maximum 3 indexed params.

### 2.8 Cached Storage References in RemainderVerifier -- COMPLETED (Phase 77)

`RemainderVerifier.sol` circuit management functions (`deactivateCircuit`, `reactivateCircuit`,
`deactivateDAGCircuit`, `reactivateDAGCircuit`) now cache the mapping lookup into a local storage
reference instead of performing redundant SLOADs on the same storage slot:

```solidity
// Before -- two separate mapping reads (2 x 2,100 gas cold or 2 x 100 gas warm):
require(circuits[circuitHash].active, "...");
circuits[circuitHash].active = false;

// After -- one mapping read, reused via storage pointer:
CircuitConfig storage cfg = circuits[circuitHash];
require(cfg.active, "...");
cfg.active = false;
```

Savings: ~100-2,100 gas per function call depending on warm/cold state.

---

## 3. Comparison: TEE Path vs ZK Path

| Metric                   | TEE Happy Path      | ZK Dispute (DAG)     | Ratio       |
|--------------------------|---------------------|----------------------|-------------|
| Gas per inference        | ~304K               | ~198M (single-tx)    | 1:653       |
| Cost @ 0.1 gwei (Arb)   | $0.076              | $49.56               | 1:653       |
| Cost @ 30 gwei (L1)     | $22.78              | $14,868.72           | 1:653       |
| Transactions needed      | 2 (submit+finalize) | 1 or 15 (batched, plus an optional cleanup) | --          |
| Latency                  | 1 hour (window)     | ~7 min prove + 15 tx | --          |

**Expected cost with dispute rate d:** `304K + d * 198M` gas per inference.
At 0.1% dispute rate: ~502K gas ($0.13 at 0.1 gwei).

The ZK figure is `eth_estimateGas` for `verifyDAGProof` on the phase1a fixture, on a local Anvil chain
(198,249,628). The "Arb" and "L1" rows for the ZK path are price conversions only: `RemainderVerifier`
(135,888 bytes) is over the 24,576-byte contract size limit of Ethereum, Arbitrum and World Chain and
cannot be deployed there. In batched mode the largest of the 15 transactions uses 31.7M gas.

The TEE path is designed so ZK is only triggered during disputes. At dispute rates below
about 0.15% (304K / 198M), the TEE path dominates total cost; above that, the ZK share is larger.

---

## 4. Recommendations (Prioritized)

| Priority | Change | File:Line | Est. Savings | Effort |
|----------|--------|-----------|-------------|--------|
| **P0** | ~~Pack MLResult timestamps + bools with address~~ | `TEEMLVerifier.sol:67-88` (`PackedMLResult`, `uint40`) | 3 slots (12 -> 9) | **DONE** |
| **P1** | ~~Convert 16 string requires to custom errors~~ | `TEEMLVerifier.sol` | ~200 gas/revert, 500B deploy | **DONE** (Phase 74) |
| **P2** | ~~Pack EnclaveInfo (bool+bool+uint40 in one slot)~~ | `TEEMLVerifier.sol:49-54` (`PackedEnclaveInfo`) | 1 slot (3 -> 2) | **DONE** |
| **P3** | Remove duplicate events (ConfigUpdated or specific) | `TEEMLVerifier.sol:214-215,224-225` | ~750 gas/admin call | Trivial |
| **P4** | Remove redundant ResultExpired event | `TEEMLVerifier.sol:514` | ~750 gas/finalize | Trivial |
| **P5** | Merge disputeResolved+disputeProverWon mappings | `TEEMLVerifier.sol:112-115` | ~2,100 gas/query | Low |
| **P6** | Add batch operations (registerEnclaves, submitResults) | New functions | ~21K gas/extra op | Medium |

| **P7** | ~~Replace modExp loop with MODEXP precompile (0x05)~~ | `SumcheckVerifier.sol`, `CommittedSumcheckVerifier.sol` | ~7K gas/inverse call | **DONE** |

**P0 and P1 are already in the code and in the measured ~304K gas per submit+finalize cycle.
Of the happy-path items only P4 remains (~750 gas per finalize, estimated).**

**ZK verification path: MODEXP precompile (P7) saves ~7K gas per modular inverse, called
multiple times during sumcheck verification. Estimated 50-100K gas savings per full DAG proof.**

---

## Measuring Gas

```bash
cd contracts
forge test --match-contract GasProfileTest -vv         # Key operations (the figures in section 1)
forge test --gas-report --match-contract GasProfile   # Per-function report (implementation frame only; lower figures)
forge snapshot --match-contract GasProfile             # Regression tracking
forge test --match-test test_dag_batch_gas_per_batch -vv  # DAG batch gas (currently fails: asserts every step is under 30M)
```

## References

- `contracts/src/tee/TEEMLVerifier.sol`, `contracts/src/tee/ITEEMLVerifier.sol`
- `contracts/src/ExecutionEngine.sol`, `contracts/test/GasProfileTest.t.sol`
- `contracts/.gas-snapshot`, `contracts/src/remainder/DAGBatchVerifier.sol`
