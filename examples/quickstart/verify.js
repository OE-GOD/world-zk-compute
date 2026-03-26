#!/usr/bin/env node
/**
 * Quickstart: Verify a ZKML proof bundle in JavaScript.
 *
 * Usage:
 *   node verify.js <proof_bundle.json>
 *   node verify.js  # uses test fixture if available
 */

const fs = require('fs');
const path = require('path');

async function main() {
  // Find proof bundle
  let bundlePath = process.argv[2];
  if (!bundlePath) {
    const fixture = path.join(
      __dirname,
      '../../contracts/test/fixtures/phase1a_dag_fixture.json'
    );
    if (fs.existsSync(fixture)) {
      bundlePath = fixture;
      console.log(`Using test fixture: ${bundlePath}`);
    } else {
      console.log('Usage: node verify.js <proof_bundle.json>');
      process.exit(1);
    }
  }

  // Load bundle
  const bundle = JSON.parse(fs.readFileSync(bundlePath, 'utf8'));
  console.log(`Proof size: ${(bundle.proof_hex || '').length} hex chars`);
  console.log(`Gens size: ${(bundle.gens_hex || '').length} hex chars`);

  // Verify via REST API
  const url = process.env.VERIFIER_URL || 'http://localhost:3000';
  try {
    const resp = await fetch(`${url}/verify`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(bundle),
    });

    if (!resp.ok) {
      const err = await resp.json();
      console.error(`Verification error: ${err.error}`);
      process.exit(1);
    }

    const result = await resp.json();
    console.log(`Verified: ${result.verified}`);
    if (result.circuit_hash) {
      console.log(`Circuit hash: ${result.circuit_hash}`);
    }
  } catch (err) {
    console.error(`REST API error: ${err.message}`);
    console.log(
      'Start the verifier: docker compose -f docker-compose.bank-demo.yml up -d'
    );
    process.exit(1);
  }
}

main();
