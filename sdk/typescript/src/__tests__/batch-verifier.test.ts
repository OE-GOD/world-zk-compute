import { describe, it, expect } from 'vitest';
import { BatchVerifier } from '../batch-verifier';

describe('BatchVerifier.loadFixture', () => {
  it('loads phase1a_dag_fixture format (proof_hex/public_inputs_hex)', () => {
    const fixture = {
      proof_hex: 'deadbeef',
      public_inputs_hex: 'cafebabe',
      circuit_hash_raw: '0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef',
      gens_hex: '0xaabbccdd',
    };

    const input = BatchVerifier.loadFixture(fixture);
    expect(input.proof).toBe('0xdeadbeef');
    expect(input.publicInputs).toBe('0xcafebabe');
    expect(input.circuitHash).toBe('0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef');
    expect(input.gensData).toBe('0xaabbccdd');
  });

  it('loads groth16_e2e_fixture format (inner_proof_hex/public_values_abi)', () => {
    const fixture = {
      inner_proof_hex: '0xdeadbeef',
      public_values_abi: '0xcafebabe',
      circuit_hash_raw: '0xabcd',
      gens_hex: '0x1122',
    };

    const input = BatchVerifier.loadFixture(fixture);
    expect(input.proof).toBe('0xdeadbeef');
    expect(input.publicInputs).toBe('0xcafebabe');
    expect(input.circuitHash).toBe('0xabcd');
    expect(input.gensData).toBe('0x1122');
  });

  it('adds 0x prefix when missing', () => {
    const fixture = {
      proof_hex: 'aabb',
      public_inputs_hex: 'ccdd',
      circuit_hash_raw: 'eeff',
      gens_hex: '1122',
    };

    const input = BatchVerifier.loadFixture(fixture);
    expect(input.proof).toBe('0xaabb');
    expect(input.publicInputs).toBe('0xccdd');
    expect(input.circuitHash).toBe('0xeeff');
    expect(input.gensData).toBe('0x1122');
  });

  it('preserves existing 0x prefix', () => {
    const fixture = {
      proof_hex: '0xaabb',
      public_inputs_hex: '0xccdd',
      circuit_hash_raw: '0xeeff',
      gens_hex: '0x1122',
    };

    const input = BatchVerifier.loadFixture(fixture);
    expect(input.proof).toBe('0xaabb');
    expect(input.publicInputs).toBe('0xccdd');
  });

  it('prefers inner_proof_hex over proof_hex', () => {
    const fixture = {
      proof_hex: 'aaaa',
      inner_proof_hex: '0xbbbb',
      public_inputs_hex: 'cccc',
      public_values_abi: '0xdddd',
      circuit_hash_raw: '0x0001',
      gens_hex: '0x0002',
    };

    const input = BatchVerifier.loadFixture(fixture);
    expect(input.proof).toBe('0xbbbb');
    expect(input.publicInputs).toBe('0xdddd');
  });

  it('throws on missing required fields', () => {
    expect(() => BatchVerifier.loadFixture({})).toThrow('Fixture missing required fields');

    expect(() =>
      BatchVerifier.loadFixture({
        proof_hex: '0xaa',
        public_inputs_hex: '0xbb',
        // missing circuit_hash_raw and gens_hex
      }),
    ).toThrow('Fixture missing required fields');

    expect(() =>
      BatchVerifier.loadFixture({
        proof_hex: '0xaa',
        circuit_hash_raw: '0xbb',
        gens_hex: '0xcc',
        // missing public_inputs_hex
      }),
    ).toThrow('Fixture missing required fields');
  });
});

describe('BatchVerifier.estimateTransactionCount', () => {
  it('estimates correctly for 88 layers / 34 groups', () => {
    const est = BatchVerifier.estimateTransactionCount(88, 34);
    expect(est.start).toBe(1);
    expect(est.continue).toBe(11); // ceil(88/8)
    expect(est.finalize).toBe(3); // ceil(34/16)
    expect(est.total).toBe(15); // 1 + 11 + 3
  });

  it('handles exact multiples', () => {
    const est = BatchVerifier.estimateTransactionCount(16, 16);
    expect(est.continue).toBe(2); // 16/8
    expect(est.finalize).toBe(1); // 16/16
    expect(est.total).toBe(4); // 1 + 2 + 1
  });

  it('handles single layer and group', () => {
    const est = BatchVerifier.estimateTransactionCount(1, 1);
    expect(est.continue).toBe(1);
    expect(est.finalize).toBe(1);
    expect(est.total).toBe(3);
  });

  it('handles zero layers', () => {
    const est = BatchVerifier.estimateTransactionCount(0, 0);
    expect(est.continue).toBe(0);
    expect(est.finalize).toBe(0);
    expect(est.total).toBe(1); // just start
  });

  it('rounds up partial batches', () => {
    const est = BatchVerifier.estimateTransactionCount(9, 17);
    expect(est.continue).toBe(2); // ceil(9/8)
    expect(est.finalize).toBe(2); // ceil(17/16)
    expect(est.total).toBe(5);
  });
});
