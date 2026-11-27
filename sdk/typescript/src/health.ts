/**
 * Standalone health check utilities for the World ZK Compute indexer.
 *
 * These functions allow checking indexer health without constructing
 * a full {@link Client} instance.
 *
 * @example
 * ```typescript
 * import { checkHealth, checkReady } from '@worldzk/sdk';
 *
 * const health = await checkHealth('http://localhost:8081');
 * console.log(health.status, health.lastIndexedBlock);
 *
 * const ready = await checkReady('http://localhost:8081');
 * console.log(ready.ready, ready.checks);
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

/**
 * Response returned by the indexer's `GET /ready` endpoint.
 *
 * Provides a top-level readiness flag together with per-subsystem checks
 * (e.g. database connectivity, indexing pipeline health).
 */
export interface IndexerReadyResponse {
  /** `true` when all subsystem checks pass. */
  ready: boolean;
  /** Per-subsystem readiness details. */
  checks: {
    /** Database connectivity status (`"ok"` or an error description). */
    db: string;
    /** Indexing pipeline status (`"ok"` or an error description). */
    indexing: string;
  };
}

/** Options for {@link checkHealth}. */
export interface CheckHealthOptions {
  /**
   * Request timeout in milliseconds.
   * @default 5000
   */
  timeout?: number;
}

/** Options for {@link checkReady}. */
export interface CheckReadyOptions {
  /**
   * Request timeout in milliseconds.
   * @default 5000
   */
  timeout?: number;
}

const DEFAULT_HEALTH_TIMEOUT = 5000;

/**
 * Read the JSON body of an indexer `/health` or `/ready` answer.
 *
 * The indexer answers 200, or 503 with the same JSON body when it is
 * degraded or not ready. Any other status (the rate limiter's 429, a proxy's
 * 502) carries no health report, so it is a failed check, not an unhealthy
 * indexer.
 */
async function readIndexerJson(
  response: Response,
  check: string,
): Promise<Record<string, unknown>> {
  if (response.status !== 200 && response.status !== 503) {
    throw new NetworkError(
      'WZK-5000',
      `Indexer ${check} check failed: HTTP ${response.status}`,
      response.status,
    );
  }
  let data: unknown;
  try {
    data = await response.json();
  } catch (e) {
    // A timeout while the body streams in must stay a timeout.
    if (e instanceof Error && e.name === 'AbortError') {
      throw e;
    }
    data = undefined;
  }
  if (typeof data !== 'object' || data === null || Array.isArray(data)) {
    throw new NetworkError(
      'WZK-5000',
      `Indexer ${check} check failed: HTTP ${response.status} without a JSON object body`,
      response.status,
    );
  }
  return data as Record<string, unknown>;
}

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
 * @throws {ServiceHealthError} If the indexer reports a non-"ok" status, or answers 503 with a JSON health body.
 * @throws {TimeoutError} If the request, including reading the body, exceeds `options.timeout` (default 5000 ms).
 * @throws {NetworkError} If the request fails due to a network issue (connection refused, DNS failure, etc.),
 *   the HTTP status is neither 200 nor 503, or the body is not a JSON object.
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

    const data = await readIndexerJson(response, 'health');

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
    if (response.status !== 200) {
      throw new ServiceHealthError(
        status,
        lastIndexedBlock,
        totalResults,
        `Indexer is unhealthy: HTTP ${response.status} with status="${status}"`,
      );
    }

    return result;
  } catch (e) {
    if (e instanceof ServiceHealthError || e instanceof NetworkError) {
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
  } finally {
    // Cleared only now so that the deadline also covers reading the body.
    clearTimeout(timeoutId);
  }
}

/**
 * Check the readiness of an indexer service.
 *
 * Makes a `GET /ready` request to the given indexer URL and returns a typed
 * {@link IndexerReadyResponse}. This is a standalone function that does not
 * require a {@link Client} instance.
 *
 * Unlike {@link checkHealth}, this function does **not** throw when the
 * service reports `ready: false` (HTTP 503) -- it simply returns the
 * response so callers can inspect individual checks.
 *
 * @param indexerUrl - Base URL of the indexer (e.g. `"http://localhost:8081"`).
 * @param options - Optional timeout configuration.
 * @returns The parsed readiness response.
 *
 * @throws {TimeoutError} If the request, including reading the body, exceeds `options.timeout` (default 5000 ms).
 * @throws {NetworkError} If the request fails due to a network issue (connection refused, DNS failure, etc.),
 *   the HTTP status is neither 200 nor 503, or the body is not a JSON object.
 *
 * @example
 * ```typescript
 * const ready = await checkReady('http://localhost:8081');
 * if (!ready.ready) {
 *   console.error('DB:', ready.checks.db, 'Indexing:', ready.checks.indexing);
 * }
 * ```
 */
export async function checkReady(
  indexerUrl: string,
  options: CheckReadyOptions = {},
): Promise<IndexerReadyResponse> {
  const url = indexerUrl.replace(/\/$/, '');
  const timeoutMs = options.timeout ?? DEFAULT_HEALTH_TIMEOUT;

  const controller = new AbortController();
  const timeoutId = setTimeout(() => controller.abort(), timeoutMs);

  try {
    const response = await fetch(`${url}/ready`, {
      method: 'GET',
      headers: { Accept: 'application/json' },
      signal: controller.signal,
    });

    const data = await readIndexerJson(response, 'ready');

    // A 503 means not ready whatever the body says.
    const ready = data.ready === true && response.status === 200;
    const rawChecks = (data.checks as Record<string, unknown>) ?? {};

    return {
      ready,
      checks: {
        db: (rawChecks.db as string) ?? 'unknown',
        indexing: (rawChecks.indexing as string) ?? 'unknown',
      },
    };
  } catch (e) {
    if (e instanceof NetworkError) {
      throw e;
    }

    if (e instanceof Error) {
      if (e.name === 'AbortError') {
        throw new TimeoutError(
          `Indexer ready check timed out after ${timeoutMs}ms`,
          timeoutMs,
        );
      }
      throw new NetworkError(
        'WZK-5000',
        `Indexer ready check failed: ${e.message}`,
        0,
      );
    }

    throw e;
  } finally {
    // Cleared only now so that the deadline also covers reading the body.
    clearTimeout(timeoutId);
  }
}
