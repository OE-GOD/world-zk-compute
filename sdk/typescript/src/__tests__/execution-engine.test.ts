import { describe, it, expect } from 'vitest';
import { executionEngineAbi } from '../abi';
import {
  ExecutionEngineClient,
  OnChainRequestStatus,
} from '../execution-engine';
import { ApiError } from '../errors';

// ---------------------------------------------------------------------------
// ABI validation tests
// ---------------------------------------------------------------------------

describe('executionEngineAbi', () => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const findFunction = (name: string): any =>
    executionEngineAbi.find(
      (e) => e.type === 'function' && e.name === name,
    );

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const findEvent = (name: string): any =>
    executionEngineAbi.find(
      (e) => e.type === 'event' && e.name === name,
    );

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const findError = (name: string): any =>
    executionEngineAbi.find(
      (e) => e.type === 'error' && e.name === name,
    );

  it('should have requestExecution function (6-param overload)', () => {
    const fns = executionEngineAbi.filter(
      (e) => e.type === 'function' && e.name === 'requestExecution',
    );
    expect(fns.length).toBe(2);
    // 6-param version includes inputType
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const sixParam = fns.find((f: any) => f.inputs.length === 6);
    expect(sixParam).toBeDefined();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect((sixParam as any).stateMutability).toBe('payable');
  });

  it('should have requestExecution function (5-param overload)', () => {
    const fns = executionEngineAbi.filter(
      (e) => e.type === 'function' && e.name === 'requestExecution',
    );
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const fiveParam = fns.find((f: any) => f.inputs.length === 5);
    expect(fiveParam).toBeDefined();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect((fiveParam as any).stateMutability).toBe('payable');
  });

  it('should have claimExecution function', () => {
    const fn = findFunction('claimExecution');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
    expect(fn.inputs).toHaveLength(1);
    expect(fn.inputs[0].type).toBe('uint256');
  });

  it('should have submitProof function', () => {
    const fn = findFunction('submitProof');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
    expect(fn.inputs).toHaveLength(3);
    expect(fn.inputs[0].name).toBe('requestId');
    expect(fn.inputs[1].name).toBe('seal');
    expect(fn.inputs[2].name).toBe('journal');
  });

  it('should have cancelExecution function', () => {
    const fn = findFunction('cancelExecution');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('nonpayable');
    expect(fn.inputs).toHaveLength(1);
  });

  it('should have getRequest view function', () => {
    const fn = findFunction('getRequest');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.outputs).toHaveLength(1);
    expect(fn.outputs[0].type).toBe('tuple');
    // Should have all struct fields
    const components = fn.outputs[0].components;
    const fieldNames = components.map((c: { name: string }) => c.name);
    expect(fieldNames).toContain('id');
    expect(fieldNames).toContain('imageId');
    expect(fieldNames).toContain('inputDigest');
    expect(fieldNames).toContain('requester');
    expect(fieldNames).toContain('status');
    expect(fieldNames).toContain('claimedBy');
    expect(fieldNames).toContain('tip');
    expect(fieldNames).toContain('maxTip');
  });

  it('should have getCurrentTip view function', () => {
    const fn = findFunction('getCurrentTip');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.outputs[0].type).toBe('uint256');
  });

  it('should have getPendingRequests view function', () => {
    const fn = findFunction('getPendingRequests');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.inputs).toHaveLength(2);
    expect(fn.outputs[0].type).toBe('uint256[]');
  });

  it('should have getProverStats view function', () => {
    const fn = findFunction('getProverStats');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.inputs[0].type).toBe('address');
    expect(fn.outputs).toHaveLength(2);
  });

  it('should have all admin functions', () => {
    const adminFns = [
      'pause',
      'unpause',
      'setProtocolFee',
      'setFeeRecipient',
      'setReputation',
      'transferOwnership',
      'acceptOwnership',
    ];
    for (const name of adminFns) {
      expect(findFunction(name)).toBeDefined();
    }
  });

  it('should have all view functions', () => {
    const viewFns = [
      'owner',
      'paused',
      'protocolFeeBps',
      'feeRecipient',
      'nextRequestId',
      'registry',
      'verifier',
      'reputation',
    ];
    for (const name of viewFns) {
      const fn = findFunction(name);
      expect(fn).toBeDefined();
      expect(fn.stateMutability).toBe('view');
    }
  });

  it('should have all expected events', () => {
    const events = [
      'ExecutionRequested',
      'ExecutionClaimed',
      'ExecutionCompleted',
      'ExecutionExpired',
      'ExecutionCancelled',
      'ClaimExpired',
      'CallbackFailed',
      'ProtocolFeeUpdated',
      'FeeRecipientUpdated',
      'ReputationContractSet',
    ];
    for (const name of events) {
      expect(findEvent(name)).toBeDefined();
    }
  });

  it('should have all expected custom errors', () => {
    const errors = [
      'InsufficientTip',
      'ProgramNotActive',
      'RequestNotFound',
      'RequestNotPending',
      'RequestNotClaimed',
      'NotRequester',
      'NotClaimant',
      'ClaimNotExpired',
      'RequestExpired',
      'ClaimDeadlinePassed',
      'InvalidProof',
      'EmptySeal',
      'EmptyJournal',
      'TransferFailed',
      'ZeroImageId',
      'ExpirationInPast',
      'ZeroAddress',
    ];
    for (const name of errors) {
      expect(findError(name)).toBeDefined();
    }
  });

  it('should have MIN_TIP constant accessor', () => {
    const fn = findFunction('MIN_TIP');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
    expect(fn.outputs[0].type).toBe('uint256');
  });

  it('should have CLAIM_WINDOW constant accessor', () => {
    const fn = findFunction('CLAIM_WINDOW');
    expect(fn).toBeDefined();
    expect(fn.stateMutability).toBe('view');
  });
});

