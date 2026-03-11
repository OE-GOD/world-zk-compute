import {
  createPublicClient,
  http,
  type PublicClient,
  type Transport,
  type Chain,
  type Log,
  type WatchContractEventReturnType,
  defineChain,
} from 'viem';
import { teeMLVerifierAbi } from './tee-verifier-abi';

type Hex = `0x${string}`;

export interface TEEEventWatcherConfig {
  rpcUrl: string;
  contractAddress: Hex;
  chainId?: number;
}

export type TEEEventName =
  | 'ResultSubmitted'
  | 'ResultChallenged'
  | 'ResultFinalized'
  | 'DisputeResolved'
  | 'EnclaveRegistered'
  | 'EnclaveRevoked';

export interface ResultSubmittedEvent {
  resultId: Hex;
  modelHash: Hex;
  inputHash: Hex;
}

export interface ResultChallengedEvent {
  resultId: Hex;
  challenger: Hex;
}

export interface ResultFinalizedEvent {
  resultId: Hex;
}

export interface DisputeResolvedEvent {
  resultId: Hex;
  proverWon: boolean;
}

export interface EnclaveRegisteredEvent {
  enclaveKey: Hex;
  enclaveImageHash: Hex;
}

export interface EnclaveRevokedEvent {
  enclaveKey: Hex;
}

export type TEEEventData =
  | { event: 'ResultSubmitted'; data: ResultSubmittedEvent }
  | { event: 'ResultChallenged'; data: ResultChallengedEvent }
  | { event: 'ResultFinalized'; data: ResultFinalizedEvent }
  | { event: 'DisputeResolved'; data: DisputeResolvedEvent }
  | { event: 'EnclaveRegistered'; data: EnclaveRegisteredEvent }
  | { event: 'EnclaveRevoked'; data: EnclaveRevokedEvent };

type EventHandler<T> = (data: T, log: Log) => void;

/**
 * Watches TEEMLVerifier contract events.
 *
 * @example
 * ```typescript
 * const watcher = new TEEEventWatcher({
 *   rpcUrl: 'http://localhost:8545',
 *   contractAddress: '0x...',
 * });
 *
 * watcher.on('ResultSubmitted', (data) => {
 *   console.log('New result:', data.resultId);
 * });
 *
 * watcher.on('ResultChallenged', (data) => {
 *   console.log('Result challenged:', data.resultId, 'by', data.challenger);
 * });
 *
 * // Start watching
 * watcher.start();
 *
 * // Stop watching
 * watcher.stop();
 * ```
 */
export class TEEEventWatcher {
  private client: PublicClient<Transport, Chain>;
  private contractAddress: Hex;
  private handlers: Map<TEEEventName, EventHandler<any>[]> = new Map();
  private unwatch: WatchContractEventReturnType | null = null;

  constructor(config: TEEEventWatcherConfig) {
    const chain = defineChain({
      id: config.chainId ?? 31337,
      name: 'Custom',
      nativeCurrency: { name: 'Ether', symbol: 'ETH', decimals: 18 },
      rpcUrls: { default: { http: [config.rpcUrl] } },
    });

    this.client = createPublicClient({ chain, transport: http(config.rpcUrl) });
    this.contractAddress = config.contractAddress;
  }

  /**
   * Register a handler for a specific event type.
   */
  on(event: 'ResultSubmitted', handler: EventHandler<ResultSubmittedEvent>): this;
  on(event: 'ResultChallenged', handler: EventHandler<ResultChallengedEvent>): this;
  on(event: 'ResultFinalized', handler: EventHandler<ResultFinalizedEvent>): this;
  on(event: 'DisputeResolved', handler: EventHandler<DisputeResolvedEvent>): this;
  on(event: 'EnclaveRegistered', handler: EventHandler<EnclaveRegisteredEvent>): this;
  on(event: 'EnclaveRevoked', handler: EventHandler<EnclaveRevokedEvent>): this;
  on(event: TEEEventName, handler: EventHandler<any>): this {
    const handlers = this.handlers.get(event) ?? [];
    handlers.push(handler);
    this.handlers.set(event, handlers);
    return this;
  }

  /**
   * Remove a handler for a specific event type.
   */
  off(event: TEEEventName, handler: EventHandler<any>): this {
    const handlers = this.handlers.get(event);
    if (handlers) {
      const idx = handlers.indexOf(handler);
      if (idx !== -1) handlers.splice(idx, 1);
    }
    return this;
  }

  /**
   * Start watching for events. Uses viem's watchContractEvent for real-time updates.
   */
  start(): this {
    if (this.unwatch) return this;

    this.unwatch = this.client.watchContractEvent({
      address: this.contractAddress,
      abi: teeMLVerifierAbi,
      onLogs: (logs) => {
        for (const log of logs) {
          this.dispatch(log);
        }
      },
    });

    return this;
  }

  /**
   * Stop watching for events.
   */
  stop(): void {
    if (this.unwatch) {
      this.unwatch();
      this.unwatch = null;
    }
  }

  /**
   * Query past events from a block range.
   */
  async getPastEvents(
    eventName?: TEEEventName,
    fromBlock: bigint = 0n,
    toBlock: bigint | 'latest' = 'latest',
  ): Promise<TEEEventData[]> {
    const logs = await this.client.getContractEvents({
      address: this.contractAddress,
      abi: teeMLVerifierAbi,
      ...(eventName ? { eventName } : {}),
      fromBlock,
      toBlock,
    });

    return logs
      .map((log) => this.parseLog(log as any))
      .filter((e): e is TEEEventData => e !== null);
  }

  /** Whether the watcher is currently active. */
  get isWatching(): boolean {
    return this.unwatch !== null;
  }

  private dispatch(log: Log): void {
    const parsed = this.parseLog(log as any);
    if (!parsed) return;

    const handlers = this.handlers.get(parsed.event) ?? [];
    for (const handler of handlers) {
      handler(parsed.data, log);
    }
  }

  private parseLog(log: { eventName?: string; args?: Record<string, any> }): TEEEventData | null {
    const name = log.eventName as TEEEventName | undefined;
    const args = log.args ?? {};

    switch (name) {
      case 'ResultSubmitted':
        return {
          event: 'ResultSubmitted',
          data: {
            resultId: args.resultId,
            modelHash: args.modelHash,
            inputHash: args.inputHash,
          },
        };
      case 'ResultChallenged':
        return {
          event: 'ResultChallenged',
          data: { resultId: args.resultId, challenger: args.challenger },
        };
      case 'ResultFinalized':
        return { event: 'ResultFinalized', data: { resultId: args.resultId } };
      case 'DisputeResolved':
        return {
          event: 'DisputeResolved',
          data: { resultId: args.resultId, proverWon: args.proverWon },
        };
      case 'EnclaveRegistered':
        return {
          event: 'EnclaveRegistered',
          data: { enclaveKey: args.enclaveKey, enclaveImageHash: args.enclaveImageHash },
        };
      case 'EnclaveRevoked':
        return { event: 'EnclaveRevoked', data: { enclaveKey: args.enclaveKey } };
      default:
        return null;
    }
  }
}
