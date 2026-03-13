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
  createPublicClient,
  createWalletClient,
  http,
  parseAbi,
  keccak256,
  encodePacked,
  type Hex,
} from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { anvil } from "viem/chains";

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

// Minimal ABI
const abi = parseAbi([
  "function submitResult(bytes32 modelHash, bytes32 inputHash, bytes32 result, bytes32 imageHash, bytes attestation) payable",
  "function finalizeResult(bytes32 resultId)",
  "function isResultValid(bytes32 resultId) view returns (bool)",
  "function proverStake() view returns (uint256)",
  "event ResultSubmitted(bytes32 indexed resultId, bytes32 modelHash, bytes32 inputHash)",
]);

async function main() {
  const publicClient = createPublicClient({
    chain: anvil,
    transport: http(RPC_URL),
  });

  const account = privateKeyToAccount(PRIVATE_KEY);
  const walletClient = createWalletClient({
    account,
    chain: anvil,
    transport: http(RPC_URL),
  });

  const chainId = await publicClient.getChainId();
  console.log(`Connected to ${RPC_URL} (chain ${chainId})`);

  // --- Submit a TEE result ---
  const modelHash =
    "0x0000000000000000000000000000000000000000000000000000000000000001" as Hex;
  const inputHash =
    "0x0000000000000000000000000000000000000000000000000000000000000002" as Hex;
  const result =
    "0x0000000000000000000000000000000000000000000000000000000000000003" as Hex;
  const imageHash =
    "0x0000000000000000000000000000000000000000000000000000000000000004" as Hex;
  const attestation = ("0x" + "00".repeat(65)) as Hex;

  const stake = await publicClient.readContract({
    address: CONTRACT_ADDRESS,
    abi,
    functionName: "proverStake",
  });
  console.log(`Required stake: ${stake} wei`);

  const txHash = await walletClient.writeContract({
    address: CONTRACT_ADDRESS,
    abi,
    functionName: "submitResult",
    args: [modelHash, inputHash, result, imageHash, attestation],
    value: stake,
  });

  const receipt = await publicClient.waitForTransactionReceipt({ hash: txHash });
  console.log(`submitResult tx: ${receipt.transactionHash} (status=${receipt.status})`);

  // Compute result ID
  const resultId = keccak256(
    encodePacked(["bytes32", "bytes32"], [modelHash, inputHash])
  );
  console.log(`Result ID: ${resultId}`);

  // --- Query result ---
  const isValid = await publicClient.readContract({
    address: CONTRACT_ADDRESS,
    abi,
    functionName: "isResultValid",
    args: [resultId],
  });
  console.log(`Is valid (before finalize): ${isValid}`);

  console.log("\nResult submitted successfully!");
  console.log(
    "To finalize, wait for the challenge window to pass, then call finalizeResult()."
  );
}

main().catch(console.error);
