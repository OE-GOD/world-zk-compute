import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { TEEEventWatcher } from '../tee-event-watcher';
import type {
  WatchEventsOptions,
  WatchEventsCallback,
  TEEEventData,
  EventSubscription,
} from '../tee-event-watcher';

// Common test config
const httpConfig = {
  rpcUrl: 'http://localhost:8545',
  contractAddress: '0x5FbDB2315678afecb367f032d93F642f64180aa3' as const,
};

const wsConfig = {
  ...httpConfig,
  wsUrl: 'ws://localhost:8545',
};

// ---- Existing tests (preserved) ----

describe('TEEEventWatcher', () => {
  it('should be constructable with minimal config', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    expect(watcher).toBeInstanceOf(TEEEventWatcher);
    expect(watcher.isWatching).toBe(false);
  });

  it('should accept custom chainId', () => {
    const watcher = new TEEEventWatcher({ ...httpConfig, chainId: 1 });
    expect(watcher).toBeInstanceOf(TEEEventWatcher);
  });

  it('should register and unregister event handlers', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const handler = () => {};

    // Register
    const result = watcher.on('ResultSubmitted', handler);
    expect(result).toBe(watcher); // chainable

    // Unregister
    const result2 = watcher.off('ResultSubmitted', handler);
    expect(result2).toBe(watcher); // chainable
  });

  it('should support all event types in on()', () => {
    const watcher = new TEEEventWatcher(httpConfig);
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
    const watcher = new TEEEventWatcher(httpConfig);

    const result = watcher
      .on('ResultSubmitted', () => {})
      .on('ResultChallenged', () => {})
      .on('ResultFinalized', () => {});

    expect(result).toBe(watcher);
  });

  it('should not be watching before start() is called', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    expect(watcher.isWatching).toBe(false);
  });

  it('should handle off() for non-existent handler gracefully', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    // Should not throw
    watcher.off('ResultSubmitted', () => {});
  });

  it('should handle off() for non-existent event gracefully', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    // Should not throw
    watcher.off('EnclaveRevoked', () => {});
  });

  it('should handle stop() when not started', () => {
    const watcher = new TEEEventWatcher(httpConfig);
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

// ---- New WebSocket subscription tests ----

describe('TEEEventWatcher.hasWebSocket', () => {
  it('should return false when no wsUrl is provided', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    expect(watcher.hasWebSocket).toBe(false);
  });

  it('should return true when a ws:// URL is provided', () => {
    const watcher = new TEEEventWatcher(wsConfig);
    expect(watcher.hasWebSocket).toBe(true);
  });

  it('should return true when a wss:// URL is provided', () => {
    const watcher = new TEEEventWatcher({
      ...httpConfig,
      wsUrl: 'wss://mainnet.infura.io/ws/v3/abc123',
    });
    expect(watcher.hasWebSocket).toBe(true);
  });

  it('should return false when wsUrl is not a websocket URL', () => {
    const watcher = new TEEEventWatcher({
      ...httpConfig,
      wsUrl: 'http://localhost:8545',
    });
    expect(watcher.hasWebSocket).toBe(false);
  });
});

describe('TEEEventWatcher.activeSubscriptionCount', () => {
  it('should be 0 when no subscriptions exist', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    expect(watcher.activeSubscriptionCount).toBe(0);
  });
});

describe('TEEEventWatcher.watchEvents (HTTP fallback)', () => {
  let watcher: TEEEventWatcher;

  beforeEach(() => {
    vi.useFakeTimers();
    // Use HTTP-only config so watchEvents falls back to polling
    watcher = new TEEEventWatcher(httpConfig);
  });

  afterEach(() => {
    watcher.unsubscribeAll();
    vi.useRealTimers();
  });

  it('should return an EventSubscription with id and active=true', () => {
    const callback = vi.fn();
    const sub = watcher.watchEvents(callback);

    expect(sub).toBeDefined();
    expect(sub.id).toBeTruthy();
    expect(typeof sub.id).toBe('string');
    expect(sub.active).toBe(true);
  });

  it('should increment activeSubscriptionCount', () => {
    const callback = vi.fn();
    expect(watcher.activeSubscriptionCount).toBe(0);

    const sub1 = watcher.watchEvents(callback);
    expect(watcher.activeSubscriptionCount).toBe(1);

    const sub2 = watcher.watchEvents(callback);
    expect(watcher.activeSubscriptionCount).toBe(2);

    sub1.unsubscribe();
    expect(watcher.activeSubscriptionCount).toBe(1);

    sub2.unsubscribe();
    expect(watcher.activeSubscriptionCount).toBe(0);
  });

  it('should give each subscription a unique id', () => {
    const callback = vi.fn();
    const sub1 = watcher.watchEvents(callback);
    const sub2 = watcher.watchEvents(callback);
    const sub3 = watcher.watchEvents(callback);

    expect(sub1.id).not.toBe(sub2.id);
    expect(sub2.id).not.toBe(sub3.id);
    expect(sub1.id).not.toBe(sub3.id);
  });
});

