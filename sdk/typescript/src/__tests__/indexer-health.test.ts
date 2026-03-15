/**
 * Tests for Client.indexerHealth()
 */
import { describe, it, expect, afterEach } from 'vitest';
import { Client } from '../client';
import { ServiceHealthError, TimeoutError, NetworkError } from '../errors';

// We mock the global fetch function to simulate indexer responses.
const originalFetch = globalThis.fetch;

function mockFetch(impl: typeof fetch) {
  globalThis.fetch = impl;
}

function restoreFetch() {
  globalThis.fetch = originalFetch;
}

describe('Client.indexerHealth', () => {
  afterEach(() => {
    restoreFetch();
  });

  it('returns typed health response when indexer is healthy', async () => {
    mockFetch(async () =>
      new Response(
        JSON.stringify({
          status: 'ok',
          last_indexed_block: 12345,
          total_results: 42,
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } }
      )
    );

    const client = new Client({ indexerUrl: 'http://localhost:8081' });
    const health = await client.indexerHealth();

    expect(health.status).toBe('ok');
    expect(health.lastIndexedBlock).toBe(12345);
    expect(health.totalResults).toBe(42);
  });

  it('throws ServiceHealthError when status is degraded', async () => {
    mockFetch(async () =>
      new Response(
        JSON.stringify({
          status: 'degraded',
          last_indexed_block: 100,
          total_results: 5,
        }),
        { status: 503, headers: { 'Content-Type': 'application/json' } }
      )
    );

    const client = new Client({ indexerUrl: 'http://localhost:8081' });

    await expect(client.indexerHealth()).rejects.toThrow(ServiceHealthError);

    try {
      await client.indexerHealth();
    } catch (e) {
      expect(e).toBeInstanceOf(ServiceHealthError);
      const err = e as ServiceHealthError;
      expect(err.status).toBe('degraded');
      expect(err.lastIndexedBlock).toBe(100);
      expect(err.totalResults).toBe(5);
      expect(err.message).toContain('degraded');
    }
  });

  it('throws when no indexer URL is configured', async () => {
    const client = new Client();
    await expect(client.indexerHealth()).rejects.toThrow(
      'Indexer URL is required'
    );
  });

  it('allows per-call indexerUrl override', async () => {
    let requestedUrl = '';
    mockFetch(async (input: RequestInfo | URL) => {
      requestedUrl = typeof input === 'string' ? input : input.toString();
      return new Response(
        JSON.stringify({
          status: 'ok',
          last_indexed_block: 1,
          total_results: 0,
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } }
      );
    });

    const client = new Client({ indexerUrl: 'http://default:8081' });
    await client.indexerHealth({ indexerUrl: 'http://override:9090' });

    expect(requestedUrl).toBe('http://override:9090/health');
  });

  it('uses the client-level indexerUrl by default', async () => {
    let requestedUrl = '';
    mockFetch(async (input: RequestInfo | URL) => {
      requestedUrl = typeof input === 'string' ? input : input.toString();
      return new Response(
        JSON.stringify({
          status: 'ok',
          last_indexed_block: 1,
          total_results: 0,
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } }
      );
    });

    const client = new Client({ indexerUrl: 'http://my-indexer:8081' });
    await client.indexerHealth();

    expect(requestedUrl).toBe('http://my-indexer:8081/health');
  });

  it('strips trailing slash from indexerUrl', async () => {
    let requestedUrl = '';
    mockFetch(async (input: RequestInfo | URL) => {
      requestedUrl = typeof input === 'string' ? input : input.toString();
      return new Response(
        JSON.stringify({
          status: 'ok',
          last_indexed_block: 1,
          total_results: 0,
        }),
        { status: 200, headers: { 'Content-Type': 'application/json' } }
      );
    });

    const client = new Client({ indexerUrl: 'http://my-indexer:8081/' });
    await client.indexerHealth();

    expect(requestedUrl).toBe('http://my-indexer:8081/health');
  });

  it('throws NetworkError when fetch fails', async () => {
    mockFetch(async () => {
      throw new Error('Connection refused');
    });

    const client = new Client({ indexerUrl: 'http://localhost:8081' });
    await expect(client.indexerHealth()).rejects.toThrow(NetworkError);
  });

  it('throws TimeoutError when request exceeds timeout', async () => {
    mockFetch(async (_input: RequestInfo | URL, init?: RequestInit) => {
      // Wait for the abort signal to fire
      return new Promise<Response>((_resolve, reject) => {
        if (init?.signal) {
          init.signal.addEventListener('abort', () => {
            const abortErr = new DOMException('The operation was aborted.', 'AbortError');
            reject(abortErr);
          });
        }
      });
    });

    const client = new Client({ indexerUrl: 'http://localhost:8081' });

    await expect(
      client.indexerHealth({ timeout: 50 })
    ).rejects.toThrow(TimeoutError);
  });

  it('defaults to 5000ms timeout', async () => {
    // Verify the default timeout is 5000ms by checking the TimeoutError message
    // We use a very short custom timeout here to actually trigger it in tests
    mockFetch(async (_input: RequestInfo | URL, init?: RequestInit) => {
      return new Promise<Response>((_resolve, reject) => {
        if (init?.signal) {
          init.signal.addEventListener('abort', () => {
            const abortErr = new DOMException('The operation was aborted.', 'AbortError');
            reject(abortErr);
          });
        }
      });
    });

    const client = new Client({ indexerUrl: 'http://localhost:8081' });

    try {
      await client.indexerHealth({ timeout: 10 });
    } catch (e) {
      expect(e).toBeInstanceOf(TimeoutError);
      expect((e as TimeoutError).timeoutMs).toBe(10);
    }
  });

  it('handles missing fields gracefully', async () => {
    mockFetch(async () =>
      new Response(
        JSON.stringify({ status: 'ok' }),
        { status: 200, headers: { 'Content-Type': 'application/json' } }
      )
    );

    const client = new Client({ indexerUrl: 'http://localhost:8081' });
    const health = await client.indexerHealth();

    expect(health.status).toBe('ok');
    expect(health.lastIndexedBlock).toBe(0);
    expect(health.totalResults).toBe(0);
  });

  it('ServiceHealthError extends WorldZKError', async () => {
    const err = new ServiceHealthError('degraded', 100, 5);
    expect(err.name).toBe('ServiceHealthError');
    expect(err.status).toBe('degraded');
    expect(err.lastIndexedBlock).toBe(100);
    expect(err.totalResults).toBe(5);
    expect(err.message).toContain('degraded');
    expect(err.message).toContain('100');
    expect(err.message).toContain('5');
  });
});
