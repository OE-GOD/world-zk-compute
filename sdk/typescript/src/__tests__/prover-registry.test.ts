import { describe, it, expect } from 'vitest';
import { proverRegistryAbi } from '../abi';
import { ProverRegistryClient } from '../prover-registry';

// ---------------------------------------------------------------------------
// ABI validation tests
// ---------------------------------------------------------------------------

describe('proverRegistryAbi', () => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const findFunction = (name: string): any =>
    proverRegistryAbi.find(
      (e) => e.type === 'function' && e.name === name,
    );

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const findEvent = (name: string): any =>
    proverRegistryAbi.find(
      (e) => e.type === 'event' && e.name === name,
    );

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const findError = (name: string): any =>
    proverRegistryAbi.find(
      (e) => e.type === 'error' && e.name === name,
    );

  it('should have register function', () => {
    const fn = findFunction('register');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
    expect(fn.inputs).toHaveLength(2);
    expect(fn.inputs[0].name).toBe('stake');
    expect(fn.inputs[0].type).toBe('uint256');
    expect(fn.inputs[1].name).toBe('endpoint');
    expect(fn.inputs[1].type).toBe('string');
  });

  it('should have deactivate function', () => {
    const fn = findFunction('deactivate');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
    expect(fn.inputs).toHaveLength(0);
  });

  it('should have reactivate function', () => {
    const fn = findFunction('reactivate');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
  });

  it('should have addStake function', () => {
    const fn = findFunction('addStake');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
    expect(fn.inputs).toHaveLength(1);
    expect(fn.inputs[0].type).toBe('uint256');
  });

  it('should have withdrawStake function', () => {
    const fn = findFunction('withdrawStake');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
    expect(fn.inputs).toHaveLength(1);
  });

  it('should have slash function', () => {
    const fn = findFunction('slash');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
    expect(fn.inputs).toHaveLength(2);
    expect(fn.inputs[0].type).toBe('address');
    expect(fn.inputs[1].type).toBe('string');
  });

  it('should have recordSuccess function', () => {
    const fn = findFunction('recordSuccess');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
    expect(fn.inputs).toHaveLength(2);
  });

  it('should have getProver view function returning tuple', () => {
    const fn = findFunction('getProver');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.outputs).toHaveLength(1);
    expect(fn.outputs[0].type).toBe('tuple');
    const components = fn.outputs[0].components;
    const fieldNames = components.map((c: { name: string }) => c.name);
    expect(fieldNames).toContain('owner');
    expect(fieldNames).toContain('stake');
    expect(fieldNames).toContain('reputation');
    expect(fieldNames).toContain('proofsSubmitted');
    expect(fieldNames).toContain('proofsFailed');
    expect(fieldNames).toContain('totalEarnings');
    expect(fieldNames).toContain('registeredAt');
    expect(fieldNames).toContain('lastActiveAt');
    expect(fieldNames).toContain('active');
    expect(fieldNames).toContain('endpoint');
    expect(components).toHaveLength(10);
  });

  it('should have isActive view function', () => {
    const fn = findFunction('isActive');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.inputs[0].type).toBe('address');
    expect(fn.outputs[0].type).toBe('bool');
  });

  it('should have isProver view function', () => {
    const fn = findFunction('isProver');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.outputs[0].type).toBe('bool');
  });

  it('should have getActiveProvers view function', () => {
    const fn = findFunction('getActiveProvers');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.outputs[0].type).toBe('address[]');
  });

  it('should have activeProverCount view function', () => {
    const fn = findFunction('activeProverCount');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.outputs[0].type).toBe('uint256');
  });

  it('should have getWeight view function', () => {
    const fn = findFunction('getWeight');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.inputs[0].type).toBe('address');
    expect(fn.outputs[0].type).toBe('uint256');
  });

  it('should have selectProver view function', () => {
    const fn = findFunction('selectProver');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.inputs[0].type).toBe('uint256');
    expect(fn.outputs[0].type).toBe('address');
  });

  it('should have getTopProvers view function', () => {
    const fn = findFunction('getTopProvers');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.inputs[0].type).toBe('uint256');
    expect(fn.outputs[0].type).toBe('address[]');
  });

  it('should have all state accessor view functions', () => {
    const viewFns = ['stakingToken', 'minStake', 'slashBasisPoints', 'totalStaked', 'owner'];
    for (const name of viewFns) {
      const fn = findFunction(name);
      expect(fn).toBeDefined();
      expect(fn.stateMutability).toBe('view');
    }
  });

  it('should have all admin functions', () => {
    const adminFns = ['setMinStake', 'setSlashBasisPoints', 'setSlasher'];
    for (const name of adminFns) {
      const fn = findFunction(name);
      expect(fn).toBeDefined();
      expect(fn.stateMutability).toBe('nonpayable');
    }
  });

  it('should have all expected events', () => {
    const events = [
      'ProverRegistered',
      'ProverDeactivated',
      'ProverReactivated',
      'StakeAdded',
      'StakeWithdrawn',
      'ProverSlashed',
      'ReputationUpdated',
      'RewardDistributed',
      'SlasherUpdated',
    ];
    for (const name of events) {
      expect(findEvent(name)).toBeDefined();
    }
  });

  it('should have all expected custom errors', () => {
    const errors = [
      'InsufficientStake',
      'ProverNotRegistered',
      'ProverAlreadyRegistered',
      'ProverNotActive',
      'UnauthorizedSlasher',
      'WithdrawalWouldBreachMinimum',
      'NoStakeToWithdraw',
    ];
    for (const name of errors) {
      expect(findError(name)).toBeDefined();
    }
  });
});