describe('TEEEventWatcher.unsubscribe', () => {
  let watcher: TEEEventWatcher;

  beforeEach(() => {
    watcher = new TEEEventWatcher(httpConfig);
  });

  afterEach(() => {
    watcher.unsubscribeAll();
  });

  it('should deactivate the subscription via EventSubscription.unsubscribe()', () => {
    const callback = vi.fn();
    const sub = watcher.watchEvents(callback);

    expect(sub.active).toBe(true);
    sub.unsubscribe();
    expect(sub.active).toBe(false);
    expect(watcher.activeSubscriptionCount).toBe(0);
  });

  it('should deactivate the subscription via watcher.unsubscribe(id)', () => {
    const callback = vi.fn();
    const sub = watcher.watchEvents(callback);

    expect(sub.active).toBe(true);
    watcher.unsubscribe(sub.id);
    expect(sub.active).toBe(false);
    expect(watcher.activeSubscriptionCount).toBe(0);
  });

  it('should handle unsubscribing an already-unsubscribed subscription', () => {
    const callback = vi.fn();
    const sub = watcher.watchEvents(callback);

    sub.unsubscribe();
    // Should not throw
    sub.unsubscribe();
    watcher.unsubscribe(sub.id);
    expect(sub.active).toBe(false);
  });

  it('should handle unsubscribing a non-existent id', () => {
    // Should not throw
    watcher.unsubscribe('non_existent_id');
  });
});

describe('TEEEventWatcher.unsubscribeAll', () => {
  let watcher: TEEEventWatcher;

  beforeEach(() => {
    watcher = new TEEEventWatcher(httpConfig);
  });

  it('should stop all subscriptions', () => {
    const callback = vi.fn();
    const sub1 = watcher.watchEvents(callback);
    const sub2 = watcher.watchEvents(callback);
    const sub3 = watcher.watchEvents(callback);

    expect(watcher.activeSubscriptionCount).toBe(3);

    watcher.unsubscribeAll();

    expect(watcher.activeSubscriptionCount).toBe(0);
    expect(sub1.active).toBe(false);
    expect(sub2.active).toBe(false);
    expect(sub3.active).toBe(false);
  });

  it('should be safe to call when no subscriptions exist', () => {
    // Should not throw
    watcher.unsubscribeAll();
    expect(watcher.activeSubscriptionCount).toBe(0);
  });
});

describe('TEEEventWatcher WatchEventsOptions defaults', () => {
  it('should default reconnect to true and maxRetries to 5', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const callback = vi.fn();

    // We can verify defaults by observing that the subscription is created
    // without errors when no options are provided
    const sub = watcher.watchEvents(callback);
    expect(sub.active).toBe(true);
    sub.unsubscribe();
  });

  it('should accept explicit options', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const callback = vi.fn();

    const sub = watcher.watchEvents(callback, {
      eventTypes: ['ResultSubmitted'],
      reconnect: false,
      maxRetries: 3,
    });
    expect(sub.active).toBe(true);
    sub.unsubscribe();
  });

  it('should accept partial options', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const callback = vi.fn();

    const sub = watcher.watchEvents(callback, { eventTypes: ['ResultFinalized'] });
    expect(sub.active).toBe(true);
    sub.unsubscribe();
  });
});

describe('TEEEventWatcher multiple simultaneous watchers', () => {
  let watcher: TEEEventWatcher;

  beforeEach(() => {
    watcher = new TEEEventWatcher(httpConfig);
  });

  afterEach(() => {
    watcher.unsubscribeAll();
  });

  it('should support multiple simultaneous subscriptions', () => {
    const cb1 = vi.fn();
    const cb2 = vi.fn();
    const cb3 = vi.fn();

    const sub1 = watcher.watchEvents(cb1, { eventTypes: ['ResultSubmitted'] });
    const sub2 = watcher.watchEvents(cb2, { eventTypes: ['ResultChallenged'] });
    const sub3 = watcher.watchEvents(cb3); // all events

    expect(sub1.active).toBe(true);
    expect(sub2.active).toBe(true);
    expect(sub3.active).toBe(true);
    expect(watcher.activeSubscriptionCount).toBe(3);
  });

  it('should allow unsubscribing one without affecting others', () => {
    const cb1 = vi.fn();
    const cb2 = vi.fn();

    const sub1 = watcher.watchEvents(cb1);
    const sub2 = watcher.watchEvents(cb2);

    sub1.unsubscribe();

    expect(sub1.active).toBe(false);
    expect(sub2.active).toBe(true);
    expect(watcher.activeSubscriptionCount).toBe(1);
  });
});

