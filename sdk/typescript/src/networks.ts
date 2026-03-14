/**
 * Pre-configured network definitions for World ZK Compute.
 *
 * @example
 * ```typescript
 * import { SEPOLIA_NETWORK, TEEVerifier } from '@worldzk/sdk';
 *
 * const verifier = new TEEVerifier({
 *   rpcUrl: 'https://eth-sepolia.g.alchemy.com/v2/YOUR_KEY',
 *   privateKey: '0x...',
 *   contractAddress: SEPOLIA_NETWORK.teeVerifierAddress,
 *   chainId: SEPOLIA_NETWORK.chainId,
 * });
 * ```
 */

type Hex = `0x${string}`;

export interface NetworkConfig {
  /** Human-readable network name */
  name: string;
  /** Chain ID */
  chainId: number;
  /** Pre-deployed verifier router address (risc0 v3.0) */
  verifierRouterAddress: Hex;
  /** Default TEEMLVerifier address (update after deployment) */
  teeVerifierAddress: Hex;
  /** Default ExecutionEngine address (update after deployment) */
  executionEngineAddress: Hex;
  /** Block explorer URL */
  explorerUrl: string;
  /** Whether this network uses TEE-only mode (no ZK verifier) */
  teeOnly: boolean;
}

/** Ethereum Sepolia testnet configuration */
export const SEPOLIA_NETWORK: NetworkConfig = {
  name: 'Ethereum Sepolia',
  chainId: 11155111,
  verifierRouterAddress: '0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187',
  teeVerifierAddress: '0x0000000000000000000000000000000000000000',
  executionEngineAddress: '0x0000000000000000000000000000000000000000',
  explorerUrl: 'https://sepolia.etherscan.io',
  teeOnly: true,
};

/** Local Anvil development network */
export const ANVIL_NETWORK: NetworkConfig = {
  name: 'Anvil (Local)',
  chainId: 31337,
  verifierRouterAddress: '0x0000000000000000000000000000000000000000',
  teeVerifierAddress: '0x0000000000000000000000000000000000000000',
  executionEngineAddress: '0x0000000000000000000000000000000000000000',
  explorerUrl: '',
  teeOnly: false,
};

/** All known networks indexed by chain ID */
export const NETWORKS: Record<number, NetworkConfig> = {
  [SEPOLIA_NETWORK.chainId]: SEPOLIA_NETWORK,
  [ANVIL_NETWORK.chainId]: ANVIL_NETWORK,
};
