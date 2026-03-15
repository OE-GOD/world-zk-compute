/**
 * Standalone health check utilities for the World ZK Compute indexer.
 *
 * These functions allow checking indexer health without constructing
 * a full {@link Client} instance.
 *
 * @example
 * ```typescript
 * import { checkHealth } from '@worldzk/sdk';
 *
 * const health = await checkHealth('http://localhost:8081');
 * console.log(health.status, health.lastIndexedBlock);
 * ```
 */

import { NetworkError, TimeoutError, ServiceHealthError } from './errors';

/**
 * Response returned by the indexer's `GET /health` endpoint.
 */
export interface IndexerHealthResponse {
  /** `"ok"` when healthy, `"degraded"` when the storage backend is impaired. */
  status: string;
  /** The most recent block number the indexer has processed. */
  lastIndexedBlock: number;
  /** Total number of result rows stored by the indexer. */
  totalResults: number;
  /** Service version string, if provided by the indexer. */
  version?: string;
  /** Uptime in seconds, if provided by the indexer. */
  uptimeSeconds?: number;
}

/** Options for {@link checkHealth}. */
export interface CheckHealthOptions {
  /**
   * Request timeout in milliseconds.
   * @default 5000
   */
  timeout?: number;
}

const DEFAULT_HEALTH_TIMEOUT = 5000;

/**
 * Check the health of an indexer service.
 *
 * Makes a `GET /health` request to the given indexer URL and returns a typed
 * {@link IndexerHealthResponse}. This is a standalone function that does not
 * require a {@link Client} instance.
 *
 * @param indexerUrl - Base URL of the indexer (e.g. `"http://localhost:8081"`).
 * @param options - Optional timeout configuration.
 * @returns The parsed health response.
 *
 * @throws {ServiceHealthError} If the indexer reports a non-"ok" status.
 * @throws {TimeoutError} If the request exceeds the configured timeout.
 * @throws {NetworkError} If the request fails due to a network issue (connection refused, DNS failure, etc.).
 *
 * @example
 * ```typescript
 * try {
 *   const health = await checkHealth('http://localhost:8081');
 *   console.log(`Indexer OK: block ${health.lastIndexedBlock}`);
 * } catch (e) {
 *   if (e instanceof ServiceHealthError) {
 *     console.error(`Indexer degraded: ${e.status}`);
 *   }
 * }
 * ```
 */
export async function checkHealth(
  indexerUrl: string,
  options: CheckHealthOptions = {},
): Promise<IndexerHealthResponse> {
  const url = indexerUrl.replace(/\/$/, '');
  const timeoutMs = options.timeout ?? DEFAULT_HEALTH_TIMEOUT;

  const controller = new AbortController();
  const timeoutId = setTimeout(() => controller.abort(), timeoutMs);

  try {
    const response = await fetch(`${url}/health`, {
      method: 'GET',
      headers: { Accept: 'application/json' },
      signal: controller.signal,
    });

    clearTimeout(timeoutId);

    const data = (await response.json()) as Record<string, unknown>;

    const status = (data.status as string) ?? 'unknown';
    const lastIndexedBlock = (data.last_indexed_block as number) ?? 0;
    const totalResults = (data.total_results as number) ?? 0;
    const version = data.version as string | undefined;
    const uptimeSeconds = data.uptime_seconds as number | undefined;

    const result: IndexerHealthResponse = {
      status,
      lastIndexedBlock,
      totalResults,
      version,
      uptimeSeconds,
    };

    if (status !== 'ok') {
      throw new ServiceHealthError(status, lastIndexedBlock, totalResults);
    }

    return result;
  } catch (e) {
    clearTimeout(timeoutId);

    if (e instanceof ServiceHealthError) {
      throw e;
    }

    if (e instanceof Error) {
      if (e.name === 'AbortError') {
        throw new TimeoutError(
          `Indexer health check timed out after ${timeoutMs}ms`,
          timeoutMs,
        );
      }
      throw new NetworkError(
        'WZK-5000',
        `Indexer health check failed: ${e.message}`,
        0,
      );
    }

    throw e;
  }
}
