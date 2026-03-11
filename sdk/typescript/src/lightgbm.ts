/**
 * World ZK Compute SDK - LightGBM Model Converter
 *
 * Converts trained LightGBM models (dump_model() JSON format) to risc0 serde
 * binary input for the xgboost-inference guest program. Uses the same internal
 * tree representation and serialization format as the Python SDK's
 * LightGBMConverter, so circuit building works identically.
 *
 * Zero external dependencies (pure-JS SHA-256 for inputDigest).
 *
 * @example
 * ```typescript
 * import { LightGBMConverter } from '@worldzk/sdk';
 *
 * const converter = LightGBMConverter.fromJSON(modelJsonString);
 * converter
 *   .addSample('0x0000...0001', [1.0, 2.0, 3.0])
 *   .setThreshold(0.5);
 *
 * const bytes = converter.serialize();
 * const digest = converter.inputDigest();
 * const url = converter.toDataUrl();
 * ```
 */


// ============================================================================
// Types
// ============================================================================

type Hex = `0x${string}`;

/**
 * A node in a decision tree.
 *
 * Mirrors the Rust struct:
 *   TreeNode { is_leaf: u32, feature_idx: u32, threshold: f64,
 *              left_child: u32, right_child: u32, value: f64 }
 */
export interface TreeNode {
  featureIndex: number;
  threshold: number;
  leftChild: number;
  rightChild: number;
  leafValue: number;
  isLeaf: boolean;
}

/**
 * A complete LightGBM model representation.
 */
export interface LightGBMModel {
  numFeatures: number;
  numClasses: number;
  trees: TreeNode[][];
}

/**
 * A single input sample: 32-byte ID + feature vector.
 */
interface Sample {
  id: Uint8Array; // exactly 32 bytes
  features: number[];
}

// ============================================================================
// Errors
// ============================================================================

/**
 * Error thrown when model JSON cannot be parsed.
 */
export class ModelParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ModelParseError';
    Object.setPrototypeOf(this, ModelParseError.prototype);
  }
}

/**
 * Error thrown when input data fails validation.
 */
export class InputValidationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'InputValidationError';
    Object.setPrototypeOf(this, InputValidationError.prototype);
  }
}

// ============================================================================
// risc0 word-aligned serde helpers (private)
//
// risc0's serde format serializes everything as u32 little-endian words.
// ============================================================================

function writeU32(val: number): Uint8Array {
  const buf = new ArrayBuffer(4);
  new DataView(buf).setUint32(0, val >>> 0, true);
  return new Uint8Array(buf);
}

function writeF64(val: number): Uint8Array {
  // Reinterpret f64 as u64 bits, then write as two u32 words (low, high)
  const f64buf = new ArrayBuffer(8);
  new DataView(f64buf).setFloat64(0, val, true); // little-endian
  const low = new DataView(f64buf).getUint32(0, true);
  const high = new DataView(f64buf).getUint32(4, true);
  const result = new Uint8Array(8);
  new DataView(result.buffer).setUint32(0, low, true);
  new DataView(result.buffer).setUint32(4, high, true);
  return result;
}

function writeByteArray32(data: Uint8Array): Uint8Array {
  if (data.length !== 32) {
    throw new InputValidationError(
      `Expected 32 bytes, got ${data.length}`
    );
  }
  const result = new Uint8Array(32 * 4); // 32 bytes, each as u32
  for (let i = 0; i < 32; i++) {
    new DataView(result.buffer).setUint32(i * 4, data[i], true);
  }
  return result;
}

function writeVecF64(values: number[]): Uint8Array {
  const parts: Uint8Array[] = [writeU32(values.length)];
  for (const v of values) {
    parts.push(writeF64(v));
  }
  return concatBytes(parts);
}

function concatBytes(arrays: Uint8Array[]): Uint8Array {
  let totalLen = 0;
  for (const a of arrays) totalLen += a.length;
  const result = new Uint8Array(totalLen);
  let offset = 0;
  for (const a of arrays) {
    result.set(a, offset);
    offset += a.length;
  }
  return result;
}

