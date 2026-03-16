# Contract Gas Optimization Report

Measured with `forge test --gas-report --match-contract GasProfile` on Solidity 0.8.20.

---

## 1. Current Gas Costs

### TEEMLVerifier (`contracts/src/tee/TEEMLVerifier.sol`)

| Operation              | Gas Used | ETH @ 30 gwei | USD @ $2500/ETH | Notes                                      |
|------------------------|----------|----------------|-----------------|---------------------------------------------|
| `submitResult`         | 245,614  | 0.00737        | $0.018          | ECDSA recover + 14-field struct cold write  |
| `challenge`            | 98,663   | 0.00296        | $0.007          | 4 warm SSTOREs + event                      |
| `finalize`             | 64,640   | 0.00194        | $0.005          | 1 SSTORE + ETH transfer + 2 events         |
| `resolveDisputeByTimeout` | 71,636 | 0.00215       | $0.005          | 3 SSTOREs + ETH transfer + event           |
| `registerEnclave`      | 92,859   | 0.00279        | $0.007          | Cold SSTORE for 4-field EnclaveInfo struct  |
| `revokeEnclave`        | 30,639   | 0.00092        | $0.002          | Single SSTORE (active = false)              |

**Happy path total (submit + finalize): ~310K gas ($0.023 at 30 gwei L1, $0.00008 at 0.1 gwei Arbitrum)**

### ExecutionEngine (`contracts/src/ExecutionEngine.sol`)

| Operation            | Gas Used | ETH @ 30 gwei | USD @ $2500/ETH | Notes                                      |
|----------------------|----------|----------------|-----------------|---------------------------------------------|
| `requestExecution`   | 180,766  | 0.00542        | $0.014          | 8-slot struct write + registry lookup       |
| `claimExecution`     | 77,285   | 0.00232        | $0.006          | 3 warm SSTOREs + event                      |
| `submitProof`        | 169,676  | 0.00509        | $0.013          | Verify + status + 2 ETH transfers + stats   |

**Full lifecycle (request + claim + submit): ~428K gas ($0.032 at 30 gwei)**

---

## 2. Optimization Opportunities

### 2.1 Storage Packing -- MLResult Struct (save ~40K gas per submitResult)

The `MLResult` struct in `ITEEMLVerifier.sol` (lines 35-50) uses 14 fields across 10+ storage
slots. Several fields can share slots:

**Current layout (unpacked):**
```
Slot N+0: enclave (address, 20 bytes) -- 12 bytes wasted
Slot N+1: submitter (address, 20 bytes) -- 12 bytes wasted
Slot N+2: modelHash (bytes32)
Slot N+3: inputHash (bytes32)
Slot N+4: resultHash (bytes32)
Slot N+5: result (bytes, dynamic -- offset pointer)
Slot N+6: submittedAt (uint256, 32 bytes) -- only needs uint48
Slot N+7: challengeDeadline (uint256) -- only needs uint48
Slot N+8: disputeDeadline (uint256) -- only needs uint48
Slot N+9: challengeBond (uint256)
Slot N+10: proverStakeAmount (uint256)
Slot N+11: finalized (bool) + challenged (bool) + challenger (address) -- may pack
```

**Proposed packed layout (saves 2 slots = ~40K gas on cold writes):**
```
Slot N+0: enclave (address 20) + submittedAt (uint48 6) + challengeDeadline (uint48 6) = 32
Slot N+1: submitter (address 20) + disputeDeadline (uint48 6) + finalized (bool 1) + challenged (bool 1) = 28
Slot N+2: challenger (address 20) -- or pack with flags above
Slot N+3: modelHash (bytes32)
Slot N+4: inputHash (bytes32)
Slot N+5: resultHash (bytes32)
Slot N+6: result (bytes dynamic)
Slot N+7: challengeBond (uint256)
Slot N+8: proverStakeAmount (uint256)
```

This reduces cold SSTOREs from ~11 to ~9, saving ~40,000 gas (2 x 20,000 gas per new slot).

### 2.2 EnclaveInfo Struct Packing (save ~20K gas per registerEnclave)

`ITEEMLVerifier.sol` lines 13-18:
```solidity
struct EnclaveInfo {
    bool registered;      // 1 byte
    bool active;          // 1 byte
    bytes32 enclaveImageHash; // 32 bytes
    uint256 registeredAt; // 32 bytes -- only needs uint48
}
```
Pack `registered`, `active`, and `registeredAt` (as uint48) into one slot with `enclaveImageHash`
in a second slot. Saves 1 cold SSTORE (~20K gas).

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

