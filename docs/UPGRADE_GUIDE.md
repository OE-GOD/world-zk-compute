# Contract Upgrade Guide

This document covers how to safely upgrade deployed World ZK Compute contracts, including the UUPS proxy-based `UpgradeableExecutionEngine` and the immutable contracts that use reference-swapping for upgrades.

---

## 1. Current Architecture

### Upgradeable Contracts (UUPS Proxy)

| Contract | Proxy Pattern | Admin Mechanism | File |
|---|---|---|---|
| `UpgradeableExecutionEngine` | `UUPSProxy` + `UUPSUpgradeable` | EIP-1967 admin slot | `contracts/src/Upgradeable.sol` |
| `RemainderVerifier` | `UUPSProxy` + `UUPSUpgradeable` | EIP-1967 admin slot + `onlyTimelocked` | `contracts/src/remainder/RemainderVerifier.sol` |

The custom UUPS implementation uses EIP-1967 storage slots for both the implementation address and the admin address. The proxy delegates all calls to the implementation via `delegatecall`. Upgrades are authorized through the `onlyAdmin` modifier and the `_authorizeUpgrade()` hook.

**EIP-1967 Slots:**
- Implementation: `0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc`
- Admin: `0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103`

Key base-contract functions:
- `upgradeTo(address newImplementation)` -- upgrade without post-upgrade call
- `upgradeToAndCall(address newImplementation, bytes data)` -- upgrade and atomically call an initializer
- `changeAdmin(address newAdmin)` -- transfer admin role (no 2-step; takes effect immediately)
- `implementation()` -- query current implementation address
- `admin()` -- query current admin address

### Immutable Contracts (No Proxy)

| Contract | Access Control | Upgradeable References | File |
|---|---|---|---|
| `ExecutionEngine` | `Ownable2Step` + `Pausable` + `ReentrancyGuard` | `reputation` (mutable), `registry`/`verifier` (immutable) | `contracts/src/ExecutionEngine.sol` |
| `TEEMLVerifier` | `Ownable2Step` + `Pausable` + `ReentrancyGuard` | `remainderVerifier` (mutable via `setRemainderVerifier()`) | `contracts/src/tee/TEEMLVerifier.sol` |
| `RemainderVerifier` | `Ownable2Step` + `Pausable` | circuit configs, Groth16/Stylus verifier addresses (all mutable) | `contracts/src/remainder/RemainderVerifier.sol` |
| `ProgramRegistry` | `Ownable2Step` + `Pausable` | none | `contracts/src/ProgramRegistry.sol` |
| `ProverRegistry` | `Ownable2Step` + `ReentrancyGuard` + `Pausable` | none | `contracts/src/ProverRegistry.sol` |
| `ProverReputation` | `Ownable2Step` + `Pausable` | none | `contracts/src/ProverReputation.sol` |

These use constructor-based initialization and cannot be upgraded in place. To "upgrade" them, deploy a new instance and update references in dependent contracts (e.g., `TEEMLVerifier.setRemainderVerifier(newAddress)`).

**Important:** `ExecutionEngine` stores `registry` and `verifier` as `immutable`. Changing these requires deploying an entirely new `ExecutionEngine`.

### Libraries (Stateless)

`DAGBatchVerifier`, `GKRVerifier`, `GKRDAGVerifier`, `GKRHybridVerifier`, `GKRDAGHybridVerifier`, `HyraxVerifier`, `PoseidonSponge`, `SumcheckVerifier`, and `HyraxProofDecoder` are Solidity libraries. They have no independent storage. To upgrade library logic, redeploy the contract that imports them.

---

## 2. Upgrade Workflow

### UUPS Proxy Upgrade (UpgradeableExecutionEngine)

```
Proxy (fixed address, holds all storage)
  |
  |-- delegatecall --> V1 Implementation  (before upgrade)
  |-- delegatecall --> V2 Implementation  (after upgradeTo)
```

**Steps:**

1. **Write the V2 implementation.** Inherit `UUPSUpgradeable`, replicate the exact V1 storage layout, and append new variables after `__gap`.

2. **Test on fork.** Run `forge test --fork-url <RPC>` with an upgrade test that deploys V2, calls `upgradeTo`, and verifies state preservation.

3. **Deploy the new implementation only.** Do NOT deploy a new proxy.

4. **Call `upgradeTo()` or `upgradeToAndCall()`.** The current admin calls one of these on the proxy address. Use `upgradeToAndCall()` when V2 needs a one-time initializer.

5. **Verify.** Confirm `implementation()` returns the new address and that existing state (requests, prover stats, fees) is intact.

### Immutable Contract Upgrade

For `TEEMLVerifier`, `RemainderVerifier`, etc.:

