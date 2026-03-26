#!/usr/bin/env node
/**
 * Quickstart: Verify a ZKML proof bundle in JavaScript.
 *
 * This script demonstrates two verification methods:
 * 1. WASM library (npm install zkml-verifier) -- browser and Node.js
 * 2. REST API (docker compose up) -- any HTTP client
 *
 * Usage:
 *   node verify.js <proof_bundle.json>
 *   node verify.js                         # uses test fixture if available
 *
 * Environment variables:
 *   VERIFIER_URL  -- REST API base URL (default: http://localhost:3000)
 */

const fs = require('fs');
const path = require('path');

/**
 * Find a proof bundle from CLI args or test fixtures.
 */
function findProofBundle() {
  if (process.argv[2]) {
    const p = process.argv[2];
    if (!fs.existsSync(p)) {
      console.error(`Error: file not found: ${p}`);
      process.exit(1);
    }
    return p;
  }

  // Try test fixtures in order of preference
  const scriptDir = __dirname;
  const candidates = [
    path.join(scriptDir, 'sample_proof.json'),
    path.join(scriptDir, '../../contracts/test/fixtures/phase1a_dag_fixture.json'),
    path.join(scriptDir, '../../web/sample_proof.json'),
  ];

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      console.log(`Using fixture: ${candidate}`);
      return candidate;
    }
  }

  console.log('Usage: node verify.js <proof_bundle.json>');
  console.log();
  console.log('No proof bundle provided and no test fixture found.');
  process.exit(1);
}

/**
 * Verify using the WASM library (requires: npm install zkml-verifier).
 */
async function verifyWasm(bundleJson) {
  try {
    const { default: init, verify_proof_json, version } = await import('zkml-verifier');
    await init();

    console.log(`zkml-verifier version: ${version()}`);
    const result = verify_proof_json(bundleJson);

    console.log(`Verified (WASM): ${result.verified}`);
    if (result.circuit_hash) {
      console.log(`  Circuit hash: ${result.circuit_hash}`);
    }
    if (result.error) {
      console.log(`  Error: ${result.error}`);
    }

    result.free(); // Free WASM memory
    return result.verified;
  } catch (err) {
    if (err.code === 'ERR_MODULE_NOT_FOUND' || err.code === 'MODULE_NOT_FOUND') {
      console.log('WASM library not available (npm install zkml-verifier)');
    } else {
      console.log(`WASM verification failed: ${err.message}`);
    }
    return null;
  }
}

/**
 * Verify using the REST API service.
 *
 * Requires: docker compose -f docker-compose.verifier.yml up -d
 */
async function verifyRestApi(bundleJson) {
  const url = process.env.VERIFIER_URL || 'http://localhost:3000';

  try {
    const resp = await fetch(`${url}/verify`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: bundleJson,
    });

    if (!resp.ok) {
      const err = await resp.json().catch(() => ({ error: resp.statusText }));
      console.error(`REST API error (${resp.status}): ${err.error}`);
      return false;
    }

    const result = await resp.json();
    console.log(`Verified (REST API): ${result.verified}`);
    if (result.circuit_hash) {
      console.log(`  Circuit hash: ${result.circuit_hash}`);
    }
    return result.verified;
  } catch (err) {
    console.log(`REST API not available: ${err.message}`);
    console.log('  Start with: docker compose -f docker-compose.verifier.yml up -d');
    return null;
  }
}

async function main() {
  const bundlePath = findProofBundle();
  const bundleJson = fs.readFileSync(bundlePath, 'utf8');
  const bundle = JSON.parse(bundleJson);

  // Show bundle info
  console.log(`Proof bundle: ${bundlePath}`);
  console.log(`  proof_hex:  ${(bundle.proof_hex || '').length} hex chars`);
  console.log(`  gens_hex:   ${(bundle.gens_hex || '').length} hex chars`);
  if (bundle.circuit_hash) {
    console.log(`  circuit:    ${bundle.circuit_hash}`);
  }
  if (bundle.model_hash) {
    console.log(`  model:      ${bundle.model_hash}`);
  }
  console.log();

  // Try WASM first, then REST API
  console.log('--- Method 1: WASM Library ---');
  const wasmResult = await verifyWasm(bundleJson);

  if (wasmResult === null) {
    console.log();
    console.log('--- Method 2: REST API ---');
    await verifyRestApi(bundleJson);
  }
}

main();
