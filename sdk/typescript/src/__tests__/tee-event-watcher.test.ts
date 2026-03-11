import { describe, it, expect } from 'vitest';
import { TEEEventWatcher } from '../tee-event-watcher';

describe('TEEEventWatcher', () => {
  const config = {
    rpcUrl: 'http://localhost:8545',
    contractAddress: '0x5FbDB2315678afecb367f032d93F642f64180aa3' as const,
  };

  it('should be constructable with minimal config', () => {
    const watcher = new TEEEventWatcher(config);
    expect(watcher).toBeInstanceOf(TEEEventWatcher);
    expect(watcher.isWatching).toBe(false);
  });

  it('should accept custom chainId', () => {
    const watcher = new TEEEventWatcher({ ...config, chainId: 1 });
    expect(watcher).toBeInstanceOf(TEEEventWatcher);
  });

  it('should register and unregister event handlers', () => {
    const watcher = new TEEEventWatcher(config);
    const handler = () => {};

    // Register
    const result = watcher.on('ResultSubmitted', handler);
    expect(result).toBe(watcher); // chainable

    // Unregister
    const result2 = watcher.off('ResultSubmitted', handler);
    expect(result2).toBe(watcher); // chainable
  });

  it('should support all event types in on()', () => {
    const watcher = new TEEEventWatcher(config);
    const noop = () => {};

    // These should all compile and run without errors
    watcher.on('ResultSubmitted', noop);
    watcher.on('ResultChallenged', noop);
    watcher.on('ResultFinalized', noop);
    watcher.on('DisputeResolved', noop);
    watcher.on('EnclaveRegistered', noop);
    watcher.on('EnclaveRevoked', noop);
  });

  it('should chain multiple on() calls', () => {
    const watcher = new TEEEventWatcher(config);

    const result = watcher
      .on('ResultSubmitted', () => {})
      .on('ResultChallenged', () => {})
      .on('ResultFinalized', () => {});

    expect(result).toBe(watcher);
  });

  it('should not be watching before start() is called', () => {
    const watcher = new TEEEventWatcher(config);
    expect(watcher.isWatching).toBe(false);
  });

  it('should handle off() for non-existent handler gracefully', () => {
    const watcher = new TEEEventWatcher(config);
    // Should not throw
    watcher.off('ResultSubmitted', () => {});
  });

  it('should handle off() for non-existent event gracefully', () => {
    const watcher = new TEEEventWatcher(config);
    // Should not throw
    watcher.off('EnclaveRevoked', () => {});
  });

  it('should handle stop() when not started', () => {
    const watcher = new TEEEventWatcher(config);
    // Should not throw
    watcher.stop();
    expect(watcher.isWatching).toBe(false);
  });
});

describe('TEEEventWatcher exports', () => {
  it('should export TEEEventWatcher class', async () => {
    const mod = await import('../index');
    expect(mod.TEEEventWatcher).toBeDefined();
  });
});
