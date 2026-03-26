import { describe, it, expect } from 'vitest';
import { teeMLVerifierAbi } from '../tee-verifier-abi';
import { TEEVerifier } from '../tee-verifier';

describe('teeMLVerifierAbi', () => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const findFunction = (name: string): any =>
    teeMLVerifierAbi.find(
      (e) => e.type === 'function' && e.name === name,
    );

  it('should have admin() view function', () => {
    const fn = findFunction('admin');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.outputs).toHaveLength(1);
    expect(fn.outputs[0].type).toBe('address');
  });

  it('should have changeAdmin(address) function', () => {
    const fn = findFunction('changeAdmin');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
    expect(fn.inputs).toHaveLength(1);
    expect(fn.inputs[0].type).toBe('address');
  });

  it('should have timelock() view function', () => {
    const fn = findFunction('timelock');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.outputs).toHaveLength(1);
    expect(fn.outputs[0].type).toBe('address');
  });

  it('should have setTimelock(address) function', () => {
    const fn = findFunction('setTimelock');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
    expect(fn.inputs).toHaveLength(1);
    expect(fn.inputs[0].type).toBe('address');
  });

  it('should have implementation() view function', () => {
    const fn = findFunction('implementation');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.outputs).toHaveLength(1);
    expect(fn.outputs[0].type).toBe('address');
  });

  it('should not have owner() function (replaced by admin)', () => {
    expect(findFunction('owner')).toBeUndefined();
  });

  it('should not have pendingOwner() function', () => {
    expect(findFunction('pendingOwner')).toBeUndefined();
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
      'admin',
      'changeAdmin',
      'timelock',
      'setTimelock',
      'implementation',
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
      'admin',
      'changeAdmin',
      'timelock',
      'setTimelock',
      'implementation',
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

  it('should not have owner() method (replaced by admin)', () => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect((TEEVerifier.prototype as any).owner).toBeUndefined();
  });
});
