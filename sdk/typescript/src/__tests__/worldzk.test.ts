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
    mockFetch(async () =>
      jsonResponse({
        status: 'healthy',
        services: [{ name: 'verifier', healthy: true, response_ms: 1 }],
        healthy_count: 1,
        total_count: 1,
      }),
    );

    const zk = new WorldZK();
    const h = await zk.health();

    expect(h.status).toBe('healthy');
    expect(h.healthy_count).toBe(1);
    expect(h.services?.[0].name).toBe('verifier');
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

  // Shapes below are what the proof-generator returns through the gateway
  // (checked against the running services).
  it('sends the model as a JSON object with its name and returns model_id', async () => {
    let capturedBody: Record<string, unknown> = {};
    mockFetch(async (_input: RequestInfo | URL, init?: RequestInit) => {
      capturedBody = JSON.parse(init?.body as string);
      return jsonResponse({
        model_id: 'mdl-001',
        model_hash: '0x9efa',
        circuit_hash: '0x43e0',
        format: 'xgboost',
        active: true,
      });
    });

    const zk = new WorldZK();
    const resp = await zk.uploadModel('{"trees":[]}', 'test-model', 'xgboost');

    expect(resp.model_id).toBe('mdl-001');
    expect(resp.model_hash).toBe('0x9efa');
    expect(resp.format).toBe('xgboost');
    // The service takes a JSON value: a string would be stored verbatim.
    expect(capturedBody['model_json']).toEqual({ trees: [] });
    expect(capturedBody['name']).toBe('test-model');
    expect(capturedBody['format']).toBe('xgboost');
  });

  it('accepts an already parsed model and omits format when not provided', async () => {
    let capturedBody: Record<string, unknown> = {};
    mockFetch(async (_input: RequestInfo | URL, init?: RequestInit) => {
      capturedBody = JSON.parse(init?.body as string);
      return jsonResponse({
        model_id: 'mdl-002',
        model_hash: '0x00',
        circuit_hash: '0x00',
        format: 'xgboost',
        active: true,
      });
    });

    const zk = new WorldZK();
    await zk.uploadModel({ learner: {} }, 'parsed');

    expect(capturedBody['model_json']).toEqual({ learner: {} });
    expect(capturedBody['name']).toBe('parsed');
    expect(capturedBody).not.toHaveProperty('format');
  });

  it('rejects model text that is not JSON before sending anything', async () => {
    let called = false;
    mockFetch(async () => {
      called = true;
      return jsonResponse({});
    });

    const zk = new WorldZK();
    await expect(zk.uploadModel('not json', 'broken')).rejects.toThrow(SyntaxError);
    expect(called).toBe(false);
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
          {
            id: 'mdl-001', name: 'A', format: 'xgboost', model_hash: '0x01', circuit_hash: '0x02',
            active: true, created_at: '2026-11-22T20:35:57Z', file_size_bytes: 1866,
          },
          {
            id: 'mdl-002', name: 'B', format: 'lightgbm', model_hash: '0x03', circuit_hash: '0x04',
            active: true, created_at: '2026-11-22T20:36:00Z', file_size_bytes: 900,
          },
        ],
      }),
    );

    const zk = new WorldZK();
    const resp = await zk.listModels();

    expect(resp.models).toHaveLength(2);
    expect(resp.models[0].id).toBe('mdl-001');
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
        model_hash: '0x9efa',
        circuit_hash: '0x43e0',
        output: 'approved',
        mock: true,
        prove_time_ms: 0,
        proof_bundle: { proof_hex: '0xbc95', public_inputs_hex: '0xcb04' },
      });
    });

    const zk = new WorldZK('http://localhost:8080');
    const resp = await zk.prove('mdl-001', [1.0, 2.5, 3.7]);

    expect(capturedUrl).toBe('http://localhost:8080/v1/prove');
    expect(capturedBody['model_id']).toBe('mdl-001');
    expect(capturedBody['features']).toEqual([1.0, 2.5, 3.7]);
    expect(resp.proof_id).toBe('prf-123');
    expect(resp.output).toBe('approved');
    expect(resp.mock).toBe(true);
    // Only set when the generator stored the proof in the registry.
    expect(resp.registry_proof_id).toBeUndefined();
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
      return jsonResponse({ verified: true, receipt_id: 'rcp-1' });
    });

    const zk = new WorldZK('http://localhost:8080');
    const resp = await zk.verify('prf-123');

    expect(capturedUrl).toBe('http://localhost:8080/v1/proofs/prf-123/verify');
    expect(resp.verified).toBe(true);
    expect(resp.receipt_id).toBe('rcp-1');
  });

  it('returns verified=false for invalid proof', async () => {
    mockFetch(async () =>
      jsonResponse({ verified: false, receipt_id: 'rcp-2', error: 'sumcheck failed' }),
    );

    const zk = new WorldZK();
    const resp = await zk.verify('prf-999');

    expect(resp.verified).toBe(false);
    expect(resp.error).toBe('sumcheck failed');
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
        receipt_id: 'rcp-1',
        proof_id: 'prf-123',
        circuit_hash: '0x8e7c',
        model_hash: '',
        verified: true,
        verified_at: '2026-11-22T20:36:13Z',
        verifier_version: '0.1.0',
        verifier_host: 'unknown',
        signature: '7370dc',
      });
    });

    const zk = new WorldZK('http://localhost:8080');
    const resp = await zk.getReceipt('prf-123');

    expect(capturedUrl).toBe('http://localhost:8080/v1/proofs/prf-123/receipt');
    expect(resp.proof_id).toBe('prf-123');
    expect(resp.receipt_id).toBe('rcp-1');
    expect(resp.verified).toBe(true);
    expect(resp.signature).toBe('7370dc');
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
          { id: 'prf-1', model_hash: '0x01', circuit_hash: '0x02', created_at: '2026-11-22T20:35:57Z', verified: true },
          { id: 'prf-2', model_hash: '0x03', circuit_hash: '0x04', created_at: '2026-11-22T20:36:00Z' },
        ],
        count: 2,
      });
    });

    const zk = new WorldZK('http://localhost:8080');
    const resp = await zk.listProofs();

    expect(capturedUrl).toBe('http://localhost:8080/v1/proofs');
    expect(resp.proofs).toHaveLength(2);
    expect(resp.proofs[0].id).toBe('prf-1');
    expect(resp.count).toBe(2);
  });

  it('applies model and limit query params', async () => {
    let capturedUrl = '';
    mockFetch(async (input: RequestInfo | URL) => {
      capturedUrl = typeof input === 'string' ? input : (input as Request).url;
      return jsonResponse({ proofs: [], count: 0 });
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
        active_proofs: 1200,
        verified_count: 567,
        failed_count: 3,
        total_storage_bytes: 86400,
      });
    });

    const zk = new WorldZK('http://localhost:8080');
    const resp = await zk.stats();

    expect(capturedUrl).toBe('http://localhost:8080/v1/stats');
    expect(resp.total_proofs).toBe(1234);
    expect(resp.verified_count).toBe(567);
    expect(resp.total_storage_bytes).toBe(86400);
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

  // The generator's `proof_id` is not a registry id: verify() and getReceipt()
  // take `registry_proof_id`, which prove() carries when the proof was stored.
  it('upload -> prove -> verify -> getReceipt', async () => {
    let callCount = 0;
    const verifiedUrls: string[] = [];
    mockFetch(async (input: RequestInfo | URL) => {
      callCount++;
      const url = typeof input === 'string' ? input : (input as Request).url;

      if (url.endsWith('/v1/models')) {
        return jsonResponse({
          model_id: 'mdl-new', model_hash: '0x01', circuit_hash: '0x02', format: 'xgboost', active: true,
        });
      }
      if (url.endsWith('/v1/prove')) {
        return jsonResponse({
          proof_id: 'gen-1',
          registry_proof_id: 'reg-1',
          model_hash: '0x01',
          circuit_hash: '0x02',
          output: 'approved',
          mock: false,
          prove_time_ms: 12,
          proof_bundle: { proof_hex: '0x00', public_inputs_hex: '0x00' },
        });
      }
      if (url.includes('/verify')) {
        verifiedUrls.push(url);
        return jsonResponse({ verified: true, receipt_id: 'rcp-1' });
      }
      if (url.includes('/receipt')) {
        verifiedUrls.push(url);
        return jsonResponse({
          receipt_id: 'rcp-1', proof_id: 'reg-1', circuit_hash: '0x02', model_hash: '0x01',
          verified: true, verified_at: '2026-11-22T20:36:13Z', signature: 'aaa',
        });
      }

      return jsonResponse({ error: 'unexpected' }, 404);
    });

    const zk = new WorldZK('http://localhost:8080', 'key');

    const model = await zk.uploadModel('{"trees":[]}', 'workflow-test');
    expect(model.model_id).toBe('mdl-new');

    const proof = await zk.prove(model.model_id, [1.0, 2.0]);
    expect(proof.proof_id).toBe('gen-1');
    expect(proof.registry_proof_id).toBe('reg-1');

    const verification = await zk.verify(proof.registry_proof_id!);
    expect(verification.verified).toBe(true);

    const receipt = await zk.getReceipt(proof.registry_proof_id!);
    expect(receipt.receipt_id).toBe(verification.receipt_id);
    expect(receipt.signature).toBe('aaa');

    expect(verifiedUrls).toEqual([
      'http://localhost:8080/v1/proofs/reg-1/verify',
      'http://localhost:8080/v1/proofs/reg-1/receipt',
    ]);
    expect(callCount).toBe(4);
  });
});
