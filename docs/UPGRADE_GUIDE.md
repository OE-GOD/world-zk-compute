# Contract Upgrade Guide

This document covers how to safely upgrade deployed World ZK Compute contracts, including the UUPS proxy-based `TEEMLVerifier` and `RemainderVerifier` and the contracts deployed without a proxy, which can only be replaced by a fresh deployment. The operational steps, with commands that were run on a local Anvil chain, are in [RUNBOOK.md section 6](RUNBOOK.md#6-contract-upgrade-procedure).

---

## 1. Current Architecture

### Upgradeable Contracts (UUPS Proxy)

| Contract | Proxy Pattern | Admin Mechanism | File |
|---|---|---|---|
| `TEEMLVerifier` | `UUPSProxy` + `UUPSUpgradeable` | EIP-1967 admin slot + `onlyTimelocked` | `contracts/src/tee/TEEMLVerifier.sol` |
| `RemainderVerifier` | `UUPSProxy` + `UUPSUpgradeable` | EIP-1967 admin slot + `onlyTimelocked` | `contracts/src/remainder/RemainderVerifier.sol` |
| `UpgradeableExecutionEngine` (not deployed by any script in the repository) | `UUPSProxy` + `UUPSUpgradeable` | EIP-1967 admin slot + `onlyTimelocked` | `contracts/src/Upgradeable.sol` |

The custom UUPS implementation uses EIP-1967 storage slots for both the implementation address and the admin address. The proxy delegates all calls to the implementation via `delegatecall`. Upgrades are authorized by the `onlyTimelocked` modifier on `upgradeTo()` and `upgradeToAndCall()`: while `timelock()` is the zero address only the admin can call them, and once a timelock is set only the timelock contract can. The `_authorizeUpgrade()` hook in these contracts only checks that the new implementation address has code; the base then requires the new implementation's `proxiableUUID()` to return the ERC-1967 implementation slot and reverts with `UpgradeToNonUUPS(address)` otherwise. This base's `proxiableUUID()` reverts when called through a proxy, so the address of a proxy that runs it, the proxy's own included, is refused too. A proxy on an older implementation (every proxy deployed before this check) answers `proxiableUUID()` through `delegatecall`, so its address is accepted and the upgrade leaves the proxy unusable; step 3 of [RUNBOOK.md section 6](RUNBOOK.md#6-contract-upgrade-procedure) catches it: its storage check of the new address's implementation slot for a proxy that keeps its implementation there, such as UUPSProxy, and its check of the address against the one deployed in step 2 in every case.

**EIP-1967 Slots:**
- Implementation: `0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc`
- Admin: `0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103`

Key base-contract functions:
- `upgradeTo(address newImplementation)` -- upgrade without post-upgrade call
- `upgradeToAndCall(address newImplementation, bytes data)` -- upgrade and atomically call an initializer
- `changeAdmin(address newAdmin)` -- transfer admin role (no 2-step; takes effect immediately)
- `setTimelock(address _timelock)` -- set, replace or clear (zero address) the timelock; admin only, takes effect immediately
- `implementation()` -- query current implementation address
- `admin()` -- query current admin address
- `timelock()` -- query current timelock address (zero address when none is set)

### Immutable Contracts (No Proxy)

| Contract | Access Control | Upgradeable References | File |
|---|---|---|---|
| `ExecutionEngine` | `Ownable2Step` + `Pausable` + `ReentrancyGuard` | `reputation` (mutable), `registry`/`verifier` (immutable) | `contracts/src/ExecutionEngine.sol` |
| `ProgramRegistry` | `Ownable2Step` + `Pausable` | none | `contracts/src/ProgramRegistry.sol` |
| `ProverRegistry` | `Ownable2Step` + `ReentrancyGuard` + `Pausable` | none | `contracts/src/ProverRegistry.sol` |
| `ProverReputation` | `Ownable2Step` + `Pausable` | none | `contracts/src/ProverReputation.sol` |

These use constructor-based initialization and cannot be upgraded in place. To "upgrade" them, deploy a new instance and update references in dependent contracts (e.g., `ExecutionEngine.setReputation(newAddress)`).

`TEEMLVerifier` and `RemainderVerifier` are not in this table: they have no `owner()`, `transferOwnership()` or `acceptOwnership()`. Their roles are `admin()` (one-step `changeAdmin(address)`) and the optional `timelock()`. Their own references stay mutable through the proxy: `TEEMLVerifier.setRemainderVerifier()` and, in `RemainderVerifier`, the circuit configs and the Groth16/Stylus verifier addresses.

**Important:** `ExecutionEngine` stores `registry` and `verifier` as `immutable`. Changing these requires deploying an entirely new `ExecutionEngine`.

### Libraries (Stateless)

`DAGBatchVerifier`, `GKRVerifier`, `GKRDAGVerifier`, `GKRHybridVerifier`, `GKRDAGHybridVerifier`, `HyraxVerifier`, `PoseidonSponge`, `SumcheckVerifier`, and `HyraxProofDecoder` are Solidity libraries. They have no independent storage, and their functions are `internal`, so they are compiled into the contract that imports them rather than deployed and linked separately. To upgrade library logic, deploy a new implementation of the contract that imports them (`RemainderVerifier`) and upgrade its proxy to it.

---

## 2. Upgrade Workflow

### UUPS Proxy Upgrade (TEEMLVerifier, RemainderVerifier)

```
Proxy (fixed address, holds all storage)
  |
  |-- delegatecall --> V1 Implementation  (before upgrade)
  |-- delegatecall --> V2 Implementation  (after upgradeTo)
```

**Steps:**

1. **Write the V2 implementation.** Inherit `UUPSUpgradeable`, replicate the exact V1 storage layout, and append new variables after the last existing one (`TEEMLVerifier` and `RemainderVerifier` declare no `__gap`).

2. **Test on fork.** Run `forge test --fork-url <RPC>` with an upgrade test that deploys V2, calls `upgradeTo`, and verifies state preservation.

3. **Deploy the new implementation only.** Do NOT deploy a new proxy.

4. **Call `upgradeTo()` or `upgradeToAndCall()`.** The current admin calls one of these on the proxy address; if a timelock is set (`timelock()` is not the zero address), the call has to come from the timelock instead. Use `upgradeToAndCall()` when V2 needs a one-time initializer.

5. **Verify.** Confirm `implementation()` returns the new address and that existing state (registered enclaves and results, or registered circuits) is intact.

The commands for each step are in [RUNBOOK.md section 6](RUNBOOK.md#6-contract-upgrade-procedure).

### Immutable Contract Upgrade

For `ExecutionEngine`, `ProgramRegistry`, `ProverRegistry` and `ProverReputation`:

1. Deploy the new version with its constructor.
2. Update references in dependent contracts:
   - `ExecutionEngine.setReputation(newReputationAddr)`
   - `ProverReputation.authorizeReporter(engine)` on a new `ProverReputation`; for a new `ExecutionEngine`, `ProverReputation.authorizeReporter(newEngine)` and `ProverRegistry.setSlasher(newEngine, true)`
   - `registry` and `verifier` are `immutable` in `ExecutionEngine`: a new `ProgramRegistry` or verifier needs a new `ExecutionEngine`
3. Update off-chain services (operator config, SDK endpoints) to the new address.
4. Pause the old contract if it should no longer accept traffic.

---

## 3. Pre-Upgrade Checklist

Run through every item before executing an upgrade on any non-devnet network.

- [ ] **Back up contract state.** Export critical mappings (requests, prover stats, enclave registrations) via archive node RPC or event log replay.
- [ ] **Test on Anvil fork.** Rehearse the upgrade against a fork of the live chain (RUNBOOK.md section 6, step 4; the repository ships no upgrade script). Verify all existing functionality works post-upgrade.
- [ ] **Verify storage layout compatibility.** Diff storage variable declarations between V1 and V2. No variable may be reordered, removed, or have its type changed.
- [ ] **Check contract size.** Run `forge build --sizes` and confirm the new implementation is under the 24,576-byte Spurious Dragon limit. `TEEMLVerifier` is (15,045 bytes of runtime bytecode). `RemainderVerifier` is not (136,142 bytes), so neither it nor a new implementation of it can be deployed on a chain that enforces the limit (Ethereum mainnet and Sepolia, Arbitrum One and Arbitrum Sepolia, World Chain and World Chain Sepolia).
- [ ] **Review the upgrade functions.** Confirm the new implementation still inherits `UUPSUpgradeable` from `contracts/src/Upgradeable.sol`: the access control is the `onlyTimelocked` modifier on `upgradeTo()` / `upgradeToAndCall()` there, and `_authorizeUpgrade()` in `TEEMLVerifier` and `RemainderVerifier` carries no modifier (it only checks that the new implementation has code). `upgradeTo()` refuses a contract without `proxiableUUID()`, a contract whose `proxiableUUID()` returns another value than the ERC-1967 implementation slot, and the address of a proxy whose implementation has this check (`UpgradeToNonUUPS`). It accepts the address of a proxy on an older implementation; RUNBOOK.md section 6, step 3, checks the new address's implementation slot for that, and checks the address against the one deployed in step 2. It does not check the upgrade functions themselves or the storage layout: after an upgrade to a contract without the upgrade functions the proxy can never be upgraded again.
- [ ] **Audit new external/public functions.** Any new functions are callable through the proxy immediately after upgrade.
- [ ] **Confirm admin key access.** The `upgradeTo()` call must come from the admin stored at EIP-1967 slot `0xb53127...`, or from the timelock contract if `timelock()` is set. Verify you control this key.
- [ ] **Pause if needed.** If the upgrade changes behavior of in-flight requests, call `pause()` before upgrading to prevent race conditions, then `unpause()` after.
- [ ] **Review events.** Ensure V2 does not remove or change the signature of existing events that off-chain indexers depend on.

---

## 4. Storage Layout Rules

The proxy's storage is shared across all implementation versions. Violating these rules corrupts live state with no recovery path.

### Rules

1. **Never reorder existing variables.** Slots are assigned sequentially by declaration order.
2. **Never remove a variable.** Replace it with a placeholder: `uint256 private __deprecated_oldVar;`.
3. **Never change a variable's type.** `uint256` to `address` reinterprets the slot.
4. **Append new variables after `__gap` or at the end.** Shrink the gap by the number of new slots consumed.
5. **Mappings and dynamic arrays are safe to append** because their data lives in hashed slots, not sequential ones.
6. **Constants and immutables are not stored on-chain** (they live in bytecode), so they do not affect layout.
7. **Struct changes are dangerous.** Changing a struct definition (field order, types, or count) that is stored in a mapping or array will corrupt existing entries.

### Current Layout: UpgradeableExecutionEngine V1

```
Slot  Variable                              Type
────  ────────────────────────────────────  ──────────────────
0     _initialized + _initializing + timelock  uint8 + bool + address (inherited from UUPSUpgradeable)
1     registry                              address
2     verifier                              address
3     protocolFeeBps                        uint256
4     feeRecipient                          address
5     nextRequestId                         uint256
6     requests                              mapping (hash-based, does not consume sequential slots)
7     proverCompletedCount                  mapping
8     proverEarnings                        mapping
9     paused                                bool
10    _reentrancyStatus                     uint256
11-58 __gap[48]                             uint256[48]
```

For the two deployed proxies, print the layout with `forge inspect src/tee/TEEMLVerifier.sol:TEEMLVerifier storage-layout` (or `src/remainder/RemainderVerifier.sol:RemainderVerifier`). Both start with the same slot 0 (`_initialized`, `_initializing`, `timelock`, then `_paused`) and declare no `__gap`.

Note: `VERSION` is a `constant` and is not stored. The EIP-1967 admin and implementation slots are at non-sequential addresses and do not collide with the layout above.

### Adding a Variable in V2

**Option A: Shrink the gap** (preferred when other contracts might inherit this one):

```solidity
// V1 layout preserved exactly
mapping(address => uint256) public proverEarnings;
bool public paused;
uint256 private _reentrancyStatus;

uint256[47] private __gap;  // was [48], now [47] -- freed one slot

uint256 public maxConcurrentClaims;  // new V2 variable in the freed slot
```

**Option B: Append after the gap** (simpler; the approach of the V2 mock in `contracts/test/UpgradeableTest.t.sol`, which declares its gap as `[50]`):

```solidity
uint256[48] private __gap;  // unchanged

uint256 public newV2Variable;  // appended past the gap
```

Both approaches are valid. Option A keeps total contract storage footprint predictable. Option B is simpler when you have many new variables.

---

## 5. Rollback Procedure

### UUPS Proxy Rollback

If a V2 upgrade introduces a bug:

1. **Record the old implementation address** before every upgrade (`implementation()`). The `Upgraded(address)` event is emitted automatically on every upgrade (not for the implementation the proxy was deployed with) -- query it from logs.

2. **Call `upgradeTo(oldImplementation)`.** This points the proxy back to V1. If a timelock is set, this call also has to be scheduled and executed through the timelock.

   ```bash
   cast send $PROXY "upgradeTo(address)" $OLD_IMPL \
     --private-key $ADMIN_KEY --rpc-url $RPC_URL
   ```

3. **Extra storage is harmless.** If V2 wrote to new slots, V1 simply ignores them (they are past V1's declared variables).

4. **Corrupted storage cannot be rolled back.** If V2 violated storage layout rules and overwrote existing slots, the damage is permanent. This is why Section 4 rules are non-negotiable.

5. **An implementation without the upgrade functions cannot be rolled back.** `upgradeTo()` only checks that the new address has code and that its `proxiableUUID()` returns the ERC-1967 implementation slot (which a proxy on an implementation with this check fails, and a proxy on an older implementation passes). If that contract does not have `upgradeTo()` itself, the proxy has no upgrade function any more and stays on it.

### Immutable Contract Rollback

1. Redeploy the previous contract version.
2. Re-register state (for example the programs in `ProgramRegistry`) by replaying admin transactions from event logs.
3. Update all references in dependent contracts and off-chain services.

### Emergency: Pause First

All core contracts support `pause()`. If you discover a vulnerability, pause immediately:

```bash
# Pause (owner/admin only)
cast send $CONTRACT "pause()" --private-key $ADMIN_KEY --rpc-url $RPC_URL
```

This blocks `submitResult`, `challenge`, `requestExecution`, `submitProof`, and `claimExecution` while you prepare the fix. On `TEEMLVerifier` it also blocks `finalize`, `resolveDispute*` and `resolveDisputeByTimeout`. `ExecutionEngine.cancelExecution` remains available so users can recover funds, and so does `TEEMLVerifier.extendDisputeWindow`.

---

## 6. Multi-Sig Upgrade Process (Production)

For production, the admin should be a multi-sig (e.g., Gnosis Safe) with a timelock. The timelock in this repository is `contracts/src/TimelockController.sol`. It is not the OpenZeppelin contract: it has a single `owner` who schedules and cancels operations, no proposer, executor or guardian roles, and once the delay has passed anyone can execute.

### Recommended Architecture

```
Gnosis Safe (3-of-5 multi-sig)
  |
  +--> TimelockController (48h minimum delay)
         |
         +--> UUPSProxy.upgradeTo(newImpl)
         +--> TEEMLVerifier.setRemainderVerifier(addr)
         +--> RemainderVerifier onlyTimelocked functions
```

### Step-by-Step

**1. Deploy `TimelockController`** with the multi-sig as `owner` and a 48-hour minimum delay (constructor arguments: `minDelay` in seconds, then the owner):

```bash
forge create src/TimelockController.sol:TimelockController \
  --rpc-url $RPC_URL --private-key $DEPLOYER_KEY --broadcast \
  --constructor-args 172800 $TIMELOCK_OWNER
```

**2. Put the timelock in front of the upgrade functions:**

```bash
# For the UUPS proxies (TEEMLVerifier, RemainderVerifier): set the timelock.
# The admin stays the admin (pause, enclave registration, circuit deactivation); upgradeTo,
# upgradeToAndCall and the other onlyTimelocked functions now only accept the timelock.
cast send $PROXY "setTimelock(address)" $TIMELOCK \
  --private-key $CURRENT_ADMIN --rpc-url $RPC_URL

# For Ownable2Step contracts (ExecutionEngine, ProgramRegistry, ProverRegistry,
# ProverReputation): two-step transfer
cast send $PROGRAM_REGISTRY "transferOwnership(address)" $TIMELOCK \
  --private-key $CURRENT_OWNER --rpc-url $RPC_URL
# Then the timelock itself has to call acceptOwnership(): schedule it, wait, execute
OP_ID=$(cast keccak "accept-ownership-program-registry")
cast send $TIMELOCK "schedule(bytes32,address,bytes,uint256)" \
  $OP_ID $PROGRAM_REGISTRY $(cast calldata "acceptOwnership()") 172800 \
  --private-key $TIMELOCK_OWNER_KEY --rpc-url $RPC_URL
# After the 48-hour delay (earlier, execute reverts with OperationNotReady); anyone can send this
cast send $TIMELOCK "execute(bytes32)" $OP_ID \
  --private-key $TIMELOCK_OWNER_KEY --rpc-url $RPC_URL
```

**3. Propose upgrade** (any multi-sig signer submits to Safe, Safe calls timelock). The first argument is an operation id of your choice (any `bytes32` not used before); the last is the delay in seconds, at least `minDelay()`:

```bash
OP_ID=$(cast keccak "upgrade-tee-verifier-1")
cast send $TIMELOCK "schedule(bytes32,address,bytes,uint256)" \
  $OP_ID \
  $PROXY \
  $(cast calldata "upgradeTo(address)" $NEW_IMPL) \
  172800 \
  --private-key $TIMELOCK_OWNER_KEY --rpc-url $RPC_URL
```

**4. Wait 48 hours.** Users and monitoring systems can inspect the pending operation.

**5. Execute** (anyone can send this once the delay has passed; before that it reverts with `OperationNotReady`):

```bash
cast send $TIMELOCK "execute(bytes32)" $OP_ID \
  --private-key $EXECUTOR_KEY --rpc-url $RPC_URL
```

**Key considerations:**
- The 48h delay gives users time to exit or raise concerns.
- For emergencies, `pause()` can be called immediately by the proxy's `admin()`: `setTimelock` does not change who the admin is, and `pause()` is `onlyAdmin` (separate from the timelocked upgrade path).
- `setTimelock(address)` is itself `onlyAdmin` and not delayed: the admin can replace or clear the timelock in one transaction.
- The repository's `TimelockController` has no `GUARDIAN_ROLE`: only its `owner` can `cancel(bytes32)` a pending operation. A guardian that can cancel without the full multi-sig quorum would need a change to that contract.

---

## 7. Forge Script Example for Upgrade

The repository ships no upgrade script. The example below is not a file in the repository; it was run as `script/Upgrade.s.sol` in a copy of `contracts/` against a `TEEMLVerifier` proxy deployed on a local Anvil chain with `contracts/script/DeployTEEMLVerifier.s.sol`. It covers a proxy with no timelock set (with one, the upgrade is scheduled through the timelock, see Section 6). The repository has no second version of `TEEMLVerifier`, so the example deploys the current contract again as the new implementation:

```solidity
// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/tee/TEEMLVerifier.sol";

contract UpgradeScript is Script {
    function run() external {
        uint256 adminKey = vm.envUint("PRIVATE_KEY");
        address proxy = vm.envAddress("PROXY_ADDRESS");
        TEEMLVerifier tee = TEEMLVerifier(payable(proxy));

        // Values to compare after the upgrade
        address adminBefore = tee.admin();
        address remainderBefore = tee.remainderVerifier();
        require(adminBefore == vm.addr(adminKey), "PRIVATE_KEY is not the proxy admin");
        require(tee.timelock() == address(0), "timelock is set: schedule upgradeTo through it");

        vm.startBroadcast(adminKey);

        // 1. Deploy the new implementation (put your new version here)
        TEEMLVerifier newImpl = new TEEMLVerifier();
        console.log("New implementation deployed at:", address(newImpl));

        // 2. Upgrade
        tee.upgradeTo(address(newImpl));

        vm.stopBroadcast();

        // 3. Verify
        require(tee.implementation() == address(newImpl), "Implementation mismatch");
        require(tee.admin() == adminBefore, "admin changed");
        require(tee.remainderVerifier() == remainderBefore, "remainderVerifier changed");
        console.log("Upgrade verified. New impl:", tee.implementation());
    }
}
```

### Run Commands

```bash
# Dry run on fork (no state changes, validates the script)
PRIVATE_KEY=$ADMIN_KEY PROXY_ADDRESS=$PROXY \
  forge script script/Upgrade.s.sol:UpgradeScript \
  --fork-url $RPC_URL -vvv

# Live broadcast
PRIVATE_KEY=$ADMIN_KEY PROXY_ADDRESS=$PROXY \
  forge script script/Upgrade.s.sol:UpgradeScript \
  --rpc-url $RPC_URL --broadcast -vvv
```

---

## 8. Storage Gaps in Current Contracts

### What Are Storage Gaps?

Storage gaps are reserved `uint256` arrays that hold empty slots for future variables. When you add a new state variable in a V2 upgrade, you shrink the gap by the same number of slots, keeping the total layout size constant.

### Where Gaps Are Used

| Contract | Gap Declaration | File | Line |
|---|---|---|---|
| `UpgradeableExecutionEngine` | `uint256[48] private __gap;` | `contracts/src/Upgradeable.sol` | 329 |
| `RemainderVerifierUpgradeable` | `uint256[49] private __gap;` | `contracts/src/remainder/RemainderVerifierUpgradeable.sol` | 163 |

These are the only two contracts in `contracts/src` that declare storage gaps, and no deploy script deploys either of them. `TEEMLVerifier` and `RemainderVerifier`, the two contracts the deploy scripts put behind a `UUPSProxy`, declare no gap: new variables in a later version go after their last existing variable.

### Where Gaps Are NOT Needed

The immutable contracts (`ExecutionEngine`, `ProgramRegistry`, `ProverRegistry`, `ProverReputation`) do not use storage gaps. They are deployed directly (no proxy) and cannot be upgraded in place. Adding a gap to these contracts would serve no purpose.

### Recommendation: If You Proxy-ify More Contracts

`TEEMLVerifier` and `RemainderVerifier` are already UUPS implementations. If you plan to put another contract (for example `ExecutionEngine`) behind a UUPS proxy in the future, you must:

1. Create an upgradeable version that inherits `UUPSUpgradeable`.
2. Replace the constructor with an `initialize()` function using the `initializer` modifier.
3. Add a `uint256[50] private __gap;` at the end of the storage declarations.
4. Convert `immutable` references to regular storage variables (immutables are set in the constructor, which is not called via proxy). The UUPS base's own `__self` stays immutable on purpose: it holds the implementation's own address so that `proxiableUUID()` can refuse a call through a proxy, and as a storage variable it would read the proxy's storage instead.

The pattern as `TEEMLVerifier` implements it (excerpt from `contracts/src/tee/TEEMLVerifier.sol`; it declares no `__gap`):

```solidity
contract TEEMLVerifier is ITEEMLVerifier, UUPSUpgradeable, Pausable, ReentrancyGuard {
    // ... storage ...

    // The implementation's own initialize() reverts with AlreadyInitialized; the proxy initializes through delegatecall.
    constructor() {
        _initialized = type(uint8).max;
    }

    function initialize(address _admin, address _remainderVerifier) external initializer {
        if (_remainderVerifier == address(0)) revert ZeroAddress();
        _setAdmin(_admin);
        remainderVerifier = _remainderVerifier;
        // Set default values explicitly (storage defaults don't apply behind a proxy)
        challengeWindow = 1 hours;
        disputeWindow = 24 hours;
        challengeBondAmount = 0.1 ether;
        proverStake = 0.1 ether;
        _cachedChainId = block.chainid;
        _cachedDomainSeparator = _computeDomainSeparator();
    }

    /// @notice Authorization hook for upgrades
    function _authorizeUpgrade(address newImplementation) internal override {
        if (newImplementation.code.length == 0) revert InvalidImplementation();
    }
}
```

---

## 9. Version Tracking

### On-Chain Version

`UpgradeableExecutionEngine` declares a compile-time constant:

```solidity
uint256 public constant VERSION = 1;  // bump to 2, 3, ... per release
```

Query it any time:

```bash
cast call $PROXY "VERSION()(uint256)" --rpc-url $RPC_URL
```

`TEEMLVerifier` and `RemainderVerifier` declare no `VERSION`, so on their proxies this call reverts. For them the implementation address is the on-chain version marker:

```bash
cast call $PROXY "implementation()(address)" --rpc-url $RPC_URL
```

The `Upgraded(address indexed implementation)` event is emitted on every upgrade. Query the full upgrade history:

```bash
cast logs --from-block 0 --address $PROXY \
  "Upgraded(address indexed)" --rpc-url $RPC_URL
```

### Deployment Registry (Off-Chain)

Maintain a `deployments/` directory with per-chain JSON files. The files that exist today are described in `deployments/README.md`; `deployments/registry.json` has an `address` and an `implementationAddress` field per contract (all currently empty) and no upgrade history. A record that keeps the history could look like this:

```json
{
  "chainId": 31337,
  "network": "localhost",
  "contracts": {
    "TEEMLVerifier": {
      "proxy": "0x1234...abcd",
      "admin": "0x...",
      "remainderVerifier": "0x9abc...ijkl",
      "implementations": [
        {
          "version": 1,
          "address": "0xaaaa...1111",
          "deployedAt": "2025-03-01T00:00:00Z",
          "txHash": "0x..."
        },
        {
          "version": 2,
          "address": "0xbbbb...2222",
          "deployedAt": "2025-06-15T00:00:00Z",
          "txHash": "0x...",
          "upgradeCalldata": "upgradeTo(0xbbbb...2222)"
        }
      ]
    },
    "RemainderVerifier": {
      "proxy": "0x9abc...ijkl",
      "deployedAt": "2025-03-01T00:00:00Z",
      "admin": "0x...",
      "registeredCircuits": ["0xf85c..."]
    }
  }
}
```

Update this file as part of every deployment or upgrade. The `DeployFullStack.s.sol` script (`contracts/script/DeployFullStack.s.sol`) already logs all addresses; extend it to write JSON output for automated tracking.

### Version Bump Checklist

When releasing a new implementation version:

1. Increment the `VERSION` constant in the new implementation contract, if it declares one (`TEEMLVerifier` and `RemainderVerifier` do not).
2. Update `deployments/<chain>.json` with the new implementation address and tx hash.
3. Tag the git commit: `git tag contracts-v2.0.0`.
4. Run the full test suite against the forked upgrade: `forge test --fork-url $RPC -vvv`.
5. Verify the new implementation's source code on Etherscan/Arbiscan with `forge verify-contract`.
