/**
 * ExecutionEngine SDK client for on-chain verifiable computation.
 *
 * Wraps the ExecutionEngine.sol contract using viem, providing typed
 * methods for the full request lifecycle: submit, claim, prove, query.
 */
import type {
  PublicClient,
  WalletClient,
  GetContractReturnType,
} from 'viem';
import { getContract, parseEther } from 'viem';
import { executionEngineAbi } from './abi';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type Hex = `0x${string}`;

/** On-chain request status enum (mirrors ExecutionEngine.RequestStatus) */
export enum OnChainRequestStatus {
  Pending = 0,
  Claimed = 1,
  Completed = 2,
  Expired = 3,
  Cancelled = 4,
}

/** Decoded on-chain ExecutionRequest struct */
export interface OnChainExecutionRequest {
  id: bigint;
  imageId: Hex;
  inputDigest: Hex;
  requester: Hex;
  createdAt: number;
  expiresAt: number;
  callbackContract: Hex;
  status: OnChainRequestStatus;
  claimedBy: Hex;
  claimedAt: number;
  claimDeadline: number;
  tip: bigint;
  maxTip: bigint;
}

/** Options for {@link ExecutionEngineClient.submitRequest} */
export interface SubmitRequestOptions {
  /** Program image ID (bytes32) */
  imageId: Hex;
  /** Keccak256 hash of the inputs (bytes32) */
  inputDigest: Hex;
  /** URL where provers can fetch inputs (default: empty string) */
  inputUrl?: string;
  /** Contract that receives the callback (default: zero address) */
  callbackContract?: Hex;
  /** Seconds until the request expires (0 uses contract default of 1 hour) */
  expirationSeconds?: bigint;
  /** Input type: 0 = public, 1 = private (default: 0) */
  inputType?: number;
  /** Tip in ETH (e.g. '0.001'). Sent as msg.value. */
  tipEther: string;
}

/** On-chain prover statistics */
export interface OnChainProverStats {
  completed: bigint;
  earnings: bigint;
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/**
 * Typed client for the ExecutionEngine smart contract.
 *
 * Read-only operations (getRequestStatus, getRequest, etc.) only require a
 * `PublicClient`. Write operations (submitRequest, claimJob, submitResult,
 * disputeResult) additionally require a `WalletClient`.
 *
 * @example
 * ```ts
 * import { createPublicClient, createWalletClient, http } from 'viem';
 * import { privateKeyToAccount } from 'viem/accounts';
 * import { sepolia } from 'viem/chains';
 * import { ExecutionEngineClient } from '@worldzk/sdk';
 *
 * const publicClient = createPublicClient({ chain: sepolia, transport: http() });
 * const walletClient = createWalletClient({
 *   chain: sepolia,
 *   transport: http(),
 *   account: privateKeyToAccount('0x...'),
 * });
 *
 * const engine = new ExecutionEngineClient(publicClient, walletClient, '0x...');
 *
 * const requestId = await engine.submitRequest({
 *   imageId: '0x1234...',
 *   inputDigest: '0xabcd...',
 *   tipEther: '0.001',
 * });
 * ```
 */
export class ExecutionEngineClient {
  private readonly publicClient: PublicClient;
  private readonly walletClient: WalletClient | undefined;
  private readonly contractAddress: Hex;

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private readonly readContract: GetContractReturnType<typeof executionEngineAbi, any, Hex>;

  constructor(
    publicClient: PublicClient,
    walletClient: WalletClient | undefined,
    contractAddress: Hex,
  ) {
    this.publicClient = publicClient;
    this.walletClient = walletClient;
    this.contractAddress = contractAddress;

    this.readContract = getContract({
      address: contractAddress,
      abi: executionEngineAbi,
      client: publicClient,
    });
  }

  // -----------------------------------------------------------------------
  // Write operations
  // -----------------------------------------------------------------------