1. Deploy the new version with its constructor.
2. Update references in dependent contracts:
   - `TEEMLVerifier.setRemainderVerifier(newRemainderAddr)`
   - `RemainderVerifier` -- re-register circuits, Groth16 verifiers, Stylus verifiers
3. Update off-chain services (operator config, SDK endpoints) to the new address.
4. Pause the old contract if it should no longer accept traffic.

---

## 3. Pre-Upgrade Checklist

Run through every item before executing an upgrade on any non-devnet network.

- [ ] **Back up contract state.** Export critical mappings (requests, prover stats, enclave registrations) via archive node RPC or event log replay.
- [ ] **Test on Anvil fork.** Run the upgrade script with `--fork-url` against a fork of the live chain. Verify all existing functionality works post-upgrade.
- [ ] **Verify storage layout compatibility.** Diff storage variable declarations between V1 and V2. No variable may be reordered, removed, or have its type changed.
- [ ] **Check contract size.** Run `forge build --sizes` and confirm the new implementation is under the 24,576-byte Spurious Dragon limit.
- [ ] **Review `_authorizeUpgrade()`.** Confirm it includes `onlyAdmin` in the new implementation. A missing modifier lets anyone upgrade the proxy.
- [ ] **Audit new external/public functions.** Any new functions are callable through the proxy immediately after upgrade.
- [ ] **Confirm admin key access.** The `upgradeTo()` call must come from the admin stored at EIP-1967 slot `0xb53127...`. Verify you control this key.
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
0     _initialized + _initializing          uint8 + bool (inherited from UUPSUpgradeable)
1     registry                              address
2     verifier                              address
3     protocolFeeBps                        uint256
4     feeRecipient                          address
5     nextRequestId                         uint256
6     requests                              mapping (hash-based, does not consume sequential slots)
7     proverCompletedCount                  mapping
8     proverEarnings                        mapping
9-58  __gap[50]                             uint256[50]
```

Note: `VERSION` is a `constant` and is not stored. The EIP-1967 admin and implementation slots are at non-sequential addresses and do not collide with the layout above.

### Adding a Variable in V2

**Option A: Shrink the gap** (preferred when other contracts might inherit this one):

```solidity
// V1 layout preserved exactly
mapping(address => uint256) public proverEarnings;

uint256[49] private __gap;  // was [50], now [49] -- freed one slot

uint256 public maxConcurrentClaims;  // new V2 variable in the freed slot
```

**Option B: Append after the gap** (simpler, used in the test suite):

```solidity
uint256[50] private __gap;  // unchanged

uint256 public newV2Variable;  // appended past the gap
```

Both approaches are valid. Option A keeps total contract storage footprint predictable. Option B is simpler when you have many new variables.

---

## 5. Rollback Procedure

### UUPS Proxy Rollback

If a V2 upgrade introduces a bug:

1. **Record the old implementation address** before every upgrade. The `Upgraded(address)` event is emitted automatically -- query it from logs.

2. **Call `upgradeTo(oldImplementation)`.** This points the proxy back to V1.

   ```bash
   cast send $PROXY "upgradeTo(address)" $OLD_IMPL \
     --private-key $ADMIN_KEY --rpc-url $RPC_URL
   ```

3. **Extra storage is harmless.** If V2 wrote to new slots, V1 simply ignores them (they are past V1's declared variables).

4. **Corrupted storage cannot be rolled back.** If V2 violated storage layout rules and overwrote existing slots, the damage is permanent. This is why Section 4 rules are non-negotiable.

### Immutable Contract Rollback

1. Redeploy the previous contract version.
2. Re-register all state (circuits, enclaves, programs) by replaying admin transactions from event logs.
3. Update all references in dependent contracts and off-chain services.

### Emergency: Pause First

All core contracts support `pause()`. If you discover a vulnerability, pause immediately:

```bash
# Pause (owner/admin only)
cast send $CONTRACT "pause()" --private-key $ADMIN_KEY --rpc-url $RPC_URL
```

This blocks `submitResult`, `challenge`, `requestExecution`, `submitProof`, and `claimExecution` while you prepare the fix. Functions like `cancelExecution` and `finalize` remain available so users can recover funds.

---

## 6. Multi-Sig Upgrade Process (Production)

For production, the admin should be a multi-sig (e.g., Gnosis Safe) with a timelock.

### Recommended Architecture

```
Gnosis Safe (3-of-5 multi-sig)
  |
  +--> TimelockController (48h minimum delay)
         |
         +--> UUPSProxy.upgradeTo(newImpl)
         +--> TEEMLVerifier.setRemainderVerifier(addr)
         +--> RemainderVerifier owner functions
