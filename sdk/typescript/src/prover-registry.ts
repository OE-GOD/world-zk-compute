/**
 * ProverRegistry SDK client for on-chain prover management.
 *
 * Wraps the ProverRegistry.sol contract using viem, providing typed
 * methods for registration, staking, reputation queries, and admin operations.
 */
import type {
  PublicClient,
  WalletClient,
} from 'viem';
import { proverRegistryAbi } from './abi';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type Hex = `0x${string}`;

/** On-chain Prover struct */
export interface OnChainProverInfo {
  owner: Hex;
  stake: bigint;
  reputation: bigint;
  proofsSubmitted: bigint;
  proofsFailed: bigint;
  totalEarnings: bigint;
  registeredAt: bigint;
  lastActiveAt: bigint;
  active: boolean;
  endpoint: string;
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/**
 * Typed client for the ProverRegistry smart contract.
 *
 * Read-only operations only require a `PublicClient`. Write operations
 * additionally require a `WalletClient`.
 *
 * @example
 * ```ts
 * import { createPublicClient, createWalletClient, http } from 'viem';
 * import { privateKeyToAccount } from 'viem/accounts';
 * import { sepolia } from 'viem/chains';
 * import { ProverRegistryClient } from '@worldzk/sdk';
 *
 * const publicClient = createPublicClient({ chain: sepolia, transport: http() });
 * const walletClient = createWalletClient({
 *   chain: sepolia,
 *   transport: http(),
 *   account: privateKeyToAccount('0x...'),
 * });
 *
 * const registry = new ProverRegistryClient(publicClient, walletClient, '0x...');
 *
 * // Query prover info
 * const info = await registry.getProverInfo('0x...');
 * console.log(`Stake: ${info.stake}, Active: ${info.active}`);
 *
 * // Get all active provers
 * const provers = await registry.getActiveProvers();
 * console.log(`Active provers: ${provers.length}`);
 * ```
 */
export class ProverRegistryClient {
  private readonly publicClient: PublicClient;
  private readonly walletClient: WalletClient | undefined;
  private readonly contractAddress: Hex;

  constructor(
    publicClient: PublicClient,
    walletClient: WalletClient | undefined,
    contractAddress: Hex,
  ) {
    this.publicClient = publicClient;
    this.walletClient = walletClient;
    this.contractAddress = contractAddress;
  }

  // -----------------------------------------------------------------------
  // Registration
  // -----------------------------------------------------------------------

  /**
   * Register as a prover with initial stake and optional endpoint.
   *
   * The caller must have approved the staking token for transfer beforehand.
   * Initial reputation is set to 5000 (50%).
   *
   * @param stake - Amount of staking tokens to deposit (must be >= minStake)
   * @param endpoint - Optional P2P endpoint for coordination
   * @returns The transaction hash
   */
  async registerProver(stake: bigint, endpoint: string = ''): Promise<Hex> {
    return this.send('register', [stake, endpoint]);
  }

  /**
   * Deactivate the caller's prover registration.
   *
   * Stops receiving new jobs. The prover can later withdraw stake below
   * the minimum while deactivated.
   *
   * @returns The transaction hash
   */
  async deregisterProver(): Promise<Hex> {
    return this.send('deactivate', []);
  }

  /**
   * Reactivate the caller's prover registration.
   *
   * The prover must have at least minStake to reactivate.
   *
   * @returns The transaction hash
   */
  async reactivateProver(): Promise<Hex> {
    return this.send('reactivate', []);
  }

  // -----------------------------------------------------------------------
  // Staking
  // -----------------------------------------------------------------------

  /**
   * Add additional stake to the caller's prover registration.
   *
   * If the prover was deactivated due to low stake and the new total
   * exceeds minStake, the prover is automatically reactivated.
   *
   * @param amount - Amount of staking tokens to add
   * @returns The transaction hash
   */
  async addStake(amount: bigint): Promise<Hex> {
    return this.send('addStake', [amount]);
  }

  /**
   * Withdraw stake from the caller's prover registration.
   *
   * If the prover is active, the remaining stake must be >= minStake.
   *
   * @param amount - Amount of staking tokens to withdraw
   * @returns The transaction hash
   */
  async withdrawStake(amount: bigint): Promise<Hex> {
    return this.send('withdrawStake', [amount]);
  }

  // -----------------------------------------------------------------------
  // Read operations
  // -----------------------------------------------------------------------

