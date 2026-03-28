/**
 * Tests for the WorldZK simplified ML inference client.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { WorldZK } from '../worldzk';

// ---------------------------------------------------------------------------
// Fetch mock helpers
// ---------------------------------------------------------------------------

const originalFetch = globalThis.fetch;

function mockFetch(impl: typeof fetch) {
  globalThis.fetch = impl;
}

function restoreFetch() {
  globalThis.fetch = originalFetch;
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

// ---------------------------------------------------------------------------
// Constructor tests
// ---------------------------------------------------------------------------

describe('WorldZK constructor', () => {
  it('uses defaults when no arguments are provided', () => {
    const zk = new WorldZK();
    // We cannot inspect private fields directly, but we can observe behavior
    // via health() which hits the default URL. Just check it constructs.
    expect(zk).toBeInstanceOf(WorldZK);
  });

  it('accepts a URL string', () => {
    const zk = new WorldZK('http://my-host:9090');
    expect(zk).toBeInstanceOf(WorldZK);
  });

  it('strips trailing slash from URL', async () => {
    let capturedUrl = '';
    mockFetch(async (input: RequestInfo | URL) => {
      capturedUrl = typeof input === 'string' ? input : (input as Request).url;
      return jsonResponse({ status: 'ok' });
    });

    const zk = new WorldZK('http://my-host:9090/');
    await zk.health();
    expect(capturedUrl).toBe('http://my-host:9090/health');
    restoreFetch();
  });

  it('accepts an options object', () => {
    const zk = new WorldZK({ url: 'http://localhost:3000', apiKey: 'secret', timeout: 5000 });
    expect(zk).toBeInstanceOf(WorldZK);
  });

  it('accepts URL string plus apiKey', () => {
    const zk = new WorldZK('http://localhost:3000', 'my-key');
    expect(zk).toBeInstanceOf(WorldZK);
  });
});

// ---------------------------------------------------------------------------
// Request header tests
// ---------------------------------------------------------------------------

describe('WorldZK request headers', () => {
  afterEach(() => {
    restoreFetch();
  });

  it('sends Authorization header when apiKey is provided', async () => {
    let capturedHeaders: Record<string, string> = {};
    mockFetch(async (_input: RequestInfo | URL, init?: RequestInit) => {
      const h = init?.headers as Record<string, string> | undefined;
      capturedHeaders = h ?? {};
      return jsonResponse({ status: 'ok' });
    });

    const zk = new WorldZK('http://localhost:8080', 'test-token');
    await zk.health();

    expect(capturedHeaders['Authorization']).toBe('Bearer test-token');
    expect(capturedHeaders['Content-Type']).toBe('application/json');
  });

  it('does not send Authorization header when no apiKey', async () => {
    let capturedHeaders: Record<string, string> = {};
    mockFetch(async (_input: RequestInfo | URL, init?: RequestInit) => {
      const h = init?.headers as Record<string, string> | undefined;
      capturedHeaders = h ?? {};
      return jsonResponse({ status: 'ok' });
    });

    const zk = new WorldZK('http://localhost:8080');
    await zk.health();

    expect(capturedHeaders['Authorization']).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// health()
// ---------------------------------------------------------------------------

describe('WorldZK.health()', () => {
  afterEach(() => {
    restoreFetch();
  });

  it('returns health status', async () => {
    mockFetch(async () => jsonResponse({ status: 'ok', circuits_loaded: 3 }));

    const zk = new WorldZK();
    const h = await zk.health();

    expect(h.status).toBe('ok');
    expect(h['circuits_loaded']).toBe(3);
  });

  it('throws on non-2xx response', async () => {
    mockFetch(async () => jsonResponse({ error: 'down' }, 503));

    const zk = new WorldZK();
    await expect(zk.health()).rejects.toThrow('WorldZK API error 503');
  });
});

// ---------------------------------------------------------------------------
// uploadModel()
// ---------------------------------------------------------------------------

describe('WorldZK.uploadModel()', () => {
  afterEach(() => {
    restoreFetch();
  });

  it('sends model JSON and returns model_id', async () => {
    let capturedBody: Record<string, unknown> = {};
    mockFetch(async (_input: RequestInfo | URL, init?: RequestInit) => {
      capturedBody = JSON.parse(init?.body as string);
      return jsonResponse({
        model_id: 'mdl-001',
        name: 'test-model',
        format: 'xgboost',
        created_at: '2026-03-27T00:00:00Z',
      });
    });

    const zk = new WorldZK();
    const resp = await zk.uploadModel('{"trees":[]}', 'test-model', 'xgboost');

    expect(resp.model_id).toBe('mdl-001');
    expect(resp.name).toBe('test-model');
    expect(resp.format).toBe('xgboost');
    expect(capturedBody['model_json']).toBe('{"trees":[]}');
    expect(capturedBody['name']).toBe('test-model');
    expect(capturedBody['format']).toBe('xgboost');
  });

  it('omits optional fields when not provided', async () => {
    let capturedBody: Record<string, unknown> = {};
    mockFetch(async (_input: RequestInfo | URL, init?: RequestInit) => {
      capturedBody = JSON.parse(init?.body as string);
      return jsonResponse({ model_id: 'mdl-002' });
    });

    const zk = new WorldZK();
    await zk.uploadModel('{}');

    expect(capturedBody['model_json']).toBe('{}');
    expect(capturedBody).not.toHaveProperty('name');
    expect(capturedBody).not.toHaveProperty('format');
  });
});

// ---------------------------------------------------------------------------
// listModels()
// ---------------------------------------------------------------------------

describe('WorldZK.listModels()', () => {
  afterEach(() => {
    restoreFetch();
  });

  it('returns list of models', async () => {
    mockFetch(async () =>
      jsonResponse({
        models: [
          { model_id: 'mdl-001', name: 'A', format: 'xgboost' },
          { model_id: 'mdl-002', name: 'B', format: 'lightgbm' },
        ],
      }),
    );

    const zk = new WorldZK();
    const resp = await zk.listModels();

    expect(resp.models).toHaveLength(2);
    expect(resp.models[0].model_id).toBe('mdl-001');
    expect(resp.models[1].format).toBe('lightgbm');
  });
});

// ---------------------------------------------------------------------------
// prove()
// ---------------------------------------------------------------------------

describe('WorldZK.prove()', () => {
  afterEach(() => {
    restoreFetch();
  });

  it('sends model_id and features, returns proof', async () => {
    let capturedUrl = '';
    let capturedBody: Record<string, unknown> = {};
    mockFetch(async (input: RequestInfo | URL, init?: RequestInit) => {
      capturedUrl = typeof input === 'string' ? input : (input as Request).url;
      capturedBody = JSON.parse(init?.body as string);
      return jsonResponse({
        proof_id: 'prf-123',
        model_id: 'mdl-001',
        status: 'completed',
        result: 42,
      });
    });

    const zk = new WorldZK('http://localhost:8080');
    const resp = await zk.prove('mdl-001', [1.0, 2.5, 3.7]);

    expect(capturedUrl).toBe('http://localhost:8080/v1/prove');
    expect(capturedBody['model_id']).toBe('mdl-001');
    expect(capturedBody['features']).toEqual([1.0, 2.5, 3.7]);
    expect(resp.proof_id).toBe('prf-123');
    expect(resp.result).toBe(42);
    expect(resp.status).toBe('completed');
  });
});

// ---------------------------------------------------------------------------
// verify()
// ---------------------------------------------------------------------------

describe('WorldZK.verify()', () => {
  afterEach(() => {
    restoreFetch();
  });

  it('verifies a proof by ID', async () => {
    let capturedUrl = '';
    mockFetch(async (input: RequestInfo | URL) => {
      capturedUrl = typeof input === 'string' ? input : (input as Request).url;
      return jsonResponse({
        proof_id: 'prf-123',
        verified: true,
        circuit_hash: '0xabcd',
        verification_time_ms: 42,
      });
    });

    const zk = new WorldZK('http://localhost:8080');
    const resp = await zk.verify('prf-123');

    expect(capturedUrl).toBe('http://localhost:8080/v1/proofs/prf-123/verify');
    expect(resp.verified).toBe(true);
    expect(resp.proof_id).toBe('prf-123');
    expect(resp.circuit_hash).toBe('0xabcd');
  });

  it('returns verified=false for invalid proof', async () => {
    mockFetch(async () =>
      jsonResponse({ proof_id: 'prf-999', verified: false, error: 'invalid seal' }),
    );

    const zk = new WorldZK();
    const resp = await zk.verify('prf-999');

    expect(resp.verified).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// getReceipt()
// ---------------------------------------------------------------------------

describe('WorldZK.getReceipt()', () => {
  afterEach(() => {
    restoreFetch();
  });

  it('retrieves receipt for a proof', async () => {
    let capturedUrl = '';
    mockFetch(async (input: RequestInfo | URL) => {
      capturedUrl = typeof input === 'string' ? input : (input as Request).url;
      return jsonResponse({
        proof_id: 'prf-123',
        seal: 'base64seal==',
        journal: 'base64journal==',
        image_id: '0xdeadbeef',
      });
    });

    const zk = new WorldZK('http://localhost:8080');
    const resp = await zk.getReceipt('prf-123');

    expect(capturedUrl).toBe('http://localhost:8080/v1/proofs/prf-123/receipt');
    expect(resp.proof_id).toBe('prf-123');
    expect(resp.seal).toBe('base64seal==');
    expect(resp.journal).toBe('base64journal==');
    expect(resp.image_id).toBe('0xdeadbeef');
  });
});

// ---------------------------------------------------------------------------
// listProofs()
// ---------------------------------------------------------------------------

describe('WorldZK.listProofs()', () => {
  afterEach(() => {
    restoreFetch();
  });

  it('lists proofs without filters', async () => {
    let capturedUrl = '';
    mockFetch(async (input: RequestInfo | URL) => {
      capturedUrl = typeof input === 'string' ? input : (input as Request).url;
      return jsonResponse({
        proofs: [
          { proof_id: 'prf-1', model_id: 'mdl-1', status: 'completed' },
          { proof_id: 'prf-2', model_id: 'mdl-2', status: 'pending' },
        ],
      });
    });

    const zk = new WorldZK('http://localhost:8080');
    const resp = await zk.listProofs();

    expect(capturedUrl).toBe('http://localhost:8080/v1/proofs');
    expect(resp.proofs).toHaveLength(2);
  });

  it('applies model and limit query params', async () => {
    let capturedUrl = '';
    mockFetch(async (input: RequestInfo | URL) => {
      capturedUrl = typeof input === 'string' ? input : (input as Request).url;
      return jsonResponse({ proofs: [] });
    });

    const zk = new WorldZK('http://localhost:8080');
    await zk.listProofs({ model: 'mdl-001', limit: 5 });

    expect(capturedUrl).toContain('model=mdl-001');
    expect(capturedUrl).toContain('limit=5');
  });
});

// ---------------------------------------------------------------------------
// stats()
// ---------------------------------------------------------------------------

describe('WorldZK.stats()', () => {
  afterEach(() => {
    restoreFetch();
  });

  it('returns service statistics', async () => {
    let capturedUrl = '';
    mockFetch(async (input: RequestInfo | URL) => {
      capturedUrl = typeof input === 'string' ? input : (input as Request).url;
      return jsonResponse({
        total_proofs: 1234,
        total_models: 10,
        total_verifications: 567,
        uptime_seconds: 86400,
      });
    });

    const zk = new WorldZK('http://localhost:8080');
    const resp = await zk.stats();

    expect(capturedUrl).toBe('http://localhost:8080/v1/stats');
    expect(resp.total_proofs).toBe(1234);
    expect(resp.total_models).toBe(10);
    expect(resp.uptime_seconds).toBe(86400);
  });
});

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

describe('WorldZK error handling', () => {
  afterEach(() => {
    restoreFetch();
  });

  it('throws with status code on 4xx errors', async () => {
    mockFetch(async () => new Response('{"error":"not found"}', {
      status: 404,
      headers: { 'Content-Type': 'application/json' },
    }));

    const zk = new WorldZK();
    await expect(zk.prove('no-such-model', [1])).rejects.toThrow('WorldZK API error 404');
  });

  it('throws with status code on 5xx errors', async () => {
    mockFetch(async () => new Response('internal server error', {
      status: 500,
      headers: { 'Content-Type': 'text/plain' },
    }));

    const zk = new WorldZK();
    await expect(zk.stats()).rejects.toThrow('WorldZK API error 500');
  });

  it('throws on network failure', async () => {
    mockFetch(async () => {
      throw new TypeError('fetch failed');
    });

    const zk = new WorldZK();
    await expect(zk.health()).rejects.toThrow('fetch failed');
  });

  it('throws on timeout', async () => {
    mockFetch(async (_input: RequestInfo | URL, init?: RequestInit) => {
      // Simulate abort by listening to the signal
      return new Promise<Response>((_resolve, reject) => {
        if (init?.signal) {
          init.signal.addEventListener('abort', () => {
            const err = new DOMException('The operation was aborted.', 'AbortError');
            reject(err);
          });
        }
      });
    });

    const zk = new WorldZK({ url: 'http://localhost:8080', timeout: 50 });
    await expect(zk.health()).rejects.toThrow('WorldZK request timed out after 50ms');
  });
});

// ---------------------------------------------------------------------------
// Integration-style: full workflow
// ---------------------------------------------------------------------------

describe('WorldZK full workflow', () => {
  afterEach(() => {
    restoreFetch();
  });

  it('upload -> prove -> verify -> getReceipt', async () => {
    let callCount = 0;
    mockFetch(async (input: RequestInfo | URL) => {
      callCount++;
      const url = typeof input === 'string' ? input : (input as Request).url;

      if (url.endsWith('/v1/models')) {
        return jsonResponse({ model_id: 'mdl-new', name: 'workflow-test' });
      }
      if (url.endsWith('/v1/prove')) {
        return jsonResponse({ proof_id: 'prf-new', model_id: 'mdl-new', status: 'completed', result: 7 });
      }
      if (url.includes('/verify')) {
        return jsonResponse({ proof_id: 'prf-new', verified: true });
      }
      if (url.includes('/receipt')) {
        return jsonResponse({ proof_id: 'prf-new', seal: 'aaa', journal: 'bbb' });
      }

      return jsonResponse({ error: 'unexpected' }, 404);
    });

    const zk = new WorldZK('http://localhost:8080', 'key');

    const model = await zk.uploadModel('{"trees":[]}', 'workflow-test');
    expect(model.model_id).toBe('mdl-new');

    const proof = await zk.prove(model.model_id, [1.0, 2.0]);
    expect(proof.proof_id).toBe('prf-new');
    expect(proof.result).toBe(7);

    const verification = await zk.verify(proof.proof_id);
    expect(verification.verified).toBe(true);

    const receipt = await zk.getReceipt(proof.proof_id);
    expect(receipt.seal).toBe('aaa');

    expect(callCount).toBe(4);
  });
});
