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
 * `keccak256(JSON.stringify(features))` where the JSON format matches
 * Rust's `serde_json::to_vec` for `Vec<f64>`.
 *
 * @param features - Array of number feature values
 * @returns 0x-prefixed hex keccak256 hash
 */
export function computeInputHash(features: number[]): Hex {
  const jsonBytes = serializeF64List(features);
  return keccak256(toHex(jsonBytes));
}

/**
 * Compute the result hash from prediction scores.
 *
 * This matches the enclave's computation:
 * `keccak256(JSON.stringify(scores))` where the JSON format matches
 * Rust's `serde_json::to_vec` for `Vec<f64>`.
 *
 * @param scores - Array of number prediction scores
 * @returns 0x-prefixed hex keccak256 hash
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
 * Serialize a list of numbers to JSON bytes matching Rust's serde_json.
 *
 * Rust's serde_json serializes f64 values with the following rules:
 * - Integers like 1.0 are serialized as "1.0" (not "1")
 * - No spaces after commas or colons
 *
 * JavaScript's JSON.stringify serializes 1.0 as "1" (no decimal point),
 * so we must fix up integer-valued floats.
 *
 * @param values - Array of numbers
 * @returns UTF-8 encoded JSON bytes
 */
export function serializeF64List(values: number[]): Uint8Array {
  // Build a JSON string that matches Rust serde_json for Vec<f64>
  const parts = values.map((v) => {
    // JSON.stringify produces "1" for 1.0. Rust produces "1.0".
    // We need to ensure integer-valued floats include ".0"
    const s = JSON.stringify(v);
    // If the number is finite and has no decimal point or exponent notation, add .0
    if (Number.isFinite(v) && !s.includes('.') && !s.includes('e') && !s.includes('E')) {
      return s + '.0';
    }
    return s;
  });
  const jsonStr = '[' + parts.join(',') + ']';
  return new TextEncoder().encode(jsonStr);
}