```

### Step-by-Step

**1. Deploy `TimelockController`** with the multi-sig as proposer and executor, 48-hour minimum delay.

**2. Transfer admin to the timelock:**

```bash
# For UUPS proxy
cast send $PROXY "changeAdmin(address)" $TIMELOCK \
  --private-key $CURRENT_ADMIN --rpc-url $RPC_URL

# For Ownable2Step contracts (two-step transfer)
cast send $TEE_VERIFIER "transferOwnership(address)" $TIMELOCK \
  --private-key $CURRENT_OWNER --rpc-url $RPC_URL
# Then from timelock:
cast send $TEE_VERIFIER "acceptOwnership()" \
  --private-key $TIMELOCK_EXECUTOR --rpc-url $RPC_URL
```

**3. Propose upgrade** (any multi-sig signer submits to Safe, Safe calls timelock):

```bash
cast send $TIMELOCK "schedule(address,uint256,bytes,bytes32,bytes32,uint256)" \
  $PROXY \
  0 \
  $(cast calldata "upgradeTo(address)" $NEW_IMPL) \
  0x0000000000000000000000000000000000000000000000000000000000000000 \
  0x0000000000000000000000000000000000000000000000000000000000000000 \
  172800 \
  --private-key $PROPOSER_KEY --rpc-url $RPC_URL
```

**4. Wait 48 hours.** Users and monitoring systems can inspect the pending operation.

**5. Execute:**

```bash
cast send $TIMELOCK "execute(address,uint256,bytes,bytes32,bytes32)" \
  $PROXY \
  0 \
  $(cast calldata "upgradeTo(address)" $NEW_IMPL) \
  0x0000000000000000000000000000000000000000000000000000000000000000 \
  0x0000000000000000000000000000000000000000000000000000000000000000 \
  --private-key $EXECUTOR_KEY --rpc-url $RPC_URL
```

**Key considerations:**
- The 48h delay gives users time to exit or raise concerns.
- For emergencies, `pause()` can be called immediately if the Safe is the direct owner of the pausable contract (separate from the timelocked upgrade path).
- Add a `GUARDIAN_ROLE` on the timelock that can cancel pending operations without the full multi-sig quorum.

---

## 7. Forge Script Example for Upgrade

Create `contracts/script/Upgrade.s.sol`:

```solidity
// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/Upgradeable.sol";

contract UpgradeableExecutionEngineV2 is UUPSUpgradeable {
    // ---- V1 storage layout (MUST be identical) ----
    uint256 public constant VERSION = 2;
    address public registry;
    address public verifier;
    uint256 public protocolFeeBps;
    address public feeRecipient;
    uint256 public nextRequestId;

    struct ExecutionRequest {
        uint256 id;
        bytes32 imageId;
        bytes32 inputDigest;
        address requester;
        uint48 createdAt;
        uint48 expiresAt;
        address callbackContract;
        uint8 status;
        address claimedBy;
        uint48 claimDeadline;
        uint256 tip;
    }

    mapping(uint256 => ExecutionRequest) public requests;
    mapping(address => uint256) public proverCompletedCount;
    mapping(address => uint256) public proverEarnings;
    uint256[50] private __gap;

    // ---- V2 additions (after gap) ----
    uint256 public maxConcurrentClaims;

    event Initialized(uint256 version);

    function initializeV2(uint256 _maxClaims) external {
        maxConcurrentClaims = _maxClaims;
        emit Initialized(VERSION);
    }

    function _authorizeUpgrade(address) internal override onlyAdmin {}

    // ... carry forward all V1 functions, add new ones ...
}

contract UpgradeScript is Script {
    function run() external {
        uint256 adminKey = vm.envUint("PRIVATE_KEY");
        address proxy = vm.envAddress("PROXY_ADDRESS");

        vm.startBroadcast(adminKey);

        // 1. Deploy new implementation
        UpgradeableExecutionEngineV2 newImpl = new UpgradeableExecutionEngineV2();
        console.log("New implementation deployed at:", address(newImpl));

        // 2. Upgrade + initialize V2 atomically
        bytes memory initData = abi.encodeCall(
            UpgradeableExecutionEngineV2.initializeV2, (5)
        );
        UpgradeableExecutionEngine(payable(proxy)).upgradeToAndCall(
            address(newImpl), initData
        );

        // 3. Verify
        address implAfter = UpgradeableExecutionEngine(payable(proxy)).implementation();
        require(implAfter == address(newImpl), "Implementation mismatch");
        console.log("Upgrade verified. New impl:", implAfter);

        // 4. Verify state preservation
        uint256 nextId = UpgradeableExecutionEngine(payable(proxy)).nextRequestId();
        console.log("nextRequestId preserved:", nextId);

        vm.stopBroadcast();
    }
}
```

### Run Commands

```bash
# Dry run on fork (no state changes, validates the script)
PRIVATE_KEY=$ADMIN_KEY PROXY_ADDRESS=$PROXY \
  forge script script/Upgrade.s.sol:UpgradeScript \
  --fork-url $RPC_URL -vvv