// ---------------------------------------------------------------------------
// OnChainRequestStatus enum tests
// ---------------------------------------------------------------------------

describe('OnChainRequestStatus', () => {
  it('should map status values to match the Solidity enum', () => {
    expect(OnChainRequestStatus.Pending).toBe(0);
    expect(OnChainRequestStatus.Claimed).toBe(1);
    expect(OnChainRequestStatus.Completed).toBe(2);
    expect(OnChainRequestStatus.Expired).toBe(3);
    expect(OnChainRequestStatus.Cancelled).toBe(4);
  });
});

// ---------------------------------------------------------------------------
// ExecutionEngineClient class tests
// ---------------------------------------------------------------------------

describe('ExecutionEngineClient', () => {
  it('should have all expected public methods', () => {
    const proto = ExecutionEngineClient.prototype;
    const expectedMethods = [
      // Write operations
      'submitRequest',
      'claimJob',
      'submitResult',
      'cancelRequest',
      'disputeResult',
      // Read operations
      'getRequestStatus',
      'getRequest',
      'getCurrentTip',
      'getPendingRequests',
      'getProverStats',
      'getNextRequestId',
      'isPaused',
      'owner',
      'getProtocolFeeBps',
      'getFeeRecipient',
      // Admin operations
      'pause',
      'unpause',
      'setProtocolFee',
      'setFeeRecipient',
      'setReputation',
      'transferOwnership',
      'acceptOwnership',
    ];
    for (const method of expectedMethods) {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      expect(typeof (proto as any)[method]).toBe('function');
    }
  });

  it('should be constructable with publicClient only (read-only mode)', () => {
    // Minimal mock publicClient
    const mockPublicClient = {
      readContract: async () => 0n,
      waitForTransactionReceipt: async () => ({}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const client = new ExecutionEngineClient(
      mockPublicClient,
      undefined,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

    expect(client).toBeInstanceOf(ExecutionEngineClient);
  });

  it('should throw when calling write method without walletClient', async () => {
    const mockPublicClient = {
      readContract: async () => 0n,
      waitForTransactionReceipt: async () => ({}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const client = new ExecutionEngineClient(
      mockPublicClient,
      undefined,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

    await expect(client.claimJob(1n)).rejects.toThrow(
      'walletClient is required for write operations',
    );
  });

  it('should throw when calling submitRequest without walletClient', async () => {
    const mockPublicClient = {
      readContract: async () => 0n,
      waitForTransactionReceipt: async () => ({}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const client = new ExecutionEngineClient(
      mockPublicClient,
      undefined,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

    await expect(
      client.submitRequest({
        imageId: '0x0000000000000000000000000000000000000000000000000000000000000001',
        inputDigest: '0x0000000000000000000000000000000000000000000000000000000000000002',
        tipEther: '0.001',
      }),
    ).rejects.toThrow('walletClient is required');
  });

  it('should throw when calling submitResult without walletClient', async () => {
    const mockPublicClient = {
      readContract: async () => 0n,
      waitForTransactionReceipt: async () => ({}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const client = new ExecutionEngineClient(
      mockPublicClient,
      undefined,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

    await expect(
      client.submitResult(1n, '0xdead', '0xbeef'),
    ).rejects.toThrow('walletClient is required');
  });

  it('should throw when calling disputeResult without walletClient', async () => {
    const mockPublicClient = {
      readContract: async () => 0n,
      waitForTransactionReceipt: async () => ({}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const client = new ExecutionEngineClient(
      mockPublicClient,
      undefined,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

    await expect(client.disputeResult(1n)).rejects.toThrow(
      'walletClient is required',
    );
  });

  it('should throw when calling cancelRequest without walletClient', async () => {
    const mockPublicClient = {
      readContract: async () => 0n,
      waitForTransactionReceipt: async () => ({}),
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
    } as any;

    const client = new ExecutionEngineClient(
      mockPublicClient,
      undefined,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

    await expect(client.cancelRequest(1n)).rejects.toThrow(
      'walletClient is required',
    );
  });

  // A mined transaction that reverted still yields a receipt; only its
  // status says the call failed. Every write method must look at it.
  const clientWithReceipt = (status: 'success' | 'reverted') =>
    new ExecutionEngineClient(
      {
        waitForTransactionReceipt: async () => ({ status, blockNumber: 7n }),
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
      } as any,
      {
        writeContract: async () => '0xdead',
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
      } as any,
      '0x1234567890abcdef1234567890abcdef12345678',
    );

  const writes: [string, (c: ExecutionEngineClient) => Promise<unknown>][] = [
    [
      'requestExecution',
      (c) =>
        c.submitRequest({
          imageId: '0x0000000000000000000000000000000000000000000000000000000000000001',
          inputDigest: '0x0000000000000000000000000000000000000000000000000000000000000002',
          tipEther: '0.001',
        }),
    ],
    ['claimExecution', (c) => c.claimJob(1n)],
    ['submitProof', (c) => c.submitResult(1n, '0xdead', '0xbeef')],
    ['cancelExecution', (c) => c.cancelRequest(1n)],
    ['claimExecution', (c) => c.disputeResult(1n)],
    ['pause', (c) => c.pause()],
  ];

  it('rejects every write whose transaction was mined but reverted', async () => {
    for (const [functionName, write] of writes) {
      const err = await write(clientWithReceipt('reverted')).catch((e: unknown) => e);
      expect(err, functionName).toBeInstanceOf(ApiError);
      expect((err as ApiError).code).toBe('WZK-4002');
      expect((err as ApiError).details).toMatchObject({
        transactionHash: '0xdead',
        functionName,
      });
    }
  });

  it('returns the hash of a write whose transaction succeeded', async () => {
    for (const [, write] of writes) {
      await expect(write(clientWithReceipt('success'))).resolves.toBe('0xdead');
    }
  });

  it('sends the submitRequest tip as msg.value', async () => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let sent: any;
    const client = new ExecutionEngineClient(
      {
        waitForTransactionReceipt: async () => ({ status: 'success', blockNumber: 7n }),
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
      } as any,
      {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        writeContract: async (req: any) => {
          sent = req;
          return '0xdead';
        },
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
      } as any,
      '0x1234567890abcdef1234567890abcdef12345678',
    );
    await client.submitRequest({
      imageId: '0x0000000000000000000000000000000000000000000000000000000000000001',
      inputDigest: '0x0000000000000000000000000000000000000000000000000000000000000002',
      tipEther: '0.001',
    });
    expect(sent.functionName).toBe('requestExecution');
    expect(sent.value).toBe(1_000_000_000_000_000n);
    expect(sent.args).toEqual([
      '0x0000000000000000000000000000000000000000000000000000000000000001',
      '0x0000000000000000000000000000000000000000000000000000000000000002',
      '',
      '0x0000000000000000000000000000000000000000',
      0n,
      0,
    ]);
  });
});

// ---------------------------------------------------------------------------
// Exports validation
// ---------------------------------------------------------------------------

describe('index exports', () => {
  it('should export ExecutionEngineClient from the package', async () => {
    const mod = await import('../index');
    expect(mod.ExecutionEngineClient).toBeDefined();
    expect(typeof mod.ExecutionEngineClient).toBe('function');
  });

  it('should export OnChainRequestStatus from the package', async () => {
    const mod = await import('../index');
    expect(mod.OnChainRequestStatus).toBeDefined();
    expect(mod.OnChainRequestStatus.Pending).toBe(0);
  });

  it('should export executionEngineAbi from the package', async () => {
    const mod = await import('../index');
    expect(mod.executionEngineAbi).toBeDefined();
    expect(Array.isArray(mod.executionEngineAbi)).toBe(true);
  });
});
