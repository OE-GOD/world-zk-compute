# World ZK Compute TypeScript SDK

TypeScript/JavaScript client library for the World ZK Compute decentralized proving network.

## Installation

```bash
npm install @worldzk/sdk
# or
yarn add @worldzk/sdk
# or
pnpm add @worldzk/sdk
```

## Quick Start

```typescript
import { Client } from '@worldzk/sdk';

// Initialize client
const client = new Client({ apiKey: 'your-api-key' });

// Submit a computation request
const request = await client.requests.create({
  imageId: '0x1234567890abcdef...',
  inputData: new TextEncoder().encode('hello world'),
});

console.log(`Request ID: ${request.id}`);
console.log(`Status: ${request.status}`);

// Wait for completion
const result = await client.requests.wait(request.id);
console.log('Proof:', result.proof);
```

## API Reference

### Client

```typescript
import { Client } from '@worldzk/sdk';

const client = new Client({
  baseUrl: 'https://api.worldzk.compute/v1', // API base URL
  apiKey: 'your-api-key',                     // Optional API key
  timeout: 30000,                             // Request timeout (ms)
  maxRetries: 3,                              // Max retry attempts
});
```

### Requests

```typescript
// List requests
const { items, pagination } = await client.requests.list({
  status: RequestStatus.Pending,
  imageId: '0x...',
  limit: 20,
  offset: 0,
});

// Create request
const request = await client.requests.create({
  imageId: '0x...',
  inputData: new Uint8Array([...]),    // Raw input bytes
  // OR
  inputUrl: 'ipfs://...',              // URL to fetch input
  inputHash: '0x...',                  // Required with inputUrl
  callbackContract: '0x...',           // Optional callback
  expirationSeconds: 3600,
  tip: 1000000000000000n,              // Wei (bigint)
});

// Get request
const request = await client.requests.get(123);

// Cancel request
const request = await client.requests.cancel(123);

// Wait for completion
const request = await client.requests.wait(123, {
  timeout: 300000,      // Max wait time (ms)
  pollInterval: 2000,   // Check interval (ms)
});
```

### Programs

```typescript
// List programs
const { items } = await client.programs.list({ active: true });

// Get program
const program = await client.programs.get('0x...');
```

### Provers

```typescript
// List provers
const { items } = await client.provers.list({ tier: ReputationTier.Gold });

// Get prover
const prover = await client.provers.get('0x...');

// Register as prover
const prover = await client.provers.register({
  name: 'My Prover',
  endpoint: 'https://my-prover.example.com',
});
```

## Error Handling

```typescript
import {
  Client,
  ApiError,
  RateLimitError,
  NotFoundError,
} from '@worldzk/sdk';

const client = new Client({ apiKey: 'your-api-key' });

try {
  const request = await client.requests.get(12345);
} catch (e) {
  if (e instanceof NotFoundError) {
    console.log(`Request not found: ${e.message}`);
  } else if (e instanceof RateLimitError) {
    console.log(`Rate limited, retry after ${e.retryAfterSeconds}s`);
  } else if (e instanceof ApiError) {
    console.log(`API error [${e.code}]: ${e.message}`);
    if (e.isRetryable) {
      console.log(`Retry after ${e.retryDelayMs}ms`);
    }
  }
}
```

### Error Codes

| Code | Description |
|------|-------------|
| WZK-1xxx | Client errors (validation, auth) |
| WZK-2xxx | Server errors |
| WZK-3xxx | Proof errors |
| WZK-4xxx | Contract/chain errors |
| WZK-5xxx | Network errors |

## Types

### ExecutionRequest

```typescript
interface ExecutionRequest {
  id: number;
  requester: string;
  imageId: string;
  inputHash: string;
  status: RequestStatus;
  createdAt: Date;
  inputUrl?: string;
  callbackContract?: string;
  tip: bigint;
  claimedBy?: string;
  claimDeadline?: Date;
  expiresAt?: Date;
  completedAt?: Date;
  proof?: ProofResult;
}
```

### Prover

```typescript
interface Prover {
  address: string;
  reputation: Reputation;
  stats: ProverStats;
  registeredAt?: Date;
  lastActiveAt?: Date;
}

interface Reputation {
  score: number;        // 0-10000 basis points
  tier: ReputationTier;
  isSlashed: boolean;
  isBanned: boolean;
}

interface ProverStats {
  totalJobs: number;
  completedJobs: number;
  failedJobs: number;
  abandonedJobs: number;
  avgProofTimeMs: number;
  totalEarnings: bigint;
}
```

## Browser Support

This SDK works in both Node.js and browser environments. It uses the native `fetch` API which is available in:
- Node.js 18+
- All modern browsers

## License

Apache-2.0