  /**
   * Submit a new execution request on-chain.
   *
   * Calls `requestExecution` on the ExecutionEngine contract. The tip is
   * sent as `msg.value` and must be >= MIN_TIP (0.0001 ETH).
   *
   * @returns The transaction hash
   */
  async submitRequest(options: SubmitRequestOptions): Promise<Hex> {
    const wallet = this.requireWallet();

    const hash = await wallet.writeContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName: 'requestExecution',
      args: [
        options.imageId,
        options.inputDigest,
        options.inputUrl ?? '',
        options.callbackContract ?? '0x0000000000000000000000000000000000000000',
        options.expirationSeconds ?? 0n,
        options.inputType ?? 0,
      ],
      value: parseEther(options.tipEther),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any);

    await this.publicClient.waitForTransactionReceipt({ hash });
    return hash;
  }

  /**
   * Claim an execution request as a prover.
   *
   * Calls `claimExecution(requestId)`. The caller becomes the assigned
   * prover and has CLAIM_WINDOW (10 min) to submit a proof.
   *
   * @returns The transaction hash
   */
  async claimJob(requestId: bigint): Promise<Hex> {
    const wallet = this.requireWallet();

    const hash = await wallet.writeContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName: 'claimExecution',
      args: [requestId],
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any);

