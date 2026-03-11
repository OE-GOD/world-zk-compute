import { describe, it, expect } from 'vitest';
import { keccak256, toHex } from 'viem';
import {
  computeModelHash,
  computeInputHash,
  computeResultHash,
  computeInputHashFromJson,
  computeResultHashFromBytes,
  serializeF64List,
} from '../hash';

describe('serializeF64List', () => {
  it('should serialize basic floats matching Rust serde_json', () => {
    const result = new TextDecoder().decode(serializeF64List([1.0, 2.5, 3.7]));
    expect(result).toBe('[1.0,2.5,3.7]');
  });

  it('should add .0 suffix to integer-valued floats', () => {
    const result = new TextDecoder().decode(serializeF64List([1.0, 2.0, 3.0]));
    expect(result).toBe('[1.0,2.0,3.0]');
  });

  it('should handle negative floats', () => {
    const result = new TextDecoder().decode(serializeF64List([-1.5, 0.0, 2.3]));
    expect(result).toBe('[-1.5,0.0,2.3]');
  });

  it('should handle single element', () => {
    const result = new TextDecoder().decode(serializeF64List([42.5]));
    expect(result).toBe('[42.5]');
  });

  it('should handle empty list', () => {
    const result = new TextDecoder().decode(serializeF64List([]));
    expect(result).toBe('[]');
  });

  it('should handle prediction scores', () => {
    const result = new TextDecoder().decode(serializeF64List([0.85]));
    expect(result).toBe('[0.85]');
  });

  it('should handle multiclass scores', () => {
    const result = new TextDecoder().decode(serializeF64List([0.1, 0.8, 0.1]));
    expect(result).toBe('[0.1,0.8,0.1]');
  });

  it('should handle zero with .0 suffix', () => {
    const result = new TextDecoder().decode(serializeF64List([0.0]));
    expect(result).toBe('[0.0]');
  });
});

describe('computeModelHash', () => {
  it('should compute keccak256 of empty bytes', () => {
    const result = computeModelHash(new Uint8Array(0));
    // keccak256 of empty input is a well-known constant
    expect(result).toBe('0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470');
  });

  it('should be deterministic', () => {
    const data = new TextEncoder().encode('hello model');
    const hash1 = computeModelHash(data);
    const hash2 = computeModelHash(data);
    expect(hash1).toBe(hash2);
  });

  it('should produce different hashes for different inputs', () => {
    const hashA = computeModelHash(new TextEncoder().encode('model_a'));
    const hashB = computeModelHash(new TextEncoder().encode('model_b'));
    expect(hashA).not.toBe(hashB);
  });

  it('should accept hex input', () => {
    const data = new TextEncoder().encode('hello');
    const hashFromBytes = computeModelHash(data);
    const hashFromHex = computeModelHash(toHex(data));
    expect(hashFromBytes).toBe(hashFromHex);
  });

  it('should return 0x-prefixed 66-char hex string', () => {
    const result = computeModelHash(new TextEncoder().encode('test'));
    expect(result).toMatch(/^0x[0-9a-f]{64}$/);
  });
});

describe('computeInputHash', () => {
  it('should match keccak256 of JSON-serialized features', () => {
    const features = [1.0, 2.5, 3.7];
    const result = computeInputHash(features);

    // Manually compute: keccak256("[1.0,2.5,3.7]")
    const jsonBytes = new TextEncoder().encode('[1.0,2.5,3.7]');
    const expected = keccak256(toHex(jsonBytes));
    expect(result).toBe(expected);
  });

  it('should handle empty features', () => {
    const result = computeInputHash([]);
    const expected = keccak256(toHex(new TextEncoder().encode('[]')));
    expect(result).toBe(expected);
  });

  it('should handle single feature', () => {
    const result = computeInputHash([42.5]);
    const expected = keccak256(toHex(new TextEncoder().encode('[42.5]')));
    expect(result).toBe(expected);
  });

  it('should handle negative features', () => {
    const result = computeInputHash([-1.5, 0.0, 2.3]);
    const expected = keccak256(toHex(new TextEncoder().encode('[-1.5,0.0,2.3]')));
    expect(result).toBe(expected);
  });

  it('should produce different hashes for different orderings', () => {
    const hashA = computeInputHash([1.0, 2.0, 3.0]);
    const hashB = computeInputHash([3.0, 2.0, 1.0]);
    expect(hashA).not.toBe(hashB);
  });
});

describe('computeResultHash', () => {
  it('should handle binary prediction', () => {
    const result = computeResultHash([0.85]);
    const expected = keccak256(toHex(new TextEncoder().encode('[0.85]')));
    expect(result).toBe(expected);
  });

  it('should handle multiclass prediction', () => {
    const result = computeResultHash([0.1, 0.8, 0.1]);
    const expected = keccak256(toHex(new TextEncoder().encode('[0.1,0.8,0.1]')));
    expect(result).toBe(expected);
  });
});

describe('computeInputHashFromJson', () => {
  it('should match computeInputHash for same data', () => {
    const features = [1.0, 2.5, 3.7];
    const hashFromList = computeInputHash(features);

    const jsonBytes = new TextEncoder().encode('[1.0,2.5,3.7]');
    const hashFromJson = computeInputHashFromJson(jsonBytes);

    expect(hashFromList).toBe(hashFromJson);
  });
});

describe('computeResultHashFromBytes', () => {
  it('should match computeResultHash for same data', () => {
    const scores = [0.85];
    const hashFromScores = computeResultHash(scores);

    const resultBytes = new TextEncoder().encode('[0.85]');
    const hashFromBytes = computeResultHashFromBytes(resultBytes);

    expect(hashFromScores).toBe(hashFromBytes);
  });
});

describe('cross-language compatibility', () => {
  it('should produce the well-known keccak256 of empty bytes', () => {
    const result = computeModelHash(new Uint8Array(0));
    expect(result).toBe('0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470');
  });

  it('should produce consistent JSON serialization across all test cases', () => {
    const testCases: [number[], string][] = [
      [[1.0, 2.5, 3.7], '[1.0,2.5,3.7]'],
      [[1.0, 2.0, 3.0], '[1.0,2.0,3.0]'],
      [[-1.5, 0.0, 2.3], '[-1.5,0.0,2.3]'],
      [[0.85], '[0.85]'],
      [[0.1, 0.8, 0.1], '[0.1,0.8,0.1]'],
      [[], '[]'],
    ];

    for (const [values, expectedJson] of testCases) {
      const jsonBytes = serializeF64List(values);
      const jsonStr = new TextDecoder().decode(jsonBytes);
      expect(jsonStr).toBe(expectedJson);
    }
  });
});
