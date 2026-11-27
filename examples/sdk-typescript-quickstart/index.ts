/**
 * World ZK Compute — TypeScript SDK Quickstart
 *
 * Prerequisites:
 *   1. Build the SDK (../../sdk/typescript) and run npm install here
 *   2. Start Anvil: anvil --block-time 1
 *   3. Deploy TEEMLVerifier behind its proxy and set CONTRACT_ADDRESS below
 *
 * Usage:
 *   npx tsx index.ts
 */

import {
  TEEVerifier,
  checkHealth,
  computeModelHash,
  computeInputHash,
  generatedTeeMLVerifierAbi,
} from "@worldzk/sdk";
import {
  createPublicClient,
  encodePacked,
  formatEther,
  http,
  keccak256,
} from "viem";
import { privateKeyToAccount } from "viem/accounts";

type Hex = `0x${string}`;

// ---------------------------------------------------------------------------
// Configuration (override via env vars)
// ---------------------------------------------------------------------------
const RPC_URL = process.env.RPC_URL ?? "http://127.0.0.1:8545";
// Proxy address when DeployTEEMLVerifier.s.sol is the first deployment of Anvil account #0
const CONTRACT_ADDRESS =
  (process.env.CONTRACT_ADDRESS as Hex) ??
  "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512";
// Contract admin and result submitter (Anvil account #0)
const PRIVATE_KEY =
  (process.env.PRIVATE_KEY as Hex) ??
  "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
// Key that stands in for the TEE enclave and signs the attestation (Anvil account #1)
const ENCLAVE_KEY =
  (process.env.ENCLAVE_KEY as Hex) ??
  "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";
const INDEXER_URL = process.env.INDEXER_URL ?? "http://127.0.0.1:8081";

async function main() {
  // --- Create SDK clients ---
  const publicClient = createPublicClient({ transport: http(RPC_URL) });
  const chainId = await publicClient.getChainId();
  const tee = new TEEVerifier({
    rpcUrl: RPC_URL,
    privateKey: PRIVATE_KEY,
    contractAddress: CONTRACT_ADDRESS,
    chainId,
  });
  console.log(`TEEVerifier connected (admin: ${await tee.admin()})`);

  // --- Compute hashes using SDK utilities ---
  const modelBytes = new TextEncoder().encode("my-model-v1");
  const features = [5.1, 3.5, 1.4, 0.2];
  const modelHash = computeModelHash(modelBytes);
  const inputHash = computeInputHash(features);
  console.log(`Model hash: ${modelHash}`);
  console.log(`Input hash: ${inputHash}`);

  // --- Read the stake a submitter has to attach ---
  const stake = await publicClient.readContract({
    address: CONTRACT_ADDRESS,
    abi: generatedTeeMLVerifierAbi,
    functionName: "proverStake",
  });
  console.log(`Required prover stake: ${formatEther(stake)} ETH`);

  // --- Register the enclave signing key (admin only, once) ---
  const enclave = privateKeyToAccount(ENCLAVE_KEY);
  const enclaveInfo = await publicClient.readContract({
    address: CONTRACT_ADDRESS,
    abi: generatedTeeMLVerifierAbi,
    functionName: "enclaves",
    args: [enclave.address],
  });
  if (!enclaveInfo.registered) {
    const imageHash = keccak256(new TextEncoder().encode("quickstart-enclave-image"));
    await tee.registerEnclave(enclave.address, imageHash);
    console.log(`Enclave registered: ${enclave.address}`);
  }

  // --- Sign the attestation the way the enclave does (EIP-712) ---
  const resultBytes = ("0x" + "00".repeat(32)) as Hex;
  const attestation = await enclave.signTypedData({
    domain: {
      name: "TEEMLVerifier",
      version: "1",
      chainId,
      verifyingContract: CONTRACT_ADDRESS,
    },
    types: {
      TEEMLResult: [
        { name: "modelHash", type: "bytes32" },
        { name: "inputHash", type: "bytes32" },
        { name: "resultHash", type: "bytes32" },
      ],
    },
    primaryType: "TEEMLResult",
    message: { modelHash, inputHash, resultHash: keccak256(resultBytes) },
  });

  // --- Submit a TEE result ---
  const txHash = await tee.submitResult(
    modelHash,
    inputHash,
    resultBytes,
    attestation,
    formatEther(stake), // stake in ETH
  );
  console.log(`submitResult tx: ${txHash}`);

  // --- Derive the result ID: keccak256(submitter, modelHash, inputHash, block number) ---
  const receipt = await publicClient.getTransactionReceipt({ hash: txHash });
  const submitter = privateKeyToAccount(PRIVATE_KEY).address;
  const resultId = keccak256(
    encodePacked(
      ["address", "bytes32", "bytes32", "uint256"],
      [submitter, modelHash, inputHash, receipt.blockNumber],
    ),
  );
  console.log(`Result ID: ${resultId}`);

  // --- Query the stored result and its validity ---
  const result = await tee.getResult(resultId);
  console.log(
    `Challenge window closes at: ${new Date(Number(result.challengeDeadline) * 1000).toISOString()}`,
  );
  const isValid = await tee.isResultValid(resultId);
  console.log(`Is valid (before finalize): ${isValid}`);

  // --- Check indexer health ---
  try {
    const health = await checkHealth(INDEXER_URL);
    console.log(`Indexer status: ${health.status}`);
  } catch {
    console.log("Indexer not available (expected if not running)");
  }

  console.log("\nQuickstart complete!");
  console.log(
    "To finalize, wait for the challenge window to pass, then call tee.finalize(resultId).",
  );
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