  /**
   * Get full on-chain prover information.
   */
  async getProverInfo(proverAddress: Hex): Promise<OnChainProverInfo> {
    const result = await this.publicClient.readContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName: 'getProver',
      args: [proverAddress],
    });

    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const r = result as any;
    return {
      owner: r.owner ?? r[0],
      stake: r.stake ?? r[1],
      reputation: r.reputation ?? r[2],
      proofsSubmitted: r.proofsSubmitted ?? r[3],
      proofsFailed: r.proofsFailed ?? r[4],
      totalEarnings: r.totalEarnings ?? r[5],
      registeredAt: r.registeredAt ?? r[6],
      lastActiveAt: r.lastActiveAt ?? r[7],
      active: r.active ?? r[8],
      endpoint: r.endpoint ?? r[9],
    };
  }

  /**
   * Check if an address is an active prover.
   */
  async isProverActive(proverAddress: Hex): Promise<boolean> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName: 'isActive',
      args: [proverAddress],
    }) as Promise<boolean>;
  }

  /**
   * Check if an address is a registered prover (active or inactive).
   */
  async isProver(proverAddress: Hex): Promise<boolean> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName: 'isProver',
      args: [proverAddress],
    }) as Promise<boolean>;
  }

  /**
   * Get all active prover addresses.
   */
  async getActiveProvers(): Promise<Hex[]> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName: 'getActiveProvers',
    }) as Promise<Hex[]>;
  }

  /**
   * Get the number of active provers.
   */
  async activeProverCount(): Promise<bigint> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName: 'activeProverCount',
    }) as Promise<bigint>;
  }

  /**
   * Get the effective weight of a prover (stake * reputation / 10000).
   */
  async getWeight(proverAddress: Hex): Promise<bigint> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName: 'getWeight',
      args: [proverAddress],
    }) as Promise<bigint>;
  }

  /**
   * Select a prover using weighted random selection based on stake and reputation.
   *
   * @param seed - Random seed (e.g., blockhash or VRF output)
   */
  async selectProver(seed: bigint): Promise<Hex> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName: 'selectProver',
      args: [seed],
    }) as Promise<Hex>;
  }

  /**
   * Get the top N provers by reputation.
   */
  async getTopProvers(n: bigint): Promise<Hex[]> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName: 'getTopProvers',
      args: [n],
    }) as Promise<Hex[]>;
  }

  // -----------------------------------------------------------------------
  // State accessors
  // -----------------------------------------------------------------------

  /** Get the staking token address. */
  async stakingToken(): Promise<Hex> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName: 'stakingToken',
    }) as Promise<Hex>;
  }

  /** Get the minimum stake required to be an active prover. */
  async minStake(): Promise<bigint> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName: 'minStake',
    }) as Promise<bigint>;
  }

  /** Get the slash basis points (e.g., 500 = 5%). */
  async slashBasisPoints(): Promise<bigint> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName: 'slashBasisPoints',
    }) as Promise<bigint>;
  }

  /** Get the total amount staked across all provers. */
  async totalStaked(): Promise<bigint> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName: 'totalStaked',
    }) as Promise<bigint>;
  }

  /** Get the contract owner address. */
  async owner(): Promise<Hex> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName: 'owner',
    }) as Promise<Hex>;
  }

  // -----------------------------------------------------------------------
  // Slashing & Rewards (authorized callers)
  // -----------------------------------------------------------------------

  /**
   * Slash a prover for misbehavior (authorized slashers or owner only).
   *
   * @param proverAddress - The prover to slash
   * @param reason - Human-readable reason
   * @returns The transaction hash
   */
  async slash(proverAddress: Hex, reason: string): Promise<Hex> {
    return this.send('slash', [proverAddress, reason]);
  }

  /**
   * Record a successful proof and distribute reward (authorized callers).
   *
   * @param proverAddress - The prover that succeeded
   * @param reward - Reward amount earned
   * @returns The transaction hash
   */
  async recordSuccess(proverAddress: Hex, reward: bigint): Promise<Hex> {
    return this.send('recordSuccess', [proverAddress, reward]);
  }

  // -----------------------------------------------------------------------
  // Admin operations (owner only)
  // -----------------------------------------------------------------------

  /** Set the minimum stake requirement (owner only). */
  async setMinStake(minStake: bigint): Promise<Hex> {
    return this.send('setMinStake', [minStake]);
  }

  /** Set the slash percentage in basis points (owner only, max 50% = 5000). */
  async setSlashBasisPoints(bps: bigint): Promise<Hex> {
    return this.send('setSlashBasisPoints', [bps]);
  }

  /** Authorize or deauthorize a slasher address (owner only). */
  async setSlasher(slasherAddress: Hex, authorized: boolean): Promise<Hex> {
    return this.send('setSlasher', [slasherAddress, authorized]);
  }

  // -----------------------------------------------------------------------
  // Internal helpers
  // -----------------------------------------------------------------------

  private requireWallet(): WalletClient {
    if (!this.walletClient) {
      throw new Error(
        'ProverRegistryClient: walletClient is required for write operations',
      );
    }
    return this.walletClient;
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private async send(functionName: string, args: any[], value?: bigint): Promise<Hex> {
    const wallet = this.requireWallet();

    const hash = await wallet.writeContract({
      address: this.contractAddress,
      abi: proverRegistryAbi,
      functionName,
      args,
      ...(value !== undefined ? { value } : {}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any);

    await this.publicClient.waitForTransactionReceipt({ hash });
    return hash;
  }
}