# Live broadcast with Etherscan verification
PRIVATE_KEY=$ADMIN_KEY PROXY_ADDRESS=$PROXY \
  forge script script/Upgrade.s.sol:UpgradeScript \
  --rpc-url $RPC_URL --broadcast --verify -vvv
```

---

## 8. Storage Gaps in Current Contracts

### What Are Storage Gaps?

Storage gaps are reserved `uint256` arrays that hold empty slots for future variables. When you add a new state variable in a V2 upgrade, you shrink the gap by the same number of slots, keeping the total layout size constant.

### Where Gaps Are Used

| Contract | Gap Declaration | File | Line |
|---|---|---|---|
| `UpgradeableExecutionEngine` | `uint256[50] private __gap;` | `contracts/src/Upgradeable.sol` | 266 |

This is the **only** contract in the codebase that uses storage gaps, because it is the only contract deployed behind a UUPS proxy.

### Where Gaps Are NOT Needed

The immutable contracts (`ExecutionEngine`, `TEEMLVerifier`, `RemainderVerifier`, `ProgramRegistry`, `ProverRegistry`) do not use storage gaps. They are deployed directly (no proxy) and cannot be upgraded in place. Adding a gap to these contracts would serve no purpose.

### Recommendation: If You Proxy-ify More Contracts

If you plan to put `TEEMLVerifier` or `RemainderVerifier` behind a UUPS proxy in the future, you must:

1. Create an upgradeable version that inherits `UUPSUpgradeable`.
2. Replace the constructor with an `initialize()` function using the `initializer` modifier.
3. Add a `uint256[50] private __gap;` at the end of the storage declarations.
4. Convert `immutable` references to regular storage variables (immutables are set in the constructor, which is not called via proxy).

Example pattern for a hypothetical upgradeable TEEMLVerifier:

```solidity
contract UpgradeableTEEMLVerifier is UUPSUpgradeable {
    address public remainderVerifier;
    uint256 public challengeBondAmount;
    uint256 public proverStake;
    mapping(address => EnclaveInfo) public enclaves;
    mapping(bytes32 => MLResult) internal _results;
    mapping(bytes32 => bool) public disputeResolved;
    mapping(bytes32 => bool) public disputeProverWon;
    mapping(bytes32 => uint256) public disputeExtensions;

    uint256[50] private __gap;  // reserve 50 slots for future upgrades

    function initialize(address _admin, address _remainderVerifier) external initializer {
        _setAdmin(_admin);
        remainderVerifier = _remainderVerifier;
        challengeBondAmount = 0.1 ether;
        proverStake = 0.1 ether;
    }

    function _authorizeUpgrade(address) internal override onlyAdmin {}
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

The `Upgraded(address indexed implementation)` event is emitted on every upgrade. Query the full upgrade history:

```bash
cast logs --from-block 0 --address $PROXY \
  "Upgraded(address)" --rpc-url $RPC_URL
```

### Deployment Registry (Off-Chain)

Maintain a `deployments/` directory with per-chain JSON files (see `deployments/chains.json`):

```json
{
  "chainId": 421614,
  "network": "arb_sepolia",
  "contracts": {
    "UpgradeableExecutionEngine": {
      "proxy": "0x1234...abcd",
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
    "TEEMLVerifier": {
      "address": "0x5678...efgh",
      "deployedAt": "2025-03-01T00:00:00Z",
      "owner": "0x...",
      "remainderVerifier": "0x9abc...ijkl"
    },
    "RemainderVerifier": {
      "address": "0x9abc...ijkl",
      "deployedAt": "2025-03-01T00:00:00Z",
      "owner": "0x...",
      "registeredCircuits": ["0xf85c..."]
    }
  }
}
```

Update this file as part of every deployment or upgrade. The `DeployFullStack.s.sol` script (`contracts/script/DeployFullStack.s.sol`) already logs all addresses; extend it to write JSON output for automated tracking.

### Version Bump Checklist

When releasing a new implementation version:

1. Increment the `VERSION` constant in the new implementation contract.
2. Update `deployments/<chain>.json` with the new implementation address and tx hash.
3. Tag the git commit: `git tag contracts-v2.0.0`.
4. Run the full test suite against the forked upgrade: `forge test --fork-url $RPC -vvv`.
5. Verify source code on Etherscan/Arbiscan: `forge verify-contract $NEW_IMPL UpgradeableExecutionEngineV2 --rpc-url $RPC`.