**TEEMLVerifier.sol lines 100-101, 110-111:** `setChallengeBondAmount` and `setProverStake` each
emit TWO events -- a specific event (`ChallengeBondUpdated`/`ProverStakeUpdated`) and a generic
`ConfigUpdated`. Remove the generic event or remove the specific ones. Each event costs ~375-750 gas.

**TEEMLVerifier.sol lines 266-267:** `finalize()` emits both `ResultExpired` and `ResultFinalized`.
The `ResultExpired` event is redundant -- it conveys the same information as `ResultFinalized`.
Removing it saves ~750 gas per finalize.

### 2.5 Short-Circuit Storage Reads in `isResultValid` (minor)

`TEEMLVerifier.sol` lines 280-289: the mappings `disputeResolved` and `disputeProverWon` (lines
42-44) are separate, each costing 2,100 gas cold. Packing these into a single mapping
(`mapping(bytes32 => uint8)` with bitflags) would save one SLOAD (~2,100 gas).

### 2.6 Batch Operations

Neither contract offers batch variants. Adding these saves 21,000 gas (base tx cost) per extra op:
- `batchRegisterEnclaves(address[], bytes32[])` -- amortize cold storage access
- `batchSubmitResults(...)` -- amortize enclave registry lookups (cold 2,100 -> warm 100 gas)
- `batchClaimExecution(uint256[])` -- save 21K gas per extra claim

### 2.7 Indexed Event Parameters

`ResultChallenged` (`ITEEMLVerifier.sol` line 73) has `challenger` non-indexed. Indexing it adds
~375 gas per emit but enables `eth_getLogs` filtering. `ResultSubmitted` (line 66) already uses
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
| Gas per inference        | ~310K               | ~213M (single-tx)    | 1:687       |
| Cost @ 0.1 gwei (Arb)   | $0.00008            | $0.053               | 1:663       |
| Cost @ 30 gwei (L1)     | $0.023              | $15.98               | 1:695       |
| Transactions needed      | 2 (submit+finalize) | 1 or 15 (batched)    | --          |
| Latency                  | 1 hour (window)     | ~7 min prove + 15 tx | --          |

**Expected cost with dispute rate d:** `310K + d * 213M` gas per inference.
At 0.1% dispute rate: ~523K gas ($0.00013 on Arbitrum).

The TEE path is designed so ZK is only triggered during disputes. At any realistic dispute
rate (<1%), the TEE path dominates total cost.

---

## 4. Recommendations (Prioritized)

| Priority | Change | File:Line | Est. Savings | Effort |
|----------|--------|-----------|-------------|--------|
| **P0** | Pack MLResult timestamps as uint48 + bools with address | `ITEEMLVerifier.sol:35-50` | ~40K gas/submit | Medium |
| **P1** | ~~Convert 16 string requires to custom errors~~ | `TEEMLVerifier.sol` | ~200 gas/revert, 500B deploy | **DONE** (Phase 74) |
| **P2** | Pack EnclaveInfo (bool+bool+uint48 in one slot) | `ITEEMLVerifier.sol:13-18` | ~20K gas/register | Low |
| **P3** | Remove duplicate events (ConfigUpdated or specific) | `TEEMLVerifier.sol:100-101,110-111` | ~750 gas/admin call | Trivial |
| **P4** | Remove redundant ResultExpired event | `TEEMLVerifier.sol:266` | ~750 gas/finalize | Trivial |
| **P5** | Merge disputeResolved+disputeProverWon mappings | `TEEMLVerifier.sol:42-44` | ~2,100 gas/query | Low |
| **P6** | Add batch operations (registerEnclaves, submitResults) | New functions | ~21K gas/extra op | Medium |

**Total potential savings on happy path (P0+P1+P4): ~41K gas per submit+finalize cycle,
reducing from ~310K to ~269K gas (13% reduction).**

---

## Measuring Gas

```bash
cd contracts
forge test --gas-report --match-contract GasProfile   # Key operations
forge snapshot --match-contract GasProfile             # Regression tracking
forge test --match-test test_dag_batch_gas_per_batch -vv  # DAG batch gas
```

## References

- `contracts/src/tee/TEEMLVerifier.sol`, `contracts/src/tee/ITEEMLVerifier.sol`
- `contracts/src/ExecutionEngine.sol`, `contracts/test/GasProfileTest.t.sol`
- `contracts/.gas-snapshot`, `contracts/src/remainder/DAGBatchVerifier.sol`
