/**
 * World ZK Compute — TypeScript SDK Quickstart
 *
 * Prerequisites:
 *   1. npm install
 *   2. Start Anvil: anvil --block-time 1
 *   3. Deploy TEEMLVerifier contract and set CONTRACT_ADDRESS below
 *
 * Usage:
 *   npx tsx index.ts
 */

import {
  TEEVerifier,
  Client,
  computeModelHash,
  computeInputHash,
} from "@worldzk/sdk";

type Hex = `0x${string}`;

// ---------------------------------------------------------------------------
// Configuration (override via env vars)
// ---------------------------------------------------------------------------
const RPC_URL = process.env.RPC_URL ?? "http://127.0.0.1:8545";
const CONTRACT_ADDRESS =
  (process.env.CONTRACT_ADDRESS as Hex) ??
  "0x5FbDB2315678afecb367f032d93F642f64180aa3";
const PRIVATE_KEY =
  (process.env.PRIVATE_KEY as Hex) ??
  "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const INDEXER_URL = process.env.INDEXER_URL ?? "http://127.0.0.1:8081";

async function main() {
  // --- Create SDK clients ---
  const tee = new TEEVerifier({
    rpcUrl: RPC_URL,
    privateKey: PRIVATE_KEY,
    contractAddress: CONTRACT_ADDRESS,
  });
  console.log("TEEVerifier connected");

  const client = new Client({
    baseUrl: INDEXER_URL,
  });

  // --- Compute hashes using SDK utilities ---
  const modelBytes = new TextEncoder().encode("my-model-v1");
  const inputBytes = new TextEncoder().encode("sample-input-data");
  const modelHash = await computeModelHash(modelBytes);
  const inputHash = await computeInputHash(inputBytes);
  console.log(`Model hash: ${modelHash}`);
  console.log(`Input hash: ${inputHash}`);

  // --- Submit a TEE result ---
  const resultBytes = ("0x" + "00".repeat(32)) as Hex;
  const attestation = ("0x" + "00".repeat(65)) as Hex;

  const txHash = await tee.submitResult(
    modelHash as Hex,
    inputHash as Hex,
    resultBytes,
    attestation,
    "0.1", // stake in ETH
  );
  console.log(`submitResult tx: ${txHash}`);

  // --- Query result validity ---
  const resultId = await tee.isResultValid(
    modelHash as Hex, // result ID derivation depends on contract
  );
  console.log(`Is valid (before finalize): ${resultId}`);

  // --- Check indexer health ---
  try {
    const health = await client.health();
    console.log(`Indexer status: ${health.status}`);
  } catch {
    console.log("Indexer not available (expected if not running)");
  }

  // --- List execution requests via API client ---
  try {
    const requests = await client.requests.list({ limit: 5 });
    console.log(`Found ${requests.pagination.total} execution requests`);
    for (const req of requests.items) {
      console.log(`  Request #${req.id}: ${req.status}`);
    }
  } catch {
    console.log("API not available (expected if not running)");
  }

  console.log("\nQuickstart complete!");
  console.log(
    "To finalize, wait for the challenge window to pass, then call tee.finalize().",
  );
}

main().catch(console.error);