    await this.publicClient.waitForTransactionReceipt({ hash });
    return hash;
  }

  /**
   * Submit a proof for a claimed execution request.
   *
   * Calls `submitProof(requestId, seal, journal)`. The contract verifies
   * the proof on-chain, pays the prover, and optionally triggers a
   * callback on the requester's contract.
   *
   * @param requestId - The request to fulfill
   * @param seal - The proof seal bytes (risc0 or Remainder)
   * @param journal - The public outputs (journal)
   * @returns The transaction hash
   */
  async submitResult(requestId: bigint, seal: Hex, journal: Hex): Promise<Hex> {
    const wallet = this.requireWallet();

    const hash = await wallet.writeContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName: 'submitProof',
      args: [requestId, seal, journal],
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any);

    await this.publicClient.waitForTransactionReceipt({ hash });
    return hash;
  }

  /**
   * Cancel a pending execution request (requester only).
   *
   * Calls `cancelExecution(requestId)`. The tip is refunded to the
   * original requester.
   *
   * @returns The transaction hash
   */
  async cancelRequest(requestId: bigint): Promise<Hex> {
    const wallet = this.requireWallet();

    const hash = await wallet.writeContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName: 'cancelExecution',
      args: [requestId],
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any);

    await this.publicClient.waitForTransactionReceipt({ hash });
    return hash;
  }

  /**
   * Dispute / reclaim an expired claim on a request.
   *
   * If a prover claimed a request but failed to submit a proof within
   * the CLAIM_WINDOW, any prover can call `claimExecution` again to
   * reclaim it. This is the "dispute" mechanism: the original claim
   * expires and a new prover takes over.
   *
   * @returns The transaction hash
   */
  async disputeResult(requestId: bigint): Promise<Hex> {
    // The contract's dispute mechanism is reclaiming an expired claim.
    // claimExecution handles this: if status == Claimed and deadline
    // passed, it emits ClaimExpired and allows reclaim.
    return this.claimJob(requestId);
  }

  // -----------------------------------------------------------------------
  // Read operations
  // -----------------------------------------------------------------------

  /**
   * Get the on-chain status of an execution request.
   *
   * @returns The {@link OnChainRequestStatus} enum value
   */
  async getRequestStatus(requestId: bigint): Promise<OnChainRequestStatus> {
    const req = await this.getRequest(requestId);
    return req.status;
  }

  /**
   * Fetch the full on-chain ExecutionRequest struct.
   */
  async getRequest(requestId: bigint): Promise<OnChainExecutionRequest> {
    const result = await this.publicClient.readContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName: 'getRequest',
      args: [requestId],
    });

    // The ABI returns a tuple. viem decodes it as an object with named fields.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const r = result as any;
    return {
      id: r.id,
      imageId: r.imageId,
      inputDigest: r.inputDigest,
      requester: r.requester,
      createdAt: Number(r.createdAt),
      expiresAt: Number(r.expiresAt),
      callbackContract: r.callbackContract,
      status: Number(r.status) as OnChainRequestStatus,
      claimedBy: r.claimedBy,
      claimedAt: Number(r.claimedAt),
      claimDeadline: Number(r.claimDeadline),
      tip: r.tip,
      maxTip: r.maxTip,
    };
  }

  /**
   * Get the current tip for a request (decreases over time).
   */
  async getCurrentTip(requestId: bigint): Promise<bigint> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName: 'getCurrentTip',
      args: [requestId],
    }) as Promise<bigint>;
  }

  /**
   * Get pending request IDs (for provers to find work).
   */
  async getPendingRequests(offset: bigint = 0n, limit: bigint = 20n): Promise<bigint[]> {
    const result = await this.publicClient.readContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName: 'getPendingRequests',
      args: [offset, limit],
    });
    return result as bigint[];
  }

  /**
   * Get prover statistics (completed count + earnings).
   */
  async getProverStats(prover: Hex): Promise<OnChainProverStats> {
    const result = await this.publicClient.readContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName: 'getProverStats',
      args: [prover],
    });
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const r = result as any;
    return {
      completed: r[0] ?? r.completed,
      earnings: r[1] ?? r.earnings,
    };
  }

  /**
   * Get the next request ID that will be assigned.
   */
  async getNextRequestId(): Promise<bigint> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName: 'nextRequestId',
    }) as Promise<bigint>;
  }

  /**
   * Check whether the contract is paused.
   */
  async isPaused(): Promise<boolean> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName: 'paused',
    }) as Promise<boolean>;
  }

  /**
   * Get the contract owner address.
   */
  async owner(): Promise<Hex> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName: 'owner',
    }) as Promise<Hex>;
  }

  /**
   * Get the protocol fee in basis points.
   */
  async getProtocolFeeBps(): Promise<bigint> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName: 'protocolFeeBps',
    }) as Promise<bigint>;
  }

  /**
   * Get the fee recipient address.
   */
  async getFeeRecipient(): Promise<Hex> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName: 'feeRecipient',
    }) as Promise<Hex>;
  }

  // -----------------------------------------------------------------------
  // Admin operations (owner only)
  // -----------------------------------------------------------------------

  /** Pause the contract (owner only). */
  async pause(): Promise<Hex> {
    return this.send('pause', []);
  }

  /** Unpause the contract (owner only). */
  async unpause(): Promise<Hex> {
    return this.send('unpause', []);
  }

  /** Set the protocol fee in basis points (owner only, max 1000 = 10%). */
  async setProtocolFee(feeBps: bigint): Promise<Hex> {
    return this.send('setProtocolFee', [feeBps]);
  }

  /** Set the fee recipient address (owner only). */
  async setFeeRecipient(recipient: Hex): Promise<Hex> {
    return this.send('setFeeRecipient', [recipient]);
  }

  /** Set or update the reputation contract (owner only). */
  async setReputation(reputationAddress: Hex): Promise<Hex> {
    return this.send('setReputation', [reputationAddress]);
  }

  /** Initiate ownership transfer (owner only, Ownable2Step). */
  async transferOwnership(newOwner: Hex): Promise<Hex> {
    return this.send('transferOwnership', [newOwner]);
  }

  /** Accept pending ownership transfer (new owner only). */
  async acceptOwnership(): Promise<Hex> {
    return this.send('acceptOwnership', []);
  }

  // -----------------------------------------------------------------------
  // Internal helpers
  // -----------------------------------------------------------------------

  private requireWallet(): WalletClient {
    if (!this.walletClient) {
      throw new Error(
        'ExecutionEngineClient: walletClient is required for write operations',
      );
    }
    return this.walletClient;
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private async send(functionName: string, args: any[], value?: bigint): Promise<Hex> {
    const wallet = this.requireWallet();

    const hash = await wallet.writeContract({
      address: this.contractAddress,
      abi: executionEngineAbi,
      functionName,
      args,
      ...(value !== undefined ? { value } : {}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any);

    await this.publicClient.waitForTransactionReceipt({ hash });
    return hash;
  }
}
