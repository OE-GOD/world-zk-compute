/**
 * Hash utilities for computing model_hash, input_hash, and result_hash.
 *
 * These functions produce hashes compatible with the TEE enclave's hashing
 * behavior. Using these ensures that on-chain verification will match the
 * enclave's attestation.
 *
 * @example
 * ```typescript
 * import { computeModelHash, computeInputHash, computeResultHash } from '@worldzk/sdk';
 *
 * // Compute model hash from raw bytes
 * const modelBytes = new TextEncoder().encode('{"learner":{}}');
 * const modelHash = computeModelHash(modelBytes);
 *
 * // Compute input hash from features
 * const inputHash = computeInputHash([1.0, 2.5, 3.7]);
 *
 * // Compute result hash from scores
 * const resultHash = computeResultHash([0.85]);
 * ```
 */

import { keccak256, toHex } from 'viem';

type Hex = `0x${string}`;

/**
 * Compute the model hash from raw model file bytes.
 *
 * This matches the enclave's computation: `keccak256(raw_file_bytes)`.
 *
 * @param modelBytes - Raw model file bytes (Uint8Array or hex string)
 * @returns 0x-prefixed hex keccak256 hash
 */
export function computeModelHash(modelBytes: Uint8Array | Hex): Hex {
  const data = modelBytes instanceof Uint8Array ? toHex(modelBytes) : modelBytes;
  return keccak256(data);
}

/**
 * Compute the input hash from a feature vector.
 *
 * This matches the enclave's computation:
 * `keccak256(serde_json::to_vec(&features))`, using {@link serializeF64List}
 * for the JSON bytes.
 *
 * @param features - Array of number feature values
 * @returns 0x-prefixed hex keccak256 hash
 * @throws {RangeError} If any feature is NaN or Infinity
 */
export function computeInputHash(features: number[]): Hex {
  const jsonBytes = serializeF64List(features);
  return keccak256(toHex(jsonBytes));
}

/**
 * Compute the result hash from prediction scores.
 *
 * This matches the enclave's computation:
 * `keccak256(serde_json::to_vec(&scores))`, using {@link serializeF64List}
 * for the JSON bytes.
 *
 * @param scores - Array of number prediction scores
 * @returns 0x-prefixed hex keccak256 hash
 * @throws {RangeError} If any score is NaN or Infinity
 */
export function computeResultHash(scores: number[]): Hex {
  const jsonBytes = serializeF64List(scores);
  return keccak256(toHex(jsonBytes));
}

/**
 * Compute the input hash from raw JSON bytes.
 *
 * Use this when you already have the JSON-serialized feature array.
 *
 * @param jsonBytes - JSON-encoded feature array bytes
 * @returns 0x-prefixed hex keccak256 hash
 */
export function computeInputHashFromJson(jsonBytes: Uint8Array | Hex): Hex {
  const data = jsonBytes instanceof Uint8Array ? toHex(jsonBytes) : jsonBytes;
  return keccak256(data);
}

/**
 * Compute the result hash from raw result bytes.
 *
 * Use this when you already have the raw result bytes (e.g. from
 * hex-decoding the enclave's response `result` field).
 *
 * @param resultBytes - Raw result bytes
 * @returns 0x-prefixed hex keccak256 hash
 */
export function computeResultHashFromBytes(resultBytes: Uint8Array | Hex): Hex {
  const data = resultBytes instanceof Uint8Array ? toHex(resultBytes) : resultBytes;
  return keccak256(data);
}

/**
 * Format one finite f64 the way serde_json (1.0.149, through the zmij crate)
 * writes it, so the bytes hash to what the enclave hashes.
 *
 * Both sides use the shortest digit string that reads back as the same
 * number; only the layout differs from JavaScript's:
 * - leading-digit exponent from -5 to 15: plain decimal, with ".0" added to
 *   whole numbers (`1.0`, `0.00001`, `1000000000000000.0`);
 * - any other exponent: scientific with an explicit sign (`1e+16`, `2.5e-6`);
 * - negative zero keeps its sign (`-0.0`).
 */
function formatF64(v: number): string {
  if (!Number.isFinite(v)) {
    // serde_json would write `null`, which no f64 reads back as; the Rust SDK
    // refuses these values, so the SDKs agree on which inputs have a hash.
    throw new RangeError(`f64 values must be finite (no NaN or Infinity), got ${v}`);
  }
  const sign = v < 0 || Object.is(v, -0) ? '-' : '';
  const abs = Math.abs(v);
  if (abs === 0) {
    return sign + '0.0';
  }
  // toExponential() with no argument gives the shortest round-trip digits.
  const [mantissa, expPart] = abs.toExponential().split('e');
  const digits = mantissa.replace('.', '');
  const exp = Number(expPart);
  if (exp >= -5 && exp <= 15) {
    if (digits.length - 1 <= exp) {
      return sign + digits + '0'.repeat(exp + 1 - digits.length) + '.0';
    }
    if (exp >= 0) {
      return sign + digits.slice(0, exp + 1) + '.' + digits.slice(exp + 1);
    }
    return sign + '0.' + '0'.repeat(-exp - 1) + digits;
  }
  const fraction = digits.length > 1 ? '.' + digits.slice(1) : '';
  return sign + digits[0] + fraction + 'e' + (exp < 0 ? '-' : '+') + Math.abs(exp);
}

/**
 * Serialize a list of numbers to the JSON bytes Rust's
 * `serde_json::to_vec(&Vec<f64>)` produces: no spaces, and each number laid
 * out as serde_json lays it out (see {@link formatF64}).
 *
 * @param values - Array of finite numbers
 * @returns UTF-8 encoded JSON bytes
 * @throws {RangeError} If any value is NaN or Infinity
 */
export function serializeF64List(values: number[]): Uint8Array {
  const jsonStr = '[' + values.map(formatF64).join(',') + ']';
  return new TextEncoder().encode(jsonStr);
}
