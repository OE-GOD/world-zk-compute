import { describe, it, expect } from 'vitest';
import { teeMLVerifierAbi } from '../tee-verifier-abi';
import { TEEVerifier } from '../tee-verifier';
import { ApiError } from '../errors';

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

  // A mined transaction that reverted still yields a receipt; only its
  // status says the call failed. The constructor opens no connection, so the
  // viem clients it builds are swapped for mocks before any call.
  const verifierWithReceipt = (status: 'success' | 'reverted') => {
    const verifier = new TEEVerifier({
      rpcUrl: 'http://localhost:1',
      privateKey: '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80',
      contractAddress: '0x1234567890abcdef1234567890abcdef12345678',
    });
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const internals = verifier as any;
    internals.publicClient = {
      waitForTransactionReceipt: async () => ({ status, blockNumber: 7n }),
    };
    internals.walletClient = { writeContract: async () => '0xdead' };
    return verifier;
  };

  it('rejects a write whose transaction was mined but reverted', async () => {
    const err = await verifierWithReceipt('reverted')
      .finalize('0x01')
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).code).toBe('WZK-4002');
    expect((err as ApiError).details).toMatchObject({
      transactionHash: '0xdead',
      functionName: 'finalize',
    });
  });

  it('returns the hash of a write whose transaction succeeded', async () => {
    await expect(verifierWithReceipt('success').finalize('0x01')).resolves.toBe('0xdead');
  });
});
