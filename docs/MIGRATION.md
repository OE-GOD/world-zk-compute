# Migration Guide

This document covers how to migrate between versions of World ZK Compute, including contract upgrades, SDK changes, and deployment updates.

For contract-specific upgrade procedures (UUPS proxy mechanics, storage layout rules, rollback procedures), see [UPGRADE_GUIDE.md](UPGRADE_GUIDE.md).

---

## v0.1.0 to v0.2.0

### Summary of Changes

- RISC Zero zkVM upgraded from v1.2 to v3.0.
- Guest programs recompiled with new image IDs.
- Verifier router selector changed from `c101b42b` (v1.2) to `73c457ba` (v3.0).
- GKR + Hyrax verification path added via `RemainderVerifier`.
- TEE attestation path added via `TEEMLVerifier`.
- DAG batch verification added for large circuits (multi-tx flow).
- SDK updated with new client methods for TEE and batch verification.

### Step 1: Update Guest Programs

Guest programs must be recompiled with risc0-zkvm v3.0. The resulting image IDs will change.

```bash
# Install matching r0vm version
rzup install r0vm 3.0.5

# Rebuild guest programs
cd examples/rule-engine
cargo build --release

# Note the new image ID from build output
```

Update all references to image IDs in:
- On-chain program registry (`ProgramRegistry.registerProgram`)
- Operator configuration files
- SDK client initialization code
- CI/CD pipeline environment variables

### Step 2: Update Contracts

**If using UUPS proxy (`UpgradeableExecutionEngine`):**

1. Deploy the new implementation contract (V2).
2. Call `upgradeTo(newImplementation)` from the admin account.
3. Verify state preservation (see [UPGRADE_GUIDE.md](UPGRADE_GUIDE.md) Section 2).

**If using immutable contracts:**

1. Deploy new contract instances.
2. Update references in dependent contracts:
   ```bash
   # Update TEEMLVerifier's remainder verifier reference
   cast send $TEE_VERIFIER "setRemainderVerifier(address)" $NEW_REMAINDER \
     --private-key $OWNER_KEY --rpc-url $RPC_URL
   ```
3. Re-register circuits, enclaves, and programs on the new contracts.
4. Update off-chain services to point to new addresses.

### Step 3: Update Deployment Registry

After deploying new contracts, update the off-chain tracking files.

**`deployments/registry.json`:**

```json
{
  "contracts": {
    "TEEMLVerifier": {
      "address": "0xNEW_ADDRESS",
      "version": "2.0.0",
      "deployTxHash": "0x..."
    }
  }
}
```

**`deployments/chains.json`:**

No structural changes needed unless adding a new chain. Verify that `verifierRouter` addresses are correct for the target chain.

### Step 4: Update SDKs

**Rust SDK:**

Update `Cargo.toml`:
```toml
[dependencies]
world-zk-compute-sdk = "0.2.0"
```

Breaking changes:
- `Client::submit_request` now accepts an `ExecutionRequest` struct instead of positional arguments.
- `Client::verify_proof` renamed to `Client::verify_receipt` for clarity.
- New method: `Client::submit_tee_result` for TEE path.

**Python SDK:**

```bash
pip install --upgrade worldzk>=0.2.0
```

Breaking changes:
- `WorldZKClient` constructor now requires `tee_verifier_address` parameter.
- `verify()` method split into `verify_zk()` and `verify_tee()`.

**TypeScript SDK:**

```bash
npm install @worldzk/sdk@^0.2.0
```

Breaking changes:
- `createClient()` options now include `teeVerifierAddress`.
- Event watcher API moved from callback to async iterator pattern.

### Step 5: Update Operator Service

Update the operator configuration:

```toml
# services/operator config
risc0_version = "3.0"
image_ids = ["0xNEW_IMAGE_ID_1", "0xNEW_IMAGE_ID_2"]
tee_verifier_address = "0x..."
remainder_verifier_address = "0x..."
```

Restart the operator service after configuration changes.

### Step 6: Verify

Run the smoke test against your deployment:

```bash
./scripts/smoke-test.sh --network <your-network>
```

Check that:
- [ ] Programs are registered with correct image IDs.
- [ ] Proof submission and verification work end-to-end.
- [ ] TEE attestation path works (if applicable).
- [ ] SDK clients can connect and submit requests.
- [ ] Event watchers receive events from updated contracts.

---

## Breaking Changes Checklist

Before upgrading any component, verify each item:

- [ ] **Image IDs**: Guest programs recompiled? New IDs registered on-chain?
- [ ] **Verifier selector**: Router expects the correct risc0 version selector?
- [ ] **Storage layout**: No reordering, removal, or type changes in proxy contracts? (See [UPGRADE_GUIDE.md](UPGRADE_GUIDE.md) Section 4)
- [ ] **ABI compatibility**: New contract functions do not shadow or remove existing ones?
- [ ] **Event signatures**: No changes to event signatures that indexers depend on?
- [ ] **SDK constructors**: Client code updated for new required parameters?
- [ ] **Configuration**: Operator and enclave configs updated with new addresses/IDs?
- [ ] **Registry files**: `deployments/registry.json` and `deployments/chains.json` updated?

---

## Contract Upgrade Procedure (Quick Reference)

For the full procedure with storage layout rules, rollback steps, and multi-sig process, see [UPGRADE_GUIDE.md](UPGRADE_GUIDE.md).

**UUPS Proxy (quick path):**

```bash
# 1. Deploy new implementation
forge create UpgradeableExecutionEngineV2 --rpc-url $RPC --private-key $KEY

# 2. Upgrade proxy
cast send $PROXY "upgradeTo(address)" $NEW_IMPL --private-key $ADMIN_KEY --rpc-url $RPC

# 3. Verify
cast call $PROXY "implementation()(address)" --rpc-url $RPC
cast call $PROXY "VERSION()(uint256)" --rpc-url $RPC
```

**Immutable contract (quick path):**

```bash
# 1. Deploy new contract
forge create NewTEEMLVerifier --constructor-args $ARGS --rpc-url $RPC --private-key $KEY

# 2. Update references
cast send $DEPENDENT "setRemainderVerifier(address)" $NEW_ADDR --private-key $OWNER --rpc-url $RPC

# 3. Pause old contract
cast send $OLD_CONTRACT "pause()" --private-key $OWNER --rpc-url $RPC
```

---

## Rollback

If an upgrade introduces issues:

1. **Pause** the affected contract immediately.
2. For UUPS proxies: call `upgradeTo(previousImplementation)`.
3. For immutable contracts: redeploy the previous version and update references.
4. Update off-chain services to point back to the working addresses.

See [UPGRADE_GUIDE.md](UPGRADE_GUIDE.md) Section 5 for detailed rollback procedures.
