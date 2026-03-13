import {
  createPublicClient,
  http,
  webSocket,
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
  /** Optional WebSocket URL. If provided, watchEvents() will use WebSocket subscriptions. */
  wsUrl?: string;
}

export type TEEEventName =
  | 'ResultSubmitted'
  | 'ResultChallenged'
  | 'ResultFinalized'
  | 'ResultExpired'
  | 'DisputeResolved'
  | 'EnclaveRegistered'
  | 'EnclaveRevoked';

export interface ResultSubmittedEvent {
  resultId: Hex;
  modelHash: Hex;
  inputHash: Hex;
  submitter: Hex;
}

export interface ResultChallengedEvent {
  resultId: Hex;
  challenger: Hex;
}

export interface ResultFinalizedEvent {
  resultId: Hex;
}

export interface ResultExpiredEvent {
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
  | { event: 'ResultExpired'; data: ResultExpiredEvent }
  | { event: 'DisputeResolved'; data: DisputeResolvedEvent }
  | { event: 'EnclaveRegistered'; data: EnclaveRegisteredEvent }
  | { event: 'EnclaveRevoked'; data: EnclaveRevokedEvent };

type EventHandler<T> = (data: T, log: Log) => void;

/** Options for the watchEvents() WebSocket subscription method. */
export interface WatchEventsOptions {
  /** Which event types to listen for. If omitted, all events are delivered. */
  eventTypes?: TEEEventName[];
  /** Whether to automatically reconnect on WebSocket failure. Defaults to true. */
  reconnect?: boolean;
  /** Maximum number of reconnection attempts. Defaults to 5. */
  maxRetries?: number;
}

/** Callback signature for watchEvents(). */
export type WatchEventsCallback = (event: TEEEventData, log: Log) => void;

/** Error callback for watchEvents(). */
export type WatchEventsErrorCallback = (error: Error) => void;

/** Represents an active WebSocket subscription returned by watchEvents(). */
export interface EventSubscription {
  /** Unique identifier for this subscription. */
  readonly id: string;
  /** Whether this subscription is currently active. */
  readonly active: boolean;
  /** Stop this subscription. */
  unsubscribe(): void;
}

/** Internal state for a single watchEvents() subscription. */
interface SubscriptionState {
  id: string;
  active: boolean;
  callback: WatchEventsCallback;
  errorCallback?: WatchEventsErrorCallback;
  options: Required<WatchEventsOptions>;
  retryCount: number;
  unwatchFn: WatchContractEventReturnType | null;
  retryTimer: ReturnType<typeof setTimeout> | null;
}

const BASE_RETRY_DELAY_MS = 1000;
const MAX_RETRY_DELAY_MS = 16000;

let subscriptionCounter = 0;

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
 *
 * @example WebSocket subscription
 * ```typescript
 * const watcher = new TEEEventWatcher({
 *   rpcUrl: 'http://localhost:8545',
 *   wsUrl: 'ws://localhost:8545',
 *   contractAddress: '0x...',
 * });
 *
 * const sub = watcher.watchEvents(
 *   (event, log) => console.log(event),
 *   { eventTypes: ['ResultSubmitted', 'ResultChallenged'], reconnect: true },
 * );
 *
 * // Later:
 * sub.unsubscribe();
 * ```
 */
export class TEEEventWatcher {
  private client: PublicClient<Transport, Chain>;
  private contractAddress: Hex;
  private handlers: Map<TEEEventName, EventHandler<any>[]> = new Map();
  private unwatch: WatchContractEventReturnType | null = null;
  private wsUrl: string | undefined;
  private chainDef: Chain;
  private subscriptions: Map<string, SubscriptionState> = new Map();

  constructor(config: TEEEventWatcherConfig) {
    this.chainDef = defineChain({
      id: config.chainId ?? 31337,
      name: 'Custom',
      nativeCurrency: { name: 'Ether', symbol: 'ETH', decimals: 18 },
      rpcUrls: { default: { http: [config.rpcUrl] } },
    });

    this.wsUrl = config.wsUrl;
    this.client = createPublicClient({ chain: this.chainDef, transport: http(config.rpcUrl) });
    this.contractAddress = config.contractAddress;
  }

  /**
   * Register a handler for a specific event type.
   */
  on(event: 'ResultSubmitted', handler: EventHandler<ResultSubmittedEvent>): this;
  on(event: 'ResultChallenged', handler: EventHandler<ResultChallengedEvent>): this;
  on(event: 'ResultFinalized', handler: EventHandler<ResultFinalizedEvent>): this;
  on(event: 'ResultExpired', handler: EventHandler<ResultExpiredEvent>): this;
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

  /** Whether the watcher is currently active (start/stop based). */
  get isWatching(): boolean {
    return this.unwatch !== null;
  }

  /** Whether a WebSocket URL was provided and WebSocket transport is available. */
  get hasWebSocket(): boolean {
    return this.wsUrl !== undefined && this.isWebSocketUrl(this.wsUrl);
  }

  /** Number of currently active watchEvents() subscriptions. */
  get activeSubscriptionCount(): number {
    let count = 0;
    for (const sub of this.subscriptions.values()) {
      if (sub.active) count++;
    }
    return count;
  }

