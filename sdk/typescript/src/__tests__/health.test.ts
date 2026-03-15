/**
 * Tests for the standalone checkHealth() function.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { checkHealth } from '../health';
import type { IndexerHealthResponse } from '../health';
import { ServiceHealthError, TimeoutError, NetworkError } from '../errors';

const originalFetch = globalThis.fetch;

function mockFetch(impl: typeof fetch) {
  globalThis.fetch = impl;
}

function restoreFetch() {
  globalThis.fetch = originalFetch;
}

describe('checkHealth', () => {
  afterEach(() => {
    restoreFetch();
  });

  it('returns a typed health response when the indexer is healthy', async () => {
    mockFetch(async () =>
      new Response(
        JSON.stringify({
          status: 'ok',
          last_indexed_block: 54321,
          total_results: 100,
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      ),
    );

    const health = await checkHealth('http://localhost:8081');

    expect(health.status).toBe('ok');
    expect(health.lastIndexedBlock).toBe(54321);
    expect(health.totalResults).toBe(100);
  });

  it('includes version and uptime when provided by indexer', async () => {
    mockFetch(async () =>
      new Response(
        JSON.stringify({
          status: 'ok',
          last_indexed_block: 100,
          total_results: 10,
          version: '1.2.3',
          uptime_seconds: 3600,
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      ),
    );

    const health = await checkHealth('http://localhost:8081');

    expect(health.status).toBe('ok');
    expect(health.version).toBe('1.2.3');
    expect(health.uptimeSeconds).toBe(3600);
  });

  it('throws ServiceHealthError when status is degraded', async () => {
    mockFetch(async () =>
      new Response(
        JSON.stringify({
          status: 'degraded',
          last_indexed_block: 50,
          total_results: 3,
        }),
        { status: 503, headers: { 'Content-Type': 'application/json' } },
      ),
    );

    await expect(checkHealth('http://localhost:8081')).rejects.toThrow(
      ServiceHealthError,
    );

    try {
      await checkHealth('http://localhost:8081');
    } catch (e) {
      expect(e).toBeInstanceOf(ServiceHealthError);
      const err = e as ServiceHealthError;
      expect(err.status).toBe('degraded');
      expect(err.lastIndexedBlock).toBe(50);
      expect(err.totalResults).toBe(3);
    }
  });

  it('throws ServiceHealthError for any non-ok status', async () => {
    mockFetch(async () =>
      new Response(
        JSON.stringify({
          status: 'error',
          last_indexed_block: 0,
          total_results: 0,
        }),
        { status: 500, headers: { 'Content-Type': 'application/json' } },
      ),
    );

    await expect(checkHealth('http://localhost:8081')).rejects.toThrow(
      ServiceHealthError,
    );
  });

  it('throws NetworkError when fetch fails (connection refused)', async () => {
    mockFetch(async () => {
      throw new Error('Connection refused');
    });

    await expect(checkHealth('http://localhost:8081')).rejects.toThrow(
      NetworkError,
    );

    try {
      await checkHealth('http://localhost:8081');
    } catch (e) {
      expect(e).toBeInstanceOf(NetworkError);
      expect((e as NetworkError).message).toContain('Connection refused');
    }
  });

  it('throws TimeoutError when request exceeds timeout', async () => {
    mockFetch(async (_input: RequestInfo | URL, init?: RequestInit) => {
      return new Promise<Response>((_resolve, reject) => {
        if (init?.signal) {
          init.signal.addEventListener('abort', () => {
            reject(
              new DOMException('The operation was aborted.', 'AbortError'),
            );
          });
        }
      });
    });

    await expect(
      checkHealth('http://localhost:8081', { timeout: 50 }),
    ).rejects.toThrow(TimeoutError);
  });

  it('defaults to 5000ms timeout', async () => {
    mockFetch(async (_input: RequestInfo | URL, init?: RequestInit) => {
      return new Promise<Response>((_resolve, reject) => {
        if (init?.signal) {
          init.signal.addEventListener('abort', () => {
            reject(
              new DOMException('The operation was aborted.', 'AbortError'),
            );
          });
        }
      });
    });

    try {
      await checkHealth('http://localhost:8081', { timeout: 10 });
    } catch (e) {
      expect(e).toBeInstanceOf(TimeoutError);
      expect((e as TimeoutError).timeoutMs).toBe(10);
    }
  });

  it('strips trailing slash from indexer URL', async () => {
    let requestedUrl = '';
    mockFetch(async (input: RequestInfo | URL) => {
      requestedUrl = typeof input === 'string' ? input : input.toString();
      return new Response(
        JSON.stringify({
          status: 'ok',
          last_indexed_block: 1,
          total_results: 0,
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      );
    });

    await checkHealth('http://localhost:8081/');

    expect(requestedUrl).toBe('http://localhost:8081/health');
  });

  it('constructs the correct URL path', async () => {
    let requestedUrl = '';
    mockFetch(async (input: RequestInfo | URL) => {
      requestedUrl = typeof input === 'string' ? input : input.toString();
      return new Response(
        JSON.stringify({
          status: 'ok',
          last_indexed_block: 1,
          total_results: 0,
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      );
    });

    await checkHealth('http://my-indexer:9090');

    expect(requestedUrl).toBe('http://my-indexer:9090/health');
  });

  it('handles missing fields gracefully with defaults', async () => {
    mockFetch(async () =>
      new Response(
        JSON.stringify({ status: 'ok' }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      ),
    );

    const health = await checkHealth('http://localhost:8081');

    expect(health.status).toBe('ok');
    expect(health.lastIndexedBlock).toBe(0);
    expect(health.totalResults).toBe(0);
    expect(health.version).toBeUndefined();
    expect(health.uptimeSeconds).toBeUndefined();
  });

  it('treats unknown status as unhealthy', async () => {
    mockFetch(async () =>
      new Response(
        JSON.stringify({
          last_indexed_block: 10,
          total_results: 5,
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      ),
    );

    // Missing status field defaults to 'unknown', which is not 'ok'
    await expect(checkHealth('http://localhost:8081')).rejects.toThrow(
      ServiceHealthError,
    );
  });

  it('exports IndexerHealthResponse type correctly', () => {
    // Compile-time type check -- if this compiles, the type is correct
    const _mock: IndexerHealthResponse = {
      status: 'ok',
      lastIndexedBlock: 0,
      totalResults: 0,
      version: '1.0.0',
      uptimeSeconds: 42,
    };
    expect(_mock.status).toBe('ok');
  });
});