// ---------------------------------------------------------------------------
// ProverRegistryClient class tests
// ---------------------------------------------------------------------------

describe('ProverRegistryClient', () => {
  it('should have all expected public methods', () => {
    const proto = ProverRegistryClient.prototype;
    const expectedMethods = [
      // Registration
      'registerProver',
      'deregisterProver',
      'reactivateProver',
      // Staking
      'addStake',
      'withdrawStake',
      // Read operations
      'getProverInfo',
      'isProverActive',
      'isProver',
      'getActiveProvers',
      'activeProverCount',
      'getWeight',
      'selectProver',
      'getTopProvers',
      // State accessors
      'stakingToken',
      'minStake',
      'slashBasisPoints',
      'totalStaked',
      'owner',
      // Slashing & Rewards
      'slash',
      'recordSuccess',
      // Admin
      'setMinStake',
      'setSlashBasisPoints',
      'setSlasher',
    ];
    for (const method of expectedMethods) {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      expect(typeof (proto as any)[method]).toBe('function');
    }
  });

  it('should be constructable with publicClient only (read-only mode)', () => {
    const mockPublicClient = {
      readContract: async () => 0n,
      waitForTransactionReceipt: async () => ({}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const client = new ProverRegistryClient(
      mockPublicClient,
      undefined,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

    expect(client).toBeInstanceOf(ProverRegistryClient);
  });

  it('should throw when calling registerProver without walletClient', async () => {
    const mockPublicClient = {
      readContract: async () => 0n,
      waitForTransactionReceipt: async () => ({}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const client = new ProverRegistryClient(
      mockPublicClient,
      undefined,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

    await expect(client.registerProver(1000n)).rejects.toThrow(
      'walletClient is required for write operations',
    );
  });

  it('should throw when calling deregisterProver without walletClient', async () => {
    const mockPublicClient = {
      readContract: async () => 0n,
      waitForTransactionReceipt: async () => ({}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const client = new ProverRegistryClient(
      mockPublicClient,
      undefined,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

    await expect(client.deregisterProver()).rejects.toThrow(
      'walletClient is required',
    );
  });

  it('should throw when calling addStake without walletClient', async () => {
    const mockPublicClient = {
      readContract: async () => 0n,
      waitForTransactionReceipt: async () => ({}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const client = new ProverRegistryClient(
      mockPublicClient,
      undefined,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

    await expect(client.addStake(500n)).rejects.toThrow(
      'walletClient is required',
    );
  });

  it('should throw when calling withdrawStake without walletClient', async () => {
    const mockPublicClient = {
      readContract: async () => 0n,
      waitForTransactionReceipt: async () => ({}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const client = new ProverRegistryClient(
      mockPublicClient,
      undefined,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

    await expect(client.withdrawStake(100n)).rejects.toThrow(
      'walletClient is required',
    );
  });

  it('should throw when calling slash without walletClient', async () => {
    const mockPublicClient = {
      readContract: async () => 0n,
      waitForTransactionReceipt: async () => ({}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const client = new ProverRegistryClient(
      mockPublicClient,
      undefined,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

    await expect(
      client.slash('0x1234567890abcdef1234567890abcdef12345678', 'bad proof'),
    ).rejects.toThrow('walletClient is required');
  });

  it('should throw when calling setMinStake without walletClient', async () => {
    const mockPublicClient = {
      readContract: async () => 0n,
      waitForTransactionReceipt: async () => ({}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const client = new ProverRegistryClient(
      mockPublicClient,
      undefined,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

    await expect(client.setMinStake(2000n)).rejects.toThrow(
      'walletClient is required',
    );
  });
});

// ---------------------------------------------------------------------------
// Exports validation
// ---------------------------------------------------------------------------

describe('index exports', () => {
  it('should export ProverRegistryClient from the package', async () => {
    const mod = await import('../index');
    expect(mod.ProverRegistryClient).toBeDefined();
    expect(typeof mod.ProverRegistryClient).toBe('function');
  });

  it('should export proverRegistryAbi from the package', async () => {
    const mod = await import('../index');
    expect(mod.proverRegistryAbi).toBeDefined();
    expect(Array.isArray(mod.proverRegistryAbi)).toBe(true);
  });
});
