/**
 * Sepolia E2E Test for TypeScript SDK
 *
 * Exercises the SDK classes against a live Sepolia deployment.
 * Skipped unless SEPOLIA_RPC_URL is set.
 *
 * To run:
 *   SEPOLIA_RPC_URL=https://eth-sepolia.g.alchemy.com/v2/KEY \
 *     npx vitest run src/__tests__/sepolia-e2e.test.ts
 */
import { describe, it, expect, beforeAll } from 'vitest';
import {
  createPublicClient,
  http,
  type Hex,
  type PublicClient,
  type Transport,
  type Chain,
} from 'viem';
import { sepolia } from 'viem/chains';
import * as fs from 'fs';
import * as path from 'path';

import { TEEEventWatcher } from '../tee-event-watcher';
import {
  computeModelHash,
  computeInputHash,
  computeResultHash,
} from '../hash';

// ---------------------------------------------------------------------------
// Environment gate
// ---------------------------------------------------------------------------
const SEPOLIA_RPC_URL = process.env.SEPOLIA_RPC_URL;

// Try to load deployment addresses
let DEPLOYMENT: Record<string, any> = {};
const DEPLOY_FILE = path.resolve(
  __dirname,
  '../../../../deployments/11155111.json'
);
try {
  DEPLOYMENT = JSON.parse(fs.readFileSync(DEPLOY_FILE, 'utf-8'));
} catch {
  // Will be handled by the skip condition
}

const CONTRACT_ADDRESS =
  (process.env.TEE_VERIFIER_ADDRESS as Hex) ||
  (DEPLOYMENT?.contracts?.TEEMLVerifier?.address as Hex);

const shouldRun = !!(SEPOLIA_RPC_URL && CONTRACT_ADDRESS);

describe.skipIf(!shouldRun)('Sepolia E2E', () => {
  let client: PublicClient<Transport, Chain>;

  beforeAll(() => {
    client = createPublicClient({
      chain: sepolia,
      transport: http(SEPOLIA_RPC_URL),
    });
  });

  it('should connect to Sepolia and get chain ID', async () => {
    const chainId = await client.getChainId();
    expect(chainId).toBe(11155111);
  });

  it('should verify contract code exists at address', async () => {
    const code = await client.getCode({ address: CONTRACT_ADDRESS! });
    expect(code).toBeDefined();
    expect(code!.length).toBeGreaterThan(2); // More than just "0x"
  });

  it('should construct TEEEventWatcher', () => {
    const watcher = new TEEEventWatcher({
      contractAddress: CONTRACT_ADDRESS!,
      rpcUrl: SEPOLIA_RPC_URL!,
      chainId: 11155111,
    });
    expect(watcher).toBeDefined();
  });

  it('should compute hashes consistently', () => {
    const modelHash = computeModelHash(new Uint8Array([1, 2, 3]));
    expect(modelHash).toMatch(/^0x[0-9a-f]{64}$/);

    const inputHash = computeInputHash([4.0, 5.0, 6.0]);
    expect(inputHash).toMatch(/^0x[0-9a-f]{64}$/);

    const resultHash = computeResultHash([0.85]);
    expect(resultHash).toMatch(/^0x[0-9a-f]{64}$/);
  });

  it('should get current block number on Sepolia', async () => {
    const blockNumber = await client.getBlockNumber();
    expect(blockNumber).toBeGreaterThan(0n);
  });
});