  /**
   * Watch for events using WebSocket subscription with automatic reconnection.
   *
   * If a `wsUrl` was provided in the config, this uses a WebSocket transport for
   * true push-based event delivery. Otherwise, it falls back to HTTP polling via
   * viem's watchContractEvent (same transport as start()).
   *
   * @param callback - Called for each matching event.
   * @param options - Optional filtering and reconnection settings.
   * @param onError - Optional error callback invoked on WebSocket errors or max retries exceeded.
   * @returns An EventSubscription handle for unsubscribing.
   */
  watchEvents(
    callback: WatchEventsCallback,
    options?: WatchEventsOptions,
    onError?: WatchEventsErrorCallback,
  ): EventSubscription {
    const id = `sub_${++subscriptionCounter}`;
    const resolvedOptions: Required<WatchEventsOptions> = {
      eventTypes: options?.eventTypes ?? [],
      reconnect: options?.reconnect ?? true,
      maxRetries: options?.maxRetries ?? 5,
    };

    const state: SubscriptionState = {
      id,
      active: true,
      callback,
      errorCallback: onError,
      options: resolvedOptions,
      retryCount: 0,
      unwatchFn: null,
      retryTimer: null,
    };

    this.subscriptions.set(id, state);
    this.startSubscription(state);

    const self = this;
    return {
      get id() {
        return id;
      },
      get active() {
        const s = self.subscriptions.get(id);
        return s?.active ?? false;
      },
      unsubscribe() {
        self.unsubscribeById(id);
      },
    };
  }

  /**
   * Stop a specific subscription by its ID.
   */
  unsubscribe(subscriptionId: string): void {
    this.unsubscribeById(subscriptionId);
  }

  /**
   * Stop all active watchEvents() subscriptions.
   */
  unsubscribeAll(): void {
    for (const id of [...this.subscriptions.keys()]) {
      this.unsubscribeById(id);
    }
  }

  // ---- Private helpers ----

  private unsubscribeById(id: string): void {
    const state = this.subscriptions.get(id);
    if (!state) return;

    state.active = false;
    if (state.unwatchFn) {
      state.unwatchFn();
      state.unwatchFn = null;
    }
    if (state.retryTimer !== null) {
      clearTimeout(state.retryTimer);
      state.retryTimer = null;
    }
    this.subscriptions.delete(id);
  }

  private startSubscription(state: SubscriptionState): void {
    if (!state.active) return;

    const useWebSocket = this.wsUrl !== undefined && this.isWebSocketUrl(this.wsUrl);

    if (useWebSocket) {
      this.startWebSocketSubscription(state);
    } else {
      this.startPollingSubscription(state);
    }
  }

  private startWebSocketSubscription(state: SubscriptionState): void {
    if (!state.active || !this.wsUrl) return;

    try {
      const wsClient = createPublicClient({
        chain: this.chainDef,
        transport: webSocket(this.wsUrl, {
          reconnect: false, // We handle reconnection ourselves
        }),
      });

      state.unwatchFn = wsClient.watchContractEvent({
        address: this.contractAddress,
        abi: teeMLVerifierAbi,
        onLogs: (logs) => {
          // Reset retry count on successful message
          state.retryCount = 0;
          for (const log of logs) {
            this.dispatchToSubscription(state, log);
          }
        },
        onError: (error: Error) => {
          this.handleSubscriptionError(state, error);
        },
      });
    } catch (error) {
      this.handleSubscriptionError(
        state,
        error instanceof Error ? error : new Error(String(error)),
      );
    }
  }

  private startPollingSubscription(state: SubscriptionState): void {
    if (!state.active) return;

    state.unwatchFn = this.client.watchContractEvent({
      address: this.contractAddress,
      abi: teeMLVerifierAbi,
      onLogs: (logs) => {
        for (const log of logs) {
          this.dispatchToSubscription(state, log);
        }
      },
    });
  }

  private handleSubscriptionError(state: SubscriptionState, error: Error): void {
    if (!state.active) return;

    // Clean up the current watcher
    if (state.unwatchFn) {
      try {
        state.unwatchFn();
      } catch {
        // Ignore cleanup errors
      }
      state.unwatchFn = null;
    }

    if (state.options.reconnect && state.retryCount < state.options.maxRetries) {
      state.retryCount++;
      const delay = Math.min(
        BASE_RETRY_DELAY_MS * Math.pow(2, state.retryCount - 1),
        MAX_RETRY_DELAY_MS,
      );

      state.retryTimer = setTimeout(() => {
        state.retryTimer = null;
        if (state.active) {
          this.startSubscription(state);
        }
      }, delay);
    } else {
      // Max retries exceeded or reconnect disabled
      state.active = false;
      this.subscriptions.delete(state.id);
      if (state.errorCallback) {
        const msg = state.options.reconnect
          ? `WebSocket connection failed after ${state.options.maxRetries} retries: ${error.message}`
          : `WebSocket connection failed: ${error.message}`;
        state.errorCallback(new Error(msg));
      }
    }
  }

  private dispatchToSubscription(state: SubscriptionState, log: Log): void {
    if (!state.active) return;

    const parsed = this.parseLog(log as any);
    if (!parsed) return;

    // Apply event type filter
    if (
      state.options.eventTypes.length > 0 &&
      !state.options.eventTypes.includes(parsed.event)
    ) {
      return;
    }

    state.callback(parsed, log);
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
            submitter: args.submitter,
          },
        };
      case 'ResultChallenged':
        return {
          event: 'ResultChallenged',
          data: { resultId: args.resultId, challenger: args.challenger },
        };
      case 'ResultFinalized':
        return { event: 'ResultFinalized', data: { resultId: args.resultId } };
      case 'ResultExpired':
        return { event: 'ResultExpired', data: { resultId: args.resultId } };
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

  private isWebSocketUrl(url: string): boolean {
    return url.startsWith('ws://') || url.startsWith('wss://');
  }
}