// ============================================================================
// Hex ID parsing
// ============================================================================

function hexToBytes(hex: string): Uint8Array {
  let clean = hex;
  if (clean.startsWith('0x') || clean.startsWith('0X')) {
    clean = clean.slice(2);
  }
  if (clean.length % 2 !== 0) {
    clean = '0' + clean;
  }
  const bytes = new Uint8Array(clean.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(clean.substring(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

function parseHexId(hex: Hex): Uint8Array {
  const bytes = hexToBytes(hex);
  if (bytes.length > 32) {
    throw new InputValidationError(
      `Hex ID too long: ${bytes.length} bytes (max 32)`
    );
  }
  // Zero-pad to 32 bytes (right-padded)
  const result = new Uint8Array(32);
  result.set(bytes, 0);
  return result;
}

// ============================================================================
// LightGBM JSON parsing (private)
// ============================================================================

interface LightGBMJsonNode {
  split_index?: number;
  split_feature?: number;
  split_gain?: number;
  threshold?: number;
  decision_type?: string;
  default_left?: boolean;
  internal_value?: number;
  left_child?: LightGBMJsonNode;
  right_child?: LightGBMJsonNode;
  leaf_index?: number;
  leaf_value?: number;
}

interface LightGBMTreeInfo {
  tree_index?: number;
  num_leaves?: number;
  num_cat?: number;
  shrinkage?: number;
  tree_structure?: LightGBMJsonNode;
}

interface LightGBMJsonModel {
  name?: string;
  version?: string;
  num_class?: number;
  num_tree_per_iteration?: number;
  max_feature_idx?: number;
  objective?: string;
  tree_info?: LightGBMTreeInfo[];
}

/**
 * Internal flat node representation matching Rust's TreeNode layout.
 */
interface FlatNode {
  isLeaf: number;    // 0 = internal, 1 = leaf
  featureIdx: number;
  threshold: number;
  leftChild: number;
  rightChild: number;
  value: number;
}

function parseLightGBMNode(
  node: LightGBMJsonNode,
  nodes: FlatNode[],
  numFeatures: number,
  treeIdx: number,
): number {
  const myIdx = nodes.length;

  // Leaf node: has "leaf_index" or "leaf_value"
  if ('leaf_index' in node || 'leaf_value' in node) {
    const leafValue = Number(node.leaf_value ?? 0.0);
    nodes.push({
      isLeaf: 1,
      featureIdx: 0,
      threshold: 0.0,
      leftChild: 0,
      rightChild: 0,
      value: leafValue,
    });
    return myIdx;
  }

  // Internal (split) node
  const splitFeature = Number(node.split_feature ?? 0);
  const threshold = Number(node.threshold ?? 0.0);
  const decisionType = node.decision_type ?? '<=';

  if (decisionType !== '<=' && decisionType !== '<') {
    throw new ModelParseError(
      `Tree ${treeIdx}: unsupported decision_type '${decisionType}'. ` +
      `Only '<=' and '<' are supported (categorical splits are not supported).`
    );
  }

  if (splitFeature < 0 || splitFeature >= numFeatures) {
    throw new ModelParseError(
      `Tree ${treeIdx}: split_feature ${splitFeature} out of bounds [0, ${numFeatures})`
    );
  }

  // Reserve a slot for this node (children to be filled later)
  nodes.push({
    isLeaf: 0,
    featureIdx: splitFeature,
    threshold: threshold,
    leftChild: 0,  // placeholder
    rightChild: 0, // placeholder
    value: 0.0,
  });

  // Parse children recursively
  const leftChildNode = node.left_child;
  const rightChildNode = node.right_child;

  if (leftChildNode == null) {
    throw new ModelParseError(
      `Tree ${treeIdx}: internal node missing 'left_child'`
    );
  }
  if (rightChildNode == null) {
    throw new ModelParseError(
      `Tree ${treeIdx}: internal node missing 'right_child'`
    );
  }

  const leftIdx = parseLightGBMNode(leftChildNode, nodes, numFeatures, treeIdx);
  const rightIdx = parseLightGBMNode(rightChildNode, nodes, numFeatures, treeIdx);

  // Patch the placeholder child indices
  nodes[myIdx] = {
    isLeaf: 0,
    featureIdx: splitFeature,
    threshold: threshold,
    leftChild: leftIdx,
    rightChild: rightIdx,
    value: 0.0,
  };

  return myIdx;
}

function parseLightGBMTree(
  treeInfo: LightGBMTreeInfo,
  treeIdx: number,
  numFeatures: number,
): FlatNode[] {
  const treeStructure = treeInfo.tree_structure;
  if (treeStructure == null) {
    throw new ModelParseError(`Tree ${treeIdx}: missing 'tree_structure'`);
  }

  const nodes: FlatNode[] = [];
  parseLightGBMNode(treeStructure, nodes, numFeatures, treeIdx);

  if (nodes.length === 0) {
    throw new ModelParseError(`Tree ${treeIdx} has no nodes`);
  }

  return nodes;
}

interface InternalModel {
  numFeatures: number;
  numClasses: number;
  baseScore: number;
  trees: FlatNode[][];
}

function parseLightGBMModelJSON(data: LightGBMJsonModel): InternalModel {
  // Extract num_features
  const maxFeatureIdx = data.max_feature_idx;
  if (maxFeatureIdx == null) {
    throw new ModelParseError(
      "Invalid LightGBM JSON: missing 'max_feature_idx'"
    );
  }
  const numFeatures = Number(maxFeatureIdx) + 1;
  if (numFeatures <= 0 || !Number.isFinite(numFeatures)) {
    throw new ModelParseError(
      `num_features must be positive, got ${numFeatures}`
    );
  }

  // Extract num_classes
  const lgbNumClass = Number(data.num_class ?? 1);
  // LightGBM conventions:
  // - Binary classification: num_class=1 -> map to 2
  // - Regression: num_class=1 -> map to 2
  // - Multiclass: num_class=N (N>=2) -> keep as-is
  const numClasses = lgbNumClass <= 1 ? 2 : lgbNumClass;

  // Parse trees
  const treeInfoList = data.tree_info ?? [];
  if (treeInfoList.length === 0) {
    throw new ModelParseError('Model contains no trees');
  }

  const trees: FlatNode[][] = [];
  for (let ti = 0; ti < treeInfoList.length; ti++) {
    try {
      trees.push(parseLightGBMTree(treeInfoList[ti], ti, numFeatures));
    } catch (e) {
      if (e instanceof ModelParseError) throw e;
      throw new ModelParseError(`Error parsing tree ${ti}: ${e}`);
    }
  }

  return {
    numFeatures,
    numClasses,
    baseScore: 0.0, // LightGBM uses init_score per sample, not global
    trees,
  };
}

// ============================================================================
// Convert between internal and public model types
// ============================================================================

function flatNodeToTreeNode(node: FlatNode): TreeNode {
  return {
    featureIndex: node.featureIdx,
    threshold: node.threshold,
    leftChild: node.leftChild,
    rightChild: node.rightChild,
    leafValue: node.value,
    isLeaf: node.isLeaf === 1,
  };
}

function treeNodeToFlatNode(node: TreeNode): FlatNode {
  return {
    isLeaf: node.isLeaf ? 1 : 0,
    featureIdx: node.featureIndex,
    threshold: node.threshold,
    leftChild: node.leftChild,
    rightChild: node.rightChild,
    value: node.leafValue,
  };
}

function internalToPublicModel(internal: InternalModel): LightGBMModel {
  return {
    numFeatures: internal.numFeatures,
    numClasses: internal.numClasses,
    trees: internal.trees.map(tree => tree.map(flatNodeToTreeNode)),
  };
}

function publicToInternalModel(pub: LightGBMModel): InternalModel {
  return {
    numFeatures: pub.numFeatures,
    numClasses: pub.numClasses,
    baseScore: 0.0,
    trees: pub.trees.map(tree => tree.map(treeNodeToFlatNode)),
  };
}

// ============================================================================
// Validation helpers
// ============================================================================

function validateModel(model: InternalModel): void {
  if (model.numFeatures <= 0) {
    throw new InputValidationError(
      `num_features must be positive, got ${model.numFeatures}`
    );
  }
  if (model.trees.length === 0) {
    throw new InputValidationError('Model must have at least one tree');
  }
  for (let ti = 0; ti < model.trees.length; ti++) {
    const tree = model.trees[ti];
    if (tree.length === 0) {
      throw new InputValidationError(`Tree ${ti} has no nodes`);
    }
    for (let ni = 0; ni < tree.length; ni++) {
      const node = tree[ni];
      if (node.isLeaf === 0) {
        if (node.leftChild < 0 || node.leftChild >= tree.length) {
          throw new InputValidationError(
            `Tree ${ti} node ${ni}: left_child ${node.leftChild} ` +
            `out of bounds [0, ${tree.length})`
          );
        }
        if (node.rightChild < 0 || node.rightChild >= tree.length) {
          throw new InputValidationError(
            `Tree ${ti} node ${ni}: right_child ${node.rightChild} ` +
            `out of bounds [0, ${tree.length})`
          );
        }
        if (node.featureIdx < 0 || node.featureIdx >= model.numFeatures) {
          throw new InputValidationError(
            `Tree ${ti} node ${ni}: feature_idx ${node.featureIdx} ` +
            `out of bounds [0, ${model.numFeatures})`
          );
        }
      }
    }
  }
}

function validateSample(
  id: Uint8Array,
  features: number[],
  numFeatures: number,
): void {
  if (id.length !== 32) {
    throw new InputValidationError(
      `Sample ID must be exactly 32 bytes, got ${id.length}`
    );
  }
  if (features.length !== numFeatures) {
    throw new InputValidationError(
      `Expected ${numFeatures} features, got ${features.length}`
    );
  }
  for (let i = 0; i < features.length; i++) {
    if (!Number.isFinite(features[i])) {
      throw new InputValidationError(
        `Feature ${i} is ${features[i]}; NaN and Inf are not allowed`
      );
    }
  }
}

// ============================================================================
// SHA-256 using Web Crypto (sync fallback via manual implementation)
// ============================================================================

/**
 * Synchronous SHA-256 implementation.
 *
 * We need a sync sha256 for inputDigest(). Since WebCrypto is async-only,
 * we use a pure-JS implementation.
 */
function sha256(data: Uint8Array): Uint8Array {
  // Constants
  const K: number[] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
  ];

  function rotr(x: number, n: number): number {
    return ((x >>> n) | (x << (32 - n))) >>> 0;
  }

  function ch(x: number, y: number, z: number): number {
    return ((x & y) ^ (~x & z)) >>> 0;
  }

  function maj(x: number, y: number, z: number): number {
    return ((x & y) ^ (x & z) ^ (y & z)) >>> 0;
  }

  function sigma0(x: number): number {
    return (rotr(x, 2) ^ rotr(x, 13) ^ rotr(x, 22)) >>> 0;
  }

  function sigma1(x: number): number {
    return (rotr(x, 6) ^ rotr(x, 11) ^ rotr(x, 25)) >>> 0;
  }

  function gamma0(x: number): number {
    return (rotr(x, 7) ^ rotr(x, 18) ^ (x >>> 3)) >>> 0;
  }

  function gamma1(x: number): number {
    return (rotr(x, 17) ^ rotr(x, 19) ^ (x >>> 10)) >>> 0;
  }

  // Pre-processing: padding
  const msgLen = data.length;
  const bitLen = msgLen * 8;
  // Message + 1 byte (0x80) + padding + 8 bytes length
  const padLen = (((msgLen + 9 + 63) >>> 6) << 6);
  const padded = new Uint8Array(padLen);
  padded.set(data);
  padded[msgLen] = 0x80;
  // Write bit length as big-endian 64-bit
  const view = new DataView(padded.buffer);
  // For messages < 2^32 bits, high word is 0
  view.setUint32(padLen - 8, 0, false);
  view.setUint32(padLen - 4, bitLen, false);

  // Initial hash values
  let h0 = 0x6a09e667;
  let h1 = 0xbb67ae85;
  let h2 = 0x3c6ef372;
  let h3 = 0xa54ff53a;
  let h4 = 0x510e527f;
  let h5 = 0x9b05688c;
  let h6 = 0x1f83d9ab;
  let h7 = 0x5be0cd19;

  const W = new Array<number>(64);

  for (let offset = 0; offset < padLen; offset += 64) {
    // Prepare message schedule
    for (let i = 0; i < 16; i++) {
      W[i] = view.getUint32(offset + i * 4, false);
    }
    for (let i = 16; i < 64; i++) {
      W[i] = (gamma1(W[i - 2]) + W[i - 7] + gamma0(W[i - 15]) + W[i - 16]) >>> 0;
    }

    let a = h0, b = h1, c = h2, d = h3;
    let e = h4, f = h5, g = h6, h = h7;

    for (let i = 0; i < 64; i++) {
      const t1 = (h + sigma1(e) + ch(e, f, g) + K[i] + W[i]) >>> 0;
      const t2 = (sigma0(a) + maj(a, b, c)) >>> 0;
      h = g;
      g = f;
      f = e;
      e = (d + t1) >>> 0;
      d = c;
      c = b;
      b = a;
      a = (t1 + t2) >>> 0;
    }

    h0 = (h0 + a) >>> 0;
    h1 = (h1 + b) >>> 0;
    h2 = (h2 + c) >>> 0;
    h3 = (h3 + d) >>> 0;
    h4 = (h4 + e) >>> 0;
    h5 = (h5 + f) >>> 0;
    h6 = (h6 + g) >>> 0;
    h7 = (h7 + h) >>> 0;
  }

  const hash = new Uint8Array(32);
  const hv = new DataView(hash.buffer);
  hv.setUint32(0, h0, false);
  hv.setUint32(4, h1, false);
  hv.setUint32(8, h2, false);
  hv.setUint32(12, h3, false);
  hv.setUint32(16, h4, false);
  hv.setUint32(20, h5, false);
  hv.setUint32(24, h6, false);
  hv.setUint32(28, h7, false);

  return hash;
}

function bytesToHex(bytes: Uint8Array): Hex {
  let hex = '0x';
  for (let i = 0; i < bytes.length; i++) {
    hex += bytes[i].toString(16).padStart(2, '0');
  }
  return hex as Hex;
}

// ============================================================================
// LightGBMConverter -- public API
// ============================================================================

/**
 * Converts LightGBM models and samples to risc0 serde binary input.
 *
 * Uses the same internal representation and serialization format as
 * the Python SDK's XGBoostConverter/LightGBMConverter, so circuit building
 * works identically.
 *
 * @example
 * ```typescript
 * const converter = LightGBMConverter.fromJSON(modelJsonString);
 * converter
 *   .addSample('0x0000...0001', [1.0, 2.0, 3.0])
 *   .setThreshold(0.5);
 *
 * const bytes = converter.serialize();
 * const digest = converter.inputDigest();
 * ```
 */
export class LightGBMConverter {
  private _model: InternalModel;
  private _publicModel: LightGBMModel;
  private _samples: Sample[] = [];
  private _threshold = 0.5;

  private constructor(internal: InternalModel) {
    this._model = internal;
    this._publicModel = internalToPublicModel(internal);
  }

  /**
   * Create a LightGBMConverter from a LightGBM dump_model() JSON string.
   *
   * @param json - The JSON string from `model.dump_model()` or `model.save_model("model.json")`
   * @returns A new LightGBMConverter instance
   * @throws {ModelParseError} If the JSON is invalid or the model structure is unsupported
   */
  static fromJSON(json: string): LightGBMConverter {
    let data: LightGBMJsonModel;
    try {
      data = JSON.parse(json) as LightGBMJsonModel;
    } catch (e) {
      throw new ModelParseError(`Invalid JSON: ${e}`);
    }
    const internal = parseLightGBMModelJSON(data);
    return new LightGBMConverter(internal);
  }

  /**
   * Create a LightGBMConverter from a pre-built LightGBMModel.
   *
   * @param model - A LightGBMModel object with numFeatures, numClasses, and trees
   * @returns A new LightGBMConverter instance
   */
  static fromModel(model: LightGBMModel): LightGBMConverter {
    const internal = publicToInternalModel(model);
    return new LightGBMConverter(internal);
  }

  /**
   * Get the parsed model.
   */
  get model(): LightGBMModel {
    return this._publicModel;
  }

  /**
   * Add a single sample. Returns `this` for chaining.
   *
   * @param id - A `0x`-prefixed hex string identifying the sample (up to 32 bytes, zero-padded)
   * @param features - Feature values; count must match model.numFeatures
   * @returns this
   * @throws {InputValidationError} If the ID or features are invalid
   */
  addSample(id: Hex, features: number[]): this {
    const idBytes = parseHexId(id);
    validateSample(idBytes, features, this._model.numFeatures);
    this._samples.push({ id: idBytes, features: [...features] });
    return this;
  }

  /**
   * Set the classification threshold. Returns `this` for chaining.
   *
   * @param threshold - The threshold value (must be finite)
   * @returns this
   * @throws {InputValidationError} If the threshold is NaN or Infinity
   */
  setThreshold(threshold: number): this {
    if (!Number.isFinite(threshold)) {
      throw new InputValidationError('Threshold must be a finite number');
    }
    this._threshold = threshold;
    return this;
  }

  /**
   * Serialize the full input to risc0 serde binary format.
   *
   * Layout (matching Rust XGBoostInput):
   *   model: XGBoostModel { num_features, num_classes, base_score, trees }
   *   samples: Vec<Sample>
   *   threshold: f64
   *
   * @returns The serialized binary data
   * @throws {InputValidationError} If the model or samples are invalid
   */
  serialize(): Uint8Array {
    this._validateAll();

    const parts: Uint8Array[] = [];

    // model: XGBoostModel
    parts.push(writeU32(this._model.numFeatures));
    parts.push(writeU32(this._model.numClasses));
    parts.push(writeF64(this._model.baseScore));

    // trees: Vec<Tree>
    parts.push(writeU32(this._model.trees.length));
    for (const tree of this._model.trees) {
      // nodes: Vec<TreeNode>
      parts.push(writeU32(tree.length));
      for (const node of tree) {
        parts.push(writeU32(node.isLeaf));
        parts.push(writeU32(node.featureIdx));
        parts.push(writeF64(node.threshold));
        parts.push(writeU32(node.leftChild));
        parts.push(writeU32(node.rightChild));
        parts.push(writeF64(node.value));
      }
    }

    // samples: Vec<Sample>
    parts.push(writeU32(this._samples.length));
    for (const sample of this._samples) {
      parts.push(writeByteArray32(sample.id));
      parts.push(writeVecF64(sample.features));
    }

    // threshold: f64
    parts.push(writeF64(this._threshold));

    return concatBytes(parts);
  }

  /**
   * Return the SHA-256 hex digest of the serialized input (0x-prefixed).
   *
   * @returns 0x-prefixed 66-character hex string
   */
  inputDigest(): Hex {
    const raw = this.serialize();
    const hash = sha256(raw);
    return bytesToHex(hash);
  }

  /**
   * Serialize and return as a data URL (base64-encoded).
   *
   * @returns A `data:application/octet-stream;base64,...` URL
   */
  toDataUrl(): string {
    const raw = this.serialize();
    // Use btoa for base64 encoding (available in browsers and Node 16+)
    let binary = '';
    for (let i = 0; i < raw.length; i++) {
      binary += String.fromCharCode(raw[i]);
    }
    const encoded = btoa(binary);
    return `data:application/octet-stream;base64,${encoded}`;
  }

  private _validateAll(): void {
    validateModel(this._model);
    if (this._samples.length === 0) {
      throw new InputValidationError('At least one sample is required');
    }
  }
}
