/**
 * Tests for ApiError.fromResponse with error bodies that lack the
 * structured `code` / `message` fields.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { Client } from '../client';
import {
  ApiError,
  AuthenticationError,
  NetworkError,
  NotFoundError,
  RateLimitError,
  ValidationError,
} from '../errors';

describe('ApiError.fromResponse', () => {
  it('keeps the status and message of a body with only an `error` field', () => {
    const err = ApiError.fromResponse({ error: 'unauthorized' }, 401);
    expect(err.message).toBe('unauthorized');
    expect(err.httpStatus).toBe(401);
  });

  it('gives an empty body an HTTP message', () => {
    const err = ApiError.fromResponse({}, 502);
    expect(err.message).toBe('HTTP 502');
    expect(err.httpStatus).toBe(502);
  });

  // One row per status rule for a body without a `code`.
  it.each([
    [400, 'WZK-1000', ValidationError, false],
    [401, 'WZK-1030', AuthenticationError, false],
    [403, 'WZK-1031', AuthenticationError, false],
    [404, 'WZK-1021', NotFoundError, false],
    [409, 'WZK-1000', ValidationError, false],
    [429, 'WZK-1040', RateLimitError, true],
    [500, 'WZK-2000', ApiError, false],
    [501, 'WZK-2000', ApiError, false],
    [502, 'WZK-5000', NetworkError, true],
    [503, 'WZK-2010', ApiError, true],
    [504, 'WZK-5001', NetworkError, true],
  ] as const)(
    'classifies a %i body without a code as %s',
    (status, code, errorClass, retryable) => {
      const err = ApiError.fromResponse({ error: 'failed' }, status);
      expect(err.code).toBe(code);
      expect(err).toBeInstanceOf(errorClass);
      expect(err.constructor).toBe(errorClass);
      expect(err.isRetryable).toBe(retryable);
      expect(err.httpStatus).toBe(status);
    }
  );

  it('accepts a null body', () => {
    const err = ApiError.fromResponse(null, 404);
    expect(err).toBeInstanceOf(NotFoundError);
    expect(err.message).toBe('HTTP 404');
  });

  it('reads the indexer rate limit hint into the retry delay', () => {
    const err = ApiError.fromResponse(
      { error: 'Too many requests. Please retry later.', retry_after_seconds: 7 },
      429
    );
    expect(err).toBeInstanceOf(RateLimitError);
    expect(err.retryDelayMs).toBe(7000);
    expect((err as RateLimitError).retryAfterSeconds).toBe(7);
  });

  it('uses the default rate limit delay without a hint', () => {
    expect(ApiError.fromResponse({}, 429).retryDelayMs).toBe(5000);
  });

  it('ignores a rate limit hint that is not a number', () => {
    const err = ApiError.fromResponse({ retry_after_seconds: 'soon' }, 429);
    expect(err.retry).toBeUndefined();
    expect(err.retryDelayMs).toBe(5000);
  });

  it('prefers a structured retry over the rate limit hint', () => {
    const err = ApiError.fromResponse(
      { retry: { retryable: true, retryAfterMs: 250 }, retry_after_seconds: 7 },
      429
    );
    expect(err.retryDelayMs).toBe(250);
  });

  it('still classifies a structured body by its code', () => {
    const err = ApiError.fromResponse({ code: 'WZK-1030', message: 'bad key' }, 500);
    expect(err).toBeInstanceOf(AuthenticationError);
    expect(err.message).toBe('bad key');
  });
});

describe('Client.request with an unstructured error body', () => {
  const originalFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('reports the HTTP status once instead of a retried network error', async () => {
    let calls = 0;
    globalThis.fetch = async () => {
      calls++;
      return new Response(JSON.stringify({ error: 'unauthorized' }), {
        status: 401,
        headers: { 'Content-Type': 'application/json' },
      });
    };
    const client = new Client({ baseUrl: 'http://localhost:1', maxRetries: 3 });
    const err = await client.request('GET', '/x').catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AuthenticationError);
    expect((err as ApiError).httpStatus).toBe(401);
    expect((err as ApiError).message).toBe('unauthorized');
    expect(calls).toBe(1);
  });

  it('retries a 429 after the delay the indexer asks for', async () => {
    let calls = 0;
    globalThis.fetch = async () => {
      calls++;
      if (calls === 1) {
        return new Response(
          JSON.stringify({ error: 'Too many requests.', retry_after_seconds: 0.01 }),
          { status: 429, headers: { 'Content-Type': 'application/json' } }
        );
      }
      return new Response(JSON.stringify({ ok: true }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      });
    };
    const client = new Client({ baseUrl: 'http://localhost:1', maxRetries: 3 });
    await expect(client.request('GET', '/x')).resolves.toEqual({ ok: true });
    expect(calls).toBe(2);
  });
});
