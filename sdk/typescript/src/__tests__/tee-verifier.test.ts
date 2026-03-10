import { describe, it, expect } from 'vitest';
import { teeMLVerifierAbi } from '../tee-verifier-abi';
import { TEEVerifier } from '../tee-verifier';

describe('teeMLVerifierAbi', () => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const findFunction = (name: string): any =>
    teeMLVerifierAbi.find(
      (e) => e.type === 'function' && e.name === name,
    );

  it('should not have admin() function (replaced by owner)', () => {
    expect(findFunction('admin')).toBeUndefined();
  });

  it('should have owner() view function', () => {
    const fn = findFunction('owner');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.outputs).toHaveLength(1);
    expect(fn.outputs[0].type).toBe('address');
  });

  it('should have pendingOwner() view function', () => {
    const fn = findFunction('pendingOwner');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
  });

  it('should have transferOwnership(address) function', () => {
    const fn = findFunction('transferOwnership');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
    expect(fn.inputs).toHaveLength(1);
    expect(fn.inputs[0].type).toBe('address');
  });

  it('should have acceptOwnership() function', () => {
    const fn = findFunction('acceptOwnership');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
  });

  it('should have pause() function', () => {
    const fn = findFunction('pause');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
  });

  it('should have unpause() function', () => {
    const fn = findFunction('unpause');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
  });

  it('should have paused() view function', () => {
    const fn = findFunction('paused');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.outputs).toHaveLength(1);
    expect(fn.outputs[0].type).toBe('bool');
  });

  it('should have all core functions', () => {
    const required = [
      'registerEnclave',
      'revokeEnclave',
      'submitResult',
      'challenge',
      'finalize',
      'resolveDispute',
      'resolveDisputeByTimeout',
      'getResult',
      'isResultValid',
      'owner',
      'remainderVerifier',
    ];
    for (const name of required) {
      expect(findFunction(name)).toBeDefined();
    }
  });
});

describe('TEEVerifier class', () => {
  it('should have all expected public methods', () => {
    const proto = TEEVerifier.prototype;
    const expectedMethods = [
      'registerEnclave',
      'revokeEnclave',
      'submitResult',
      'challenge',
      'finalize',
      'resolveDispute',
      'resolveDisputeByTimeout',
      'getResult',
      'isResultValid',
      'owner',
      'pendingOwner',
      'transferOwnership',
      'acceptOwnership',
      'pause',
      'unpause',
      'paused',
      'remainderVerifier',
    ];
    for (const method of expectedMethods) {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      expect(typeof (proto as any)[method]).toBe('function');
    }
  });

  it('should not have admin() method', () => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect((TEEVerifier.prototype as any).admin).toBeUndefined();
  });
});
