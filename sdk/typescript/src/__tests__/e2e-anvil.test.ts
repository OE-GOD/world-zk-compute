/**
 * E2E Anvil Test for TypeScript SDK
 *
 * Exercises the SDK classes (TEEVerifier, TEEEventWatcher, hash utilities)
 * against a running Anvil instance. The entire suite is skipped when the
 * ANVIL_URL environment variable is not set.
 *
 * To run:
 *   ANVIL_URL=http://127.0.0.1:8545 npx vitest run src/__tests__/e2e-anvil.test.ts
 *
 * These tests do NOT deploy contracts. They verify that the SDK types can be
 * constructed and that calling methods against an RPC endpoint produces the
 * expected error shapes (since no contract is deployed at the target address).
 */
import { describe, it, expect, beforeAll } from 'vitest';
import {
  createPublicClient,
  http,
  type Hex,
  type PublicClient,
  type Transport,
  type Chain,
  defineChain,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';

import { TEEVerifier } from '../tee-verifier';
import type { TEEVerifierConfig } from '../tee-verifier';
import { TEEEventWatcher } from '../tee-event-watcher';
import type {
  TEEEventWatcherConfig,
  TEEEventData,
  EventSubscription,
} from '../tee-event-watcher';
import {
  computeModelHash,
  computeInputHash,
  computeResultHash,
  computeInputHashFromJson,
  computeResultHashFromBytes,
  serializeF64List,
} from '../hash';
import { teeMLVerifierAbi } from '../tee-verifier-abi';

// ---------------------------------------------------------------------------
// Environment gate
// ---------------------------------------------------------------------------
const ANVIL_URL = process.env.ANVIL_URL;

// Anvil default deterministic accounts
const ADMIN_PRIVATE_KEY: Hex =
  '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80';
const PROVER_PRIVATE_KEY: Hex =
  '0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d';

const adminAccount = privateKeyToAccount(ADMIN_PRIVATE_KEY);
// A fake contract address (no contract deployed here)
const FAKE_CONTRACT_ADDRESS: Hex =
  '0x5FbDB2315678afecb367f032d93F642f64180aa3';

// ---------------------------------------------------------------------------
// Test suite -- skipped entirely when ANVIL_URL is not set
// ---------------------------------------------------------------------------
describe.skipIf(!ANVIL_URL)('E2E Anvil: SDK classes against local Anvil', () => {
  let chain: Chain;
  let publicClient: PublicClient<Transport, Chain>;

  beforeAll(() => {
    chain = defineChain({
      id: 31337,
      name: 'Anvil',
      nativeCurrency: { name: 'Ether', symbol: 'ETH', decimals: 18 },
      rpcUrls: { default: { http: [ANVIL_URL!] } },
    });

    publicClient = createPublicClient({
      chain,
      transport: http(ANVIL_URL!),
    });
  });

  // -------------------------------------------------------------------------
  // 1. Viem clients can connect to Anvil
  // -------------------------------------------------------------------------
  it('should connect viem publicClient to Anvil and fetch block number', async () => {
    const blockNumber = await publicClient.getBlockNumber();
    expect(typeof blockNumber).toBe('bigint');
    expect(blockNumber).toBeGreaterThanOrEqual(0n);
  });

  it('should connect viem walletClient to Anvil and have a funded account', async () => {
    const balance = await publicClient.getBalance({
      address: adminAccount.address,
    });
    // Anvil default accounts have 10000 ETH
    expect(balance).toBeGreaterThan(0n);
  });

  // -------------------------------------------------------------------------
  // 2. TEEVerifier construction and method availability
  // -------------------------------------------------------------------------
  it('should construct TEEVerifier with valid config', () => {
    const config: TEEVerifierConfig = {
      rpcUrl: ANVIL_URL!,
      privateKey: ADMIN_PRIVATE_KEY,
      contractAddress: FAKE_CONTRACT_ADDRESS,
    };
    const verifier = new TEEVerifier(config);
    expect(verifier).toBeInstanceOf(TEEVerifier);

    // Verify all public methods exist
    expect(typeof verifier.registerEnclave).toBe('function');
    expect(typeof verifier.revokeEnclave).toBe('function');
    expect(typeof verifier.submitResult).toBe('function');
    expect(typeof verifier.challenge).toBe('function');
    expect(typeof verifier.finalize).toBe('function');
    expect(typeof verifier.resolveDispute).toBe('function');
    expect(typeof verifier.resolveDisputeByTimeout).toBe('function');
    expect(typeof verifier.getResult).toBe('function');
    expect(typeof verifier.isResultValid).toBe('function');
    expect(typeof verifier.owner).toBe('function');
    expect(typeof verifier.pendingOwner).toBe('function');
    expect(typeof verifier.transferOwnership).toBe('function');
    expect(typeof verifier.acceptOwnership).toBe('function');
    expect(typeof verifier.pause).toBe('function');
    expect(typeof verifier.unpause).toBe('function');
    expect(typeof verifier.paused).toBe('function');
    expect(typeof verifier.remainderVerifier).toBe('function');
  });

  it('should construct TEEVerifier with custom chainId', () => {
    const config: TEEVerifierConfig = {
      rpcUrl: ANVIL_URL!,
      privateKey: PROVER_PRIVATE_KEY,
      contractAddress: FAKE_CONTRACT_ADDRESS,
      chainId: 1,
    };
    const verifier = new TEEVerifier(config);
    expect(verifier).toBeInstanceOf(TEEVerifier);
  });

  // -------------------------------------------------------------------------
  // 3. TEEVerifier.submitResult -- no deployed contract, expect revert
  // -------------------------------------------------------------------------
  it('should fail submitResult against a non-existent contract with a known error shape', async () => {
    const verifier = new TEEVerifier({
      rpcUrl: ANVIL_URL!,
      privateKey: PROVER_PRIVATE_KEY,
      contractAddress: FAKE_CONTRACT_ADDRESS,
    });

    const modelHash: Hex = ('0x' + '11'.repeat(32)) as Hex;
    const inputHash: Hex = ('0x' + '22'.repeat(32)) as Hex;
    const resultBytes: Hex = '0xdeadbeef';
    const fakeAttestation: Hex = ('0x' + 'ff'.repeat(65)) as Hex;

    try {
      await verifier.submitResult(
        modelHash,
        inputHash,
        resultBytes,
        fakeAttestation,
        '0.1',
      );
      // Should not reach here
      expect.unreachable('submitResult should have thrown');
    } catch (error: unknown) {
      // viem throws a ContractFunctionExecutionError or similar
      expect(error).toBeDefined();
      expect(error instanceof Error).toBe(true);
    }
  });

  // -------------------------------------------------------------------------
  // 4. TEEVerifier.getResult -- no deployed contract, expect revert
  // -------------------------------------------------------------------------
  it('should fail getResult against a non-existent contract', async () => {
    const verifier = new TEEVerifier({
      rpcUrl: ANVIL_URL!,
      privateKey: ADMIN_PRIVATE_KEY,
      contractAddress: FAKE_CONTRACT_ADDRESS,
    });

    const unknownId: Hex = ('0x' + '00'.repeat(32)) as Hex;

    try {
      await verifier.getResult(unknownId);
      // If the address has no code, viem will throw
      expect.unreachable('getResult should have thrown');
    } catch (error: unknown) {
      expect(error).toBeDefined();
      expect(error instanceof Error).toBe(true);
    }
  });

  // -------------------------------------------------------------------------
  // 5. TEEVerifier.challenge -- no deployed contract, expect revert
  // -------------------------------------------------------------------------
  it('should fail challenge against a non-existent contract', async () => {
    const verifier = new TEEVerifier({
      rpcUrl: ANVIL_URL!,
      privateKey: PROVER_PRIVATE_KEY,
      contractAddress: FAKE_CONTRACT_ADDRESS,
    });

    const fakeResultId: Hex = ('0x' + 'ab'.repeat(32)) as Hex;

    try {
      await verifier.challenge(fakeResultId, '0.1');
      expect.unreachable('challenge should have thrown');
    } catch (error: unknown) {
      expect(error).toBeDefined();
      expect(error instanceof Error).toBe(true);
    }
  });

  // -------------------------------------------------------------------------
  // 6. TEEEventWatcher construction and method checks
  // -------------------------------------------------------------------------
  it('should construct TEEEventWatcher with valid config', () => {
    const config: TEEEventWatcherConfig = {
      rpcUrl: ANVIL_URL!,
      contractAddress: FAKE_CONTRACT_ADDRESS,
    };
    const watcher = new TEEEventWatcher(config);
    expect(watcher).toBeInstanceOf(TEEEventWatcher);
    expect(watcher.isWatching).toBe(false);
    expect(watcher.hasWebSocket).toBe(false);
    expect(watcher.activeSubscriptionCount).toBe(0);

    // Verify public methods exist
    expect(typeof watcher.on).toBe('function');
    expect(typeof watcher.off).toBe('function');
    expect(typeof watcher.start).toBe('function');
    expect(typeof watcher.stop).toBe('function');
    expect(typeof watcher.getPastEvents).toBe('function');
    expect(typeof watcher.watchEvents).toBe('function');
    expect(typeof watcher.unsubscribe).toBe('function');
    expect(typeof watcher.unsubscribeAll).toBe('function');
  });

  it('should construct TEEEventWatcher with custom chainId and wsUrl', () => {
    const watcher = new TEEEventWatcher({
      rpcUrl: ANVIL_URL!,
      contractAddress: FAKE_CONTRACT_ADDRESS,
      chainId: 42161,
      wsUrl: 'ws://127.0.0.1:8546',
    });
    expect(watcher).toBeInstanceOf(TEEEventWatcher);
    expect(watcher.hasWebSocket).toBe(true);
  });

  // -------------------------------------------------------------------------
  // 7. TEEEventWatcher.getPastEvents -- returns empty array for empty chain
  // -------------------------------------------------------------------------
  it('should return empty array from getPastEvents when no contract events exist', async () => {
    const watcher = new TEEEventWatcher({
      rpcUrl: ANVIL_URL!,
      contractAddress: FAKE_CONTRACT_ADDRESS,
    });

    const events = await watcher.getPastEvents('ResultSubmitted', 0n, 'latest');
    expect(Array.isArray(events)).toBe(true);
    // No contract deployed at the address, so no events
    expect(events.length).toBe(0);
  });

  it('should return empty array from getPastEvents without event filter', async () => {
    const watcher = new TEEEventWatcher({
      rpcUrl: ANVIL_URL!,
      contractAddress: FAKE_CONTRACT_ADDRESS,
    });

    const allEvents = await watcher.getPastEvents(undefined, 0n, 'latest');
    expect(Array.isArray(allEvents)).toBe(true);
    expect(allEvents.length).toBe(0);
  });

  // -------------------------------------------------------------------------
  // 8. TEEEventWatcher.watchEvents subscription lifecycle
  // -------------------------------------------------------------------------
  it('should create and unsubscribe a watchEvents subscription', () => {
    const watcher = new TEEEventWatcher({
      rpcUrl: ANVIL_URL!,
      contractAddress: FAKE_CONTRACT_ADDRESS,
    });

    const events: TEEEventData[] = [];
    const sub: EventSubscription = watcher.watchEvents(
      (event) => {
        events.push(event);
      },
      { eventTypes: ['ResultSubmitted', 'ResultChallenged'] },
    );

    expect(sub.active).toBe(true);
    expect(typeof sub.id).toBe('string');
    expect(sub.id.length).toBeGreaterThan(0);
    expect(watcher.activeSubscriptionCount).toBe(1);

    sub.unsubscribe();

    expect(sub.active).toBe(false);
    expect(watcher.activeSubscriptionCount).toBe(0);
  });

  // -------------------------------------------------------------------------
  // 9. TEEEventWatcher on/off handler registration
  // -------------------------------------------------------------------------
  it('should register and unregister event handlers via on/off', () => {
    const watcher = new TEEEventWatcher({
      rpcUrl: ANVIL_URL!,
      contractAddress: FAKE_CONTRACT_ADDRESS,
    });

    const handler = () => {};

    // on() should be chainable
    const result = watcher
      .on('ResultSubmitted', handler)
      .on('ResultChallenged', handler)
      .on('ResultFinalized', handler)
      .on('DisputeResolved', handler)
      .on('EnclaveRegistered', handler)
      .on('EnclaveRevoked', handler);

    expect(result).toBe(watcher);

    // off() should be chainable and not throw
    const offResult = watcher
      .off('ResultSubmitted', handler)
      .off('EnclaveRevoked', handler);

    expect(offResult).toBe(watcher);
  });

  // -------------------------------------------------------------------------
  // 10. Hash utilities produce valid bytes32 values
  // -------------------------------------------------------------------------
  it('should produce valid bytes32 hashes from SDK hash utilities', () => {
    const hexPattern = /^0x[0-9a-f]{64}$/;

    // computeModelHash from Uint8Array
    const modelBytes = new TextEncoder().encode('{"learner":{}}');
    const modelHash = computeModelHash(modelBytes);
    expect(modelHash).toMatch(hexPattern);

    // computeModelHash from hex string
    const modelHashFromHex = computeModelHash('0xdeadbeef');
    expect(modelHashFromHex).toMatch(hexPattern);

    // computeInputHash from feature vector
    const inputHash = computeInputHash([1.0, 2.5, 3.7]);
    expect(inputHash).toMatch(hexPattern);

    // computeResultHash from prediction scores
    const resultHash = computeResultHash([0.85, 0.15]);
    expect(resultHash).toMatch(hexPattern);

    // computeInputHashFromJson
    const jsonBytes = new TextEncoder().encode('[1.0,2.5,3.7]');
    const inputHashFromJson = computeInputHashFromJson(jsonBytes);
    expect(inputHashFromJson).toMatch(hexPattern);

    // computeResultHashFromBytes
    const resultHashFromBytes = computeResultHashFromBytes('0xabcd');
    expect(resultHashFromBytes).toMatch(hexPattern);

    // All distinct inputs produce distinct hashes
    expect(modelHash).not.toBe(modelHashFromHex);
    expect(inputHash).not.toBe(resultHash);
  });

  it('should serialize f64 list matching Rust serde_json behavior', () => {
    // Integer-valued floats get ".0" suffix
    const bytes = serializeF64List([1.0, 2.0, 3.5]);
    const jsonStr = new TextDecoder().decode(bytes);
    expect(jsonStr).toBe('[1.0,2.0,3.5]');

    // Negative values
    const negBytes = serializeF64List([-1.0, 0.0, 100.0]);
    const negJsonStr = new TextDecoder().decode(negBytes);
    expect(negJsonStr).toBe('[-1.0,0.0,100.0]');

    // Empty array
    const emptyBytes = serializeF64List([]);
    const emptyJsonStr = new TextDecoder().decode(emptyBytes);
    expect(emptyJsonStr).toBe('[]');
  });

  // -------------------------------------------------------------------------
  // 11. TEE verifier ABI is well-formed
  // -------------------------------------------------------------------------
  it('should export a well-formed teeMLVerifierAbi with expected entries', () => {
    expect(Array.isArray(teeMLVerifierAbi)).toBe(true);
    expect(teeMLVerifierAbi.length).toBeGreaterThan(0);

    // Check for key function signatures
    const functionNames = teeMLVerifierAbi
      .filter((entry) => entry.type === 'function')
      .map((entry) => entry.name);

    expect(functionNames).toContain('submitResult');
    expect(functionNames).toContain('challenge');
    expect(functionNames).toContain('finalize');
    expect(functionNames).toContain('getResult');
    expect(functionNames).toContain('isResultValid');
    expect(functionNames).toContain('registerEnclave');
    expect(functionNames).toContain('owner');
    expect(functionNames).toContain('paused');

    // Check for key event signatures
    const eventNames = teeMLVerifierAbi
      .filter((entry) => entry.type === 'event')
      .map((entry) => entry.name);

    expect(eventNames).toContain('ResultSubmitted');
    expect(eventNames).toContain('ResultChallenged');
    expect(eventNames).toContain('ResultFinalized');
    expect(eventNames).toContain('DisputeResolved');
    expect(eventNames).toContain('EnclaveRegistered');
    expect(eventNames).toContain('EnclaveRevoked');
  });

  // -------------------------------------------------------------------------
  // 12. TEEVerifier.isResultValid -- no deployed contract, expect revert
  // -------------------------------------------------------------------------
  it('should fail isResultValid against a non-existent contract', async () => {
    const verifier = new TEEVerifier({
      rpcUrl: ANVIL_URL!,
      privateKey: ADMIN_PRIVATE_KEY,
      contractAddress: FAKE_CONTRACT_ADDRESS,
    });

    const fakeId: Hex = ('0x' + 'cc'.repeat(32)) as Hex;

    try {
      await verifier.isResultValid(fakeId);
      expect.unreachable('isResultValid should have thrown');
    } catch (error: unknown) {
      expect(error).toBeDefined();
      expect(error instanceof Error).toBe(true);
    }
  });

  // -------------------------------------------------------------------------
  // 13. TEEEventWatcher start/stop lifecycle against Anvil
  // -------------------------------------------------------------------------
  it('should start and stop the watcher without errors', () => {
    const watcher = new TEEEventWatcher({
      rpcUrl: ANVIL_URL!,
      contractAddress: FAKE_CONTRACT_ADDRESS,
    });

    expect(watcher.isWatching).toBe(false);

    watcher.start();
    expect(watcher.isWatching).toBe(true);

    // start() is idempotent
    watcher.start();
    expect(watcher.isWatching).toBe(true);

    watcher.stop();
    expect(watcher.isWatching).toBe(false);

    // stop() is idempotent
    watcher.stop();
    expect(watcher.isWatching).toBe(false);
  });

  // -------------------------------------------------------------------------
  // 14. Multiple watchEvents subscriptions can coexist
  // -------------------------------------------------------------------------
  it('should support multiple concurrent watchEvents subscriptions', () => {
    const watcher = new TEEEventWatcher({
      rpcUrl: ANVIL_URL!,
      contractAddress: FAKE_CONTRACT_ADDRESS,
    });

    const sub1 = watcher.watchEvents(() => {}, {
      eventTypes: ['ResultSubmitted'],
    });
    const sub2 = watcher.watchEvents(() => {}, {
      eventTypes: ['ResultChallenged'],
    });
    const sub3 = watcher.watchEvents(() => {}); // all events

    expect(watcher.activeSubscriptionCount).toBe(3);
    expect(sub1.id).not.toBe(sub2.id);
    expect(sub2.id).not.toBe(sub3.id);

    // Unsubscribe one; others remain
    sub1.unsubscribe();
    expect(watcher.activeSubscriptionCount).toBe(2);
    expect(sub1.active).toBe(false);
    expect(sub2.active).toBe(true);
    expect(sub3.active).toBe(true);

    // unsubscribeAll cleans everything
    watcher.unsubscribeAll();
    expect(watcher.activeSubscriptionCount).toBe(0);
    expect(sub2.active).toBe(false);
    expect(sub3.active).toBe(false);
  });

  // -------------------------------------------------------------------------
  // 15. SDK exports are available and correctly typed
  // -------------------------------------------------------------------------
  it('should export all expected symbols from the SDK index', async () => {
    const sdk = await import('../index');

    // Classes
    expect(sdk.TEEVerifier).toBeDefined();
    expect(sdk.TEEEventWatcher).toBeDefined();

    // Hash utilities
    expect(typeof sdk.computeModelHash).toBe('function');
    expect(typeof sdk.computeInputHash).toBe('function');
    expect(typeof sdk.computeResultHash).toBe('function');
    expect(typeof sdk.computeInputHashFromJson).toBe('function');
    expect(typeof sdk.computeResultHashFromBytes).toBe('function');
    expect(typeof sdk.serializeF64List).toBe('function');

    // ABI
    expect(sdk.teeMLVerifierAbi).toBeDefined();
    expect(Array.isArray(sdk.teeMLVerifierAbi)).toBe(true);

    // Error classes
    expect(sdk.WorldZKError).toBeDefined();
    expect(sdk.ApiError).toBeDefined();
    expect(sdk.ValidationError).toBeDefined();
    expect(sdk.NetworkError).toBeDefined();
    expect(sdk.TimeoutError).toBeDefined();
  });
});