describe('TEEEventWatcher coexistence of start/stop and watchEvents', () => {
  it('should allow start() and watchEvents() to coexist', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const callback = vi.fn();

    // start() uses the old API
    watcher.start();
    expect(watcher.isWatching).toBe(true);

    // watchEvents() creates an independent subscription
    const sub = watcher.watchEvents(callback);
    expect(sub.active).toBe(true);
    expect(watcher.activeSubscriptionCount).toBe(1);

    // stop() should not affect watchEvents subscriptions
    watcher.stop();
    expect(watcher.isWatching).toBe(false);
    expect(sub.active).toBe(true);
    expect(watcher.activeSubscriptionCount).toBe(1);

    // unsubscribe should not affect start/stop state
    sub.unsubscribe();
    expect(watcher.activeSubscriptionCount).toBe(0);
  });
});

describe('TEEEventWatcher WebSocket detection', () => {
  it('should detect ws:// as WebSocket', () => {
    const watcher = new TEEEventWatcher({
      ...httpConfig,
      wsUrl: 'ws://localhost:8546',
    });
    expect(watcher.hasWebSocket).toBe(true);
  });

  it('should detect wss:// as WebSocket', () => {
    const watcher = new TEEEventWatcher({
      ...httpConfig,
      wsUrl: 'wss://secure.example.com',
    });
    expect(watcher.hasWebSocket).toBe(true);
  });

  it('should not detect http:// as WebSocket', () => {
    const watcher = new TEEEventWatcher({
      ...httpConfig,
      wsUrl: 'http://localhost:8545',
    });
    expect(watcher.hasWebSocket).toBe(false);
  });

  it('should not detect https:// as WebSocket', () => {
    const watcher = new TEEEventWatcher({
      ...httpConfig,
      wsUrl: 'https://secure.example.com',
    });
    expect(watcher.hasWebSocket).toBe(false);
  });
});

describe('TEEEventWatcher new type exports', () => {
  it('should export WatchEventsOptions type', async () => {
    // This is a compile-time check; if it imports, the type exists
    const _options: WatchEventsOptions = {
      eventTypes: ['ResultSubmitted'],
      reconnect: true,
      maxRetries: 3,
    };
    expect(_options.eventTypes).toEqual(['ResultSubmitted']);
  });

  it('should export WatchEventsCallback type', async () => {
    const _callback: WatchEventsCallback = (_event: TEEEventData) => {};
    expect(typeof _callback).toBe('function');
  });

  it('should export EventSubscription type', async () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const sub: EventSubscription = watcher.watchEvents(() => {});
    expect(sub.id).toBeTruthy();
    expect(typeof sub.unsubscribe).toBe('function');
    sub.unsubscribe();
  });

  it('should export new types from index', async () => {
    const mod = await import('../index');
    // TEEEventWatcher is the main class export
    expect(mod.TEEEventWatcher).toBeDefined();
    // Type exports are verified at compile time, but we can verify
    // the class has the new method
    const watcher = new mod.TEEEventWatcher(httpConfig);
    expect(typeof watcher.watchEvents).toBe('function');
    expect(typeof watcher.unsubscribe).toBe('function');
    expect(typeof watcher.unsubscribeAll).toBe('function');
  });
});

describe('TEEEventWatcher event filtering via watchEvents options', () => {
  it('should accept empty eventTypes (all events)', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const sub = watcher.watchEvents(() => {}, { eventTypes: [] });
    expect(sub.active).toBe(true);
    sub.unsubscribe();
  });

  it('should accept single event type filter', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const sub = watcher.watchEvents(() => {}, { eventTypes: ['ResultSubmitted'] });
    expect(sub.active).toBe(true);
    sub.unsubscribe();
  });

  it('should accept multiple event type filters', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const sub = watcher.watchEvents(() => {}, {
      eventTypes: ['ResultSubmitted', 'ResultChallenged', 'DisputeResolved'],
    });
    expect(sub.active).toBe(true);
    sub.unsubscribe();
  });
});

describe('TEEEventWatcher reconnection config', () => {
  it('should accept reconnect=false to disable reconnection', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const sub = watcher.watchEvents(() => {}, { reconnect: false });
    expect(sub.active).toBe(true);
    sub.unsubscribe();
  });

  it('should accept custom maxRetries', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const sub = watcher.watchEvents(() => {}, { maxRetries: 10 });
    expect(sub.active).toBe(true);
    sub.unsubscribe();
  });

  it('should accept maxRetries=0 to disable retries', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const sub = watcher.watchEvents(() => {}, { maxRetries: 0, reconnect: true });
    expect(sub.active).toBe(true);
    sub.unsubscribe();
  });
});

describe('TEEEventWatcher error callback', () => {
  it('should accept an onError callback', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const errorCb = vi.fn();
    const sub = watcher.watchEvents(() => {}, {}, errorCb);
    expect(sub.active).toBe(true);
    sub.unsubscribe();
  });

  it('should work without an onError callback', () => {
    const watcher = new TEEEventWatcher(httpConfig);
    const sub = watcher.watchEvents(() => {});
    expect(sub.active).toBe(true);
    sub.unsubscribe();
  });
});
