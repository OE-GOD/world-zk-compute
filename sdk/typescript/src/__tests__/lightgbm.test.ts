import { describe, it, expect } from 'vitest';
import {
  LightGBMConverter,
  ModelParseError,
  InputValidationError,
  type TreeNode,
  type LightGBMModel,
} from '../lightgbm';

// ============================================================================
// Inline sample LightGBM models (matching Python test fixtures)
// ============================================================================

/**
 * A minimal binary classification LightGBM model with 2 trees, 4 features.
 *
 * Tree 0 (5 nodes):
 *     split on feature 2, threshold 0.5
 *       left: leaf 0.1
 *       right: split on feature 0, threshold 1.5
 *         left: leaf -0.2
 *         right: leaf 0.3
 *
 * Tree 1 (3 nodes):
 *     split on feature 1, threshold 3.0
 *       left: leaf -0.15
 *       right: leaf 0.25
 */
function sampleBinaryModel(): object {
  return {
    name: 'tree',
    version: 'v3.3.5',
    num_class: 1,
    num_tree_per_iteration: 1,
    label_index: 0,
    max_feature_idx: 3,
    objective: 'binary sigmoid:1',
    tree_info: [
      {
        tree_index: 0,
        num_leaves: 3,
        num_cat: 0,
        shrinkage: 1,
        tree_structure: {
          split_index: 0,
          split_feature: 2,
          split_gain: 100.5,
          threshold: 0.5,
          decision_type: '<=',
          default_left: true,
          internal_value: 0,
          left_child: {
            leaf_index: 0,
            leaf_value: 0.1,
          },
          right_child: {
            split_index: 1,
            split_feature: 0,
            threshold: 1.5,
            decision_type: '<=',
            default_left: true,
            internal_value: 0,
            left_child: {
              leaf_index: 1,
              leaf_value: -0.2,
            },
            right_child: {
              leaf_index: 2,
              leaf_value: 0.3,
            },
          },
        },
      },
      {
        tree_index: 1,
        num_leaves: 2,
        num_cat: 0,
        shrinkage: 1,
        tree_structure: {
          split_index: 0,
          split_feature: 1,
          split_gain: 50.0,
          threshold: 3.0,
          decision_type: '<=',
          default_left: true,
          internal_value: 0,
          left_child: {
            leaf_index: 0,
            leaf_value: -0.15,
          },
          right_child: {
            leaf_index: 1,
            leaf_value: 0.25,
          },
        },
      },
    ],
  };
}

/**
 * A multiclass LightGBM model with 3 classes, 2 features, 6 trees.
 * num_tree_per_iteration=3 means each boosting round produces 3 trees
 * (one per class). We have 2 rounds -> 6 trees total.
 */
function sampleMulticlassModel(): object {
  function makeSimpleTree(feature: number, threshold: number, leftVal: number, rightVal: number) {
    return {
      tree_structure: {
        split_index: 0,
        split_feature: feature,
        threshold: threshold,
        decision_type: '<=',
        left_child: { leaf_index: 0, leaf_value: leftVal },
        right_child: { leaf_index: 1, leaf_value: rightVal },
      },
    };
  }

  return {
    name: 'tree',
    version: 'v3.3.5',
    num_class: 3,
    num_tree_per_iteration: 3,
    max_feature_idx: 1,
    objective: 'multiclass softmax num_class:3',
    tree_info: [
      // Round 1: trees for class 0, 1, 2
      makeSimpleTree(0, 1.0, 0.1, -0.1),
      makeSimpleTree(1, 2.0, -0.2, 0.2),
      makeSimpleTree(0, 0.5, 0.3, -0.3),
      // Round 2: trees for class 0, 1, 2
      makeSimpleTree(1, 1.5, 0.05, -0.05),
      makeSimpleTree(0, 0.8, -0.1, 0.1),
      makeSimpleTree(1, 2.5, 0.15, -0.15),
    ],
  };
}

/**
 * A model where one tree is just a single leaf (stump-like).
 */
function sampleSingleLeafModel(): object {
  return {
    name: 'tree',
    version: 'v3.3.5',
    num_class: 1,
    num_tree_per_iteration: 1,
    max_feature_idx: 1,
    objective: 'regression',
    tree_info: [
      {
        tree_structure: {
          leaf_index: 0,
          leaf_value: 0.42,
        },
      },
    ],
  };
}

/**
 * A model with a deeper tree (depth 3).
 */
function sampleDeepTreeModel(): object {
  return {
    name: 'tree',
    version: 'v3.3.5',
    num_class: 1,
    num_tree_per_iteration: 1,
    max_feature_idx: 2,
    objective: 'binary sigmoid:1',
    tree_info: [
      {
        tree_structure: {
          split_index: 0,
          split_feature: 0,
          threshold: 5.0,
          decision_type: '<=',
          left_child: {
            split_index: 1,
            split_feature: 1,
            threshold: 2.0,
            decision_type: '<=',
            left_child: {
              leaf_index: 0,
              leaf_value: -0.5,
            },
            right_child: {
              split_index: 3,
              split_feature: 2,
              threshold: 1.0,
              decision_type: '<=',
              left_child: {
                leaf_index: 1,
                leaf_value: 0.1,
              },
              right_child: {
                leaf_index: 2,
                leaf_value: 0.2,
              },
            },
          },
          right_child: {
            split_index: 2,
            split_feature: 1,
            threshold: 3.0,
            decision_type: '<=',
            left_child: {
              leaf_index: 3,
              leaf_value: 0.3,
            },
            right_child: {
              leaf_index: 4,
              leaf_value: 0.8,
            },
          },
        },
      },
    ],
  };
}

// Helper: traverse a flat tree array to get the leaf value for given features
function traverseTree(nodes: TreeNode[], features: number[]): number {
  let node = nodes[0];
  while (!node.isLeaf) {
    if (features[node.featureIndex] <= node.threshold) {
      node = nodes[node.leftChild];
    } else {
      node = nodes[node.rightChild];
    }
  }
  return node.leafValue;
}

// ============================================================================
// Test: JSON parsing -- binary classification
// ============================================================================

describe('LightGBMConverter - Binary classification parsing', () => {
  it('should parse model metadata correctly', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleBinaryModel()));
    const model = converter.model;

    expect(model.numFeatures).toBe(4);  // max_feature_idx=3 -> 4 features
    expect(model.numClasses).toBe(2);   // num_class=1 -> 2 (binary)
    expect(model.trees.length).toBe(2);
  });

  it('should parse tree 0 structure correctly', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleBinaryModel()));
    const tree0 = converter.model.trees[0];

    // Tree 0: root + left_leaf + right_internal + 2 right_leaves = 5 nodes
    expect(tree0.length).toBe(5);

    // Root: splits on feature 2 at threshold 0.5
    const root = tree0[0];
    expect(root.isLeaf).toBe(false);
    expect(root.featureIndex).toBe(2);
    expect(root.threshold).toBeCloseTo(0.5);

    // Left child is a leaf with value 0.1
    const left = tree0[root.leftChild];
    expect(left.isLeaf).toBe(true);
    expect(left.leafValue).toBeCloseTo(0.1);

    // Right child is an internal node
    const right = tree0[root.rightChild];
    expect(right.isLeaf).toBe(false);
    expect(right.featureIndex).toBe(0);
    expect(right.threshold).toBeCloseTo(1.5);

    // Right's children
    const rightLeft = tree0[right.leftChild];
    expect(rightLeft.isLeaf).toBe(true);
    expect(rightLeft.leafValue).toBeCloseTo(-0.2);

    const rightRight = tree0[right.rightChild];
    expect(rightRight.isLeaf).toBe(true);
    expect(rightRight.leafValue).toBeCloseTo(0.3);
  });

  it('should parse tree 1 structure correctly', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleBinaryModel()));
    const tree1 = converter.model.trees[1];

    expect(tree1.length).toBe(3);

    const root = tree1[0];
    expect(root.isLeaf).toBe(false);
    expect(root.featureIndex).toBe(1);
    expect(root.threshold).toBeCloseTo(3.0);

    const left = tree1[root.leftChild];
    expect(left.isLeaf).toBe(true);
    expect(left.leafValue).toBeCloseTo(-0.15);

    const right = tree1[root.rightChild];
    expect(right.isLeaf).toBe(true);
    expect(right.leafValue).toBeCloseTo(0.25);
  });

  it('should simulate tree inference correctly (left paths)', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleBinaryModel()));
    const model = converter.model;

    // features: [f0=0, f1=0, f2=0, f3=0]
    // Tree 0: f2=0 <= 0.5 -> left -> leaf 0.1
    // Tree 1: f1=0 <= 3.0 -> left -> leaf -0.15
    const features = [0.0, 0.0, 0.0, 0.0];
    const tree0Val = traverseTree(model.trees[0], features);
    const tree1Val = traverseTree(model.trees[1], features);
    expect(tree0Val).toBeCloseTo(0.1);
    expect(tree1Val).toBeCloseTo(-0.15);
    expect(tree0Val + tree1Val).toBeCloseTo(-0.05);
  });

  it('should simulate tree inference correctly (right paths)', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleBinaryModel()));
    const model = converter.model;

    // features: [f0=2, f1=4, f2=1, f3=0]
    // Tree 0: f2=1 > 0.5 -> right -> split(f0, 1.5) -> f0=2 > 1.5 -> right -> leaf 0.3
    // Tree 1: f1=4 > 3.0 -> right -> leaf 0.25
    const features = [2.0, 4.0, 1.0, 0.0];
    const tree0Val = traverseTree(model.trees[0], features);
    const tree1Val = traverseTree(model.trees[1], features);
    expect(tree0Val).toBeCloseTo(0.3);
    expect(tree1Val).toBeCloseTo(0.25);
    expect(tree0Val + tree1Val).toBeCloseTo(0.55);
  });
});

// ============================================================================
// Test: Multiclass model
// ============================================================================

describe('LightGBMConverter - Multiclass model', () => {
  it('should parse multiclass model metadata', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleMulticlassModel()));
    const model = converter.model;

    expect(model.numFeatures).toBe(2);
    expect(model.numClasses).toBe(3);
    expect(model.trees.length).toBe(6);
  });

  it('should parse all trees as 3-node trees (1 split + 2 leaves)', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleMulticlassModel()));
    const model = converter.model;

    for (let i = 0; i < model.trees.length; i++) {
      const tree = model.trees[i];
      expect(tree.length).toBe(3);
      expect(tree[0].isLeaf).toBe(false);
      expect(tree[1].isLeaf).toBe(true);
      expect(tree[2].isLeaf).toBe(true);
    }
  });

  it('should serialize multiclass model successfully', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleMulticlassModel()));
    converter.addSample('0x' + '00'.repeat(32) as `0x${string}`, [0.5, 1.5]);

    const result = converter.serialize();
    expect(result).toBeInstanceOf(Uint8Array);
    expect(result.length).toBeGreaterThan(0);
  });
});

// ============================================================================
// Test: Single-leaf tree
// ============================================================================

describe('LightGBMConverter - Single-leaf tree', () => {
  it('should parse a single-leaf tree', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleSingleLeafModel()));
    const model = converter.model;

    expect(model.numFeatures).toBe(2);
    expect(model.numClasses).toBe(2); // regression -> mapped to 2
    expect(model.trees.length).toBe(1);
    expect(model.trees[0].length).toBe(1);
    expect(model.trees[0][0].isLeaf).toBe(true);
    expect(model.trees[0][0].leafValue).toBeCloseTo(0.42);
  });

  it('should serialize single-leaf tree model', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleSingleLeafModel()));
    converter.addSample('0x' + '00'.repeat(32) as `0x${string}`, [1.0, 2.0]);

    const result = converter.serialize();
    expect(result).toBeInstanceOf(Uint8Array);
    expect(result.length).toBeGreaterThan(0);
  });
});

// ============================================================================
// Test: Deep tree
// ============================================================================

describe('LightGBMConverter - Deep tree', () => {
  it('should parse a depth-3 tree with 9 nodes', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleDeepTreeModel()));
    const tree = converter.model.trees[0];

    // 4 split nodes + 5 leaf nodes = 9 total
    expect(tree.length).toBe(9);

    // Verify root
    expect(tree[0].isLeaf).toBe(false);
    expect(tree[0].featureIndex).toBe(0);
    expect(tree[0].threshold).toBeCloseTo(5.0);
  });

  it('should traverse all paths correctly', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleDeepTreeModel()));
    const tree = converter.model.trees[0];

    // f0=3 <= 5 -> left, f1=1 <= 2 -> left -> leaf -0.5
    expect(traverseTree(tree, [3.0, 1.0, 0.5])).toBeCloseTo(-0.5);

    // f0=3 <= 5 -> left, f1=3 > 2 -> right, f2=0.5 <= 1 -> left -> leaf 0.1
    expect(traverseTree(tree, [3.0, 3.0, 0.5])).toBeCloseTo(0.1);

    // f0=3 <= 5 -> left, f1=3 > 2 -> right, f2=2 > 1 -> right -> leaf 0.2
    expect(traverseTree(tree, [3.0, 3.0, 2.0])).toBeCloseTo(0.2);

    // f0=6 > 5 -> right, f1=2 <= 3 -> left -> leaf 0.3
    expect(traverseTree(tree, [6.0, 2.0, 0.0])).toBeCloseTo(0.3);

    // f0=6 > 5 -> right, f1=4 > 3 -> right -> leaf 0.8
    expect(traverseTree(tree, [6.0, 4.0, 0.0])).toBeCloseTo(0.8);
  });
});

// ============================================================================
// Test: Serialization determinism
// ============================================================================

describe('LightGBMConverter - Serialization determinism', () => {
  it('should produce identical bytes for identical inputs', () => {
    const json = JSON.stringify(sampleBinaryModel());
    const zeroId = ('0x' + '00'.repeat(32)) as `0x${string}`;

    const conv1 = LightGBMConverter.fromJSON(json);
    conv1.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]);

    const conv2 = LightGBMConverter.fromJSON(json);
    conv2.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]);

    const bytes1 = conv1.serialize();
    const bytes2 = conv2.serialize();

    expect(bytes1).toEqual(bytes2);
  });

  it('should produce identical digests for identical inputs', () => {
    const json = JSON.stringify(sampleBinaryModel());
    const zeroId = ('0x' + '00'.repeat(32)) as `0x${string}`;

    const conv1 = LightGBMConverter.fromJSON(json);
    conv1.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]);

    const conv2 = LightGBMConverter.fromJSON(json);
    conv2.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]);

    expect(conv1.inputDigest()).toBe(conv2.inputDigest());
  });

  it('should produce different bytes for different features', () => {
    const json = JSON.stringify(sampleBinaryModel());
    const zeroId = ('0x' + '00'.repeat(32)) as `0x${string}`;

    const conv1 = LightGBMConverter.fromJSON(json);
    conv1.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]);

    const conv2 = LightGBMConverter.fromJSON(json);
    conv2.addSample(zeroId, [5.0, 6.0, 7.0, 8.0]);

    expect(conv1.serialize()).not.toEqual(conv2.serialize());
    expect(conv1.inputDigest()).not.toBe(conv2.inputDigest());
  });

  it('should produce different bytes for different thresholds', () => {
    const json = JSON.stringify(sampleBinaryModel());
    const zeroId = ('0x' + '00'.repeat(32)) as `0x${string}`;

    const conv1 = LightGBMConverter.fromJSON(json);
    conv1.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]).setThreshold(0.3);

    const conv2 = LightGBMConverter.fromJSON(json);
    conv2.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]).setThreshold(0.7);

    expect(conv1.serialize()).not.toEqual(conv2.serialize());
  });
});

// ============================================================================
// Test: API chaining
// ============================================================================

describe('LightGBMConverter - API chaining', () => {
  it('should return this from addSample', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleBinaryModel()));
    const zeroId = ('0x' + '00'.repeat(32)) as `0x${string}`;
    const result = converter.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]);
    expect(result).toBe(converter);
  });

  it('should return this from setThreshold', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleBinaryModel()));
    const result = converter.setThreshold(0.7);
    expect(result).toBe(converter);
  });

  it('should support full method chaining', () => {
    const json = JSON.stringify(sampleBinaryModel());
    const zeroId = ('0x' + '00'.repeat(32)) as `0x${string}`;
    const oneId = ('0x' + '01' + '00'.repeat(31)) as `0x${string}`;

    const result = LightGBMConverter.fromJSON(json)
      .setThreshold(0.3)
      .addSample(zeroId, [1.0, 2.0, 3.0, 4.0])
      .addSample(oneId, [5.0, 6.0, 7.0, 8.0]);

    expect(result).toBeInstanceOf(LightGBMConverter);
    const bytes = result.serialize();
    expect(bytes).toBeInstanceOf(Uint8Array);
    expect(bytes.length).toBeGreaterThan(0);
  });

  it('should allow multiple samples', () => {
    const json = JSON.stringify(sampleBinaryModel());
    const converter = LightGBMConverter.fromJSON(json);
    const zeroId = ('0x' + '00'.repeat(32)) as `0x${string}`;

    converter
      .addSample(zeroId, [1.0, 2.0, 3.0, 4.0])
      .addSample(zeroId, [5.0, 6.0, 7.0, 8.0])
      .addSample(zeroId, [9.0, 10.0, 11.0, 12.0]);

    const bytes = converter.serialize();
    expect(bytes.length).toBeGreaterThan(0);
  });
});

// ============================================================================
// Test: fromModel
// ============================================================================

describe('LightGBMConverter - fromModel', () => {
  it('should create converter from a LightGBMModel', () => {
    const model: LightGBMModel = {
      numFeatures: 2,
      numClasses: 2,
      trees: [
        [
          {
            featureIndex: 0,
            threshold: 1.0,
            leftChild: 1,
            rightChild: 2,
            leafValue: 0,
            isLeaf: false,
          },
          {
            featureIndex: 0,
            threshold: 0,
            leftChild: 0,
            rightChild: 0,
            leafValue: -0.3,
            isLeaf: true,
          },
          {
            featureIndex: 0,
            threshold: 0,
            leftChild: 0,
            rightChild: 0,
            leafValue: 0.7,
            isLeaf: true,
          },
        ],
      ],
    };

    const converter = LightGBMConverter.fromModel(model);
    expect(converter.model.numFeatures).toBe(2);
    expect(converter.model.numClasses).toBe(2);
    expect(converter.model.trees.length).toBe(1);
    expect(converter.model.trees[0].length).toBe(3);
  });

  it('should serialize correctly when built from LightGBMModel', () => {
    const model: LightGBMModel = {
      numFeatures: 2,
      numClasses: 2,
      trees: [
        [
          {
            featureIndex: 0,
            threshold: 0,
            leftChild: 0,
            rightChild: 0,
            leafValue: 0.5,
            isLeaf: true,
          },
        ],
      ],
    };

    const converter = LightGBMConverter.fromModel(model);
    converter.addSample('0x' + '00'.repeat(32) as `0x${string}`, [1.0, 2.0]);

    const bytes = converter.serialize();
    expect(bytes).toBeInstanceOf(Uint8Array);
    expect(bytes.length).toBeGreaterThan(0);
  });
});

// ============================================================================
// Test: Error cases -- invalid JSON
// ============================================================================

describe('LightGBMConverter - Error cases', () => {
  it('should throw on invalid JSON string', () => {
    expect(() => LightGBMConverter.fromJSON('not valid json')).toThrow(ModelParseError);
  });

  it('should throw on empty JSON object', () => {
    expect(() => LightGBMConverter.fromJSON('{}')).toThrow(ModelParseError);
    expect(() => LightGBMConverter.fromJSON('{}')).toThrow(/max_feature_idx/);
  });

  it('should throw when max_feature_idx is missing', () => {
    const data = { num_class: 1, tree_info: [{ tree_structure: { leaf_index: 0, leaf_value: 0.1 } }] };
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(ModelParseError);
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(/max_feature_idx/);
  });

  it('should throw when tree_info is empty', () => {
    const data = { max_feature_idx: 3, num_class: 1, tree_info: [] };
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(ModelParseError);
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(/no trees/);
  });

  it('should throw when tree_structure is missing', () => {
    const data = { max_feature_idx: 3, num_class: 1, tree_info: [{}] };
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(ModelParseError);
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(/tree_structure/);
  });

  it('should throw on out-of-range split_feature', () => {
    const data = {
      num_class: 1,
      max_feature_idx: 1,
      tree_info: [{
        tree_structure: {
          split_index: 0,
          split_feature: 99,
          threshold: 1.0,
          decision_type: '<=',
          left_child: { leaf_index: 0, leaf_value: 0.1 },
          right_child: { leaf_index: 1, leaf_value: 0.2 },
        },
      }],
    };
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(ModelParseError);
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(/split_feature 99 out of bounds/);
  });

  it('should throw on unsupported decision_type', () => {
    const data = {
      num_class: 1,
      max_feature_idx: 0,
      tree_info: [{
        tree_structure: {
          split_index: 0,
          split_feature: 0,
          threshold: 1.0,
          decision_type: '==',
          left_child: { leaf_index: 0, leaf_value: 0.1 },
          right_child: { leaf_index: 1, leaf_value: 0.2 },
        },
      }],
    };
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(ModelParseError);
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(/unsupported decision_type/);
  });

  it('should throw on missing left_child in internal node', () => {
    const data = {
      num_class: 1,
      max_feature_idx: 1,
      tree_info: [{
        tree_structure: {
          split_index: 0,
          split_feature: 0,
          threshold: 1.0,
          decision_type: '<=',
          right_child: { leaf_index: 1, leaf_value: 0.2 },
        },
      }],
    };
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(ModelParseError);
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(/missing 'left_child'/);
  });

  it('should throw on negative max_feature_idx', () => {
    const data = {
      num_class: 1,
      max_feature_idx: -1,
      tree_info: [{ tree_structure: { leaf_index: 0, leaf_value: 0.1 } }],
    };
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(ModelParseError);
    expect(() => LightGBMConverter.fromJSON(JSON.stringify(data))).toThrow(/num_features must be positive/);
  });

  it('should accept decision_type "<"', () => {
    const data = {
      num_class: 1,
      max_feature_idx: 0,
      tree_info: [{
        tree_structure: {
          split_index: 0,
          split_feature: 0,
          threshold: 1.0,
          decision_type: '<',
          left_child: { leaf_index: 0, leaf_value: 0.1 },
          right_child: { leaf_index: 1, leaf_value: 0.2 },
        },
      }],
    };
    const converter = LightGBMConverter.fromJSON(JSON.stringify(data));
    expect(converter.model.trees[0][0].threshold).toBeCloseTo(1.0);
  });
});

// ============================================================================
// Test: Input validation errors
// ============================================================================

describe('LightGBMConverter - Input validation', () => {
  const zeroId = ('0x' + '00'.repeat(32)) as `0x${string}`;

  function makeConverter(): LightGBMConverter {
    return LightGBMConverter.fromJSON(JSON.stringify(sampleBinaryModel()));
  }

  it('should throw on wrong feature count', () => {
    const conv = makeConverter();
    expect(() => conv.addSample(zeroId, [1.0, 2.0])).toThrow(InputValidationError);
    expect(() => conv.addSample(zeroId, [1.0, 2.0])).toThrow(/Expected 4 features, got 2/);
  });

  it('should throw on short hex ID', () => {
    const conv = makeConverter();
    // A short hex ID is accepted (zero-padded), but we test that 32-byte parsing works
    const shortId = '0x01' as `0x${string}`;
    // This should NOT throw -- short hex IDs are zero-padded to 32 bytes
    conv.addSample(shortId, [1.0, 2.0, 3.0, 4.0]);
  });

  it('should throw on hex ID longer than 32 bytes', () => {
    const conv = makeConverter();
    const longId = ('0x' + 'ff'.repeat(33)) as `0x${string}`;
    expect(() => conv.addSample(longId, [1.0, 2.0, 3.0, 4.0])).toThrow(InputValidationError);
    expect(() => conv.addSample(longId, [1.0, 2.0, 3.0, 4.0])).toThrow(/too long/);
  });

  it('should throw on NaN feature', () => {
    const conv = makeConverter();
    expect(() => conv.addSample(zeroId, [NaN, 2.0, 3.0, 4.0])).toThrow(InputValidationError);
    expect(() => conv.addSample(zeroId, [NaN, 2.0, 3.0, 4.0])).toThrow(/NaN/);
  });

  it('should throw on Infinity feature', () => {
    const conv = makeConverter();
    expect(() => conv.addSample(zeroId, [Infinity, 2.0, 3.0, 4.0])).toThrow(InputValidationError);
    expect(() => conv.addSample(zeroId, [Infinity, 2.0, 3.0, 4.0])).toThrow(/Inf/);
  });

  it('should throw on NaN threshold', () => {
    const conv = makeConverter();
    expect(() => conv.setThreshold(NaN)).toThrow(InputValidationError);
    expect(() => conv.setThreshold(NaN)).toThrow(/finite/);
  });

  it('should throw on Infinity threshold', () => {
    const conv = makeConverter();
    expect(() => conv.setThreshold(Infinity)).toThrow(InputValidationError);
  });

  it('should throw on serialize with no samples', () => {
    const conv = makeConverter();
    expect(() => conv.serialize()).toThrow(InputValidationError);
    expect(() => conv.serialize()).toThrow(/At least one sample/);
  });
});

// ============================================================================
// Test: Data URL output
// ============================================================================

describe('LightGBMConverter - Data URL', () => {
  it('should produce a valid data URL', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleBinaryModel()));
    const zeroId = ('0x' + '00'.repeat(32)) as `0x${string}`;
    converter.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]);

    const url = converter.toDataUrl();
    expect(url).toMatch(/^data:application\/octet-stream;base64,/);

    // Decode and verify it matches raw serialize
    const encodedPart = url.split(',')[1];
    const decoded = Uint8Array.from(atob(encodedPart), c => c.charCodeAt(0));
    expect(decoded).toEqual(converter.serialize());
  });
});

// ============================================================================
// Test: Input digest
// ============================================================================

describe('LightGBMConverter - Input digest', () => {
  it('should produce a 0x-prefixed 66-char hex string', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleBinaryModel()));
    const zeroId = ('0x' + '00'.repeat(32)) as `0x${string}`;
    converter.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]);

    const digest = converter.inputDigest();
    expect(digest).toMatch(/^0x[0-9a-f]{64}$/);
  });

  it('should be deterministic', () => {
    const json = JSON.stringify(sampleBinaryModel());
    const zeroId = ('0x' + '00'.repeat(32)) as `0x${string}`;

    const conv1 = LightGBMConverter.fromJSON(json);
    conv1.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]);

    const conv2 = LightGBMConverter.fromJSON(json);
    conv2.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]);

    expect(conv1.inputDigest()).toBe(conv2.inputDigest());
  });
});

// ============================================================================
// Test: Edge cases
// ============================================================================

describe('LightGBMConverter - Edge cases', () => {
  it('should handle leaf with no leaf_value (defaults to 0)', () => {
    const data = {
      num_class: 1,
      max_feature_idx: 0,
      tree_info: [{
        tree_structure: {
          leaf_index: 0,
          // no leaf_value key
        },
      }],
    };
    const converter = LightGBMConverter.fromJSON(JSON.stringify(data));
    expect(converter.model.trees[0][0].leafValue).toBeCloseTo(0.0);
  });

  it('should handle large leaf values', () => {
    const data = {
      num_class: 1,
      max_feature_idx: 0,
      tree_info: [{
        tree_structure: {
          split_index: 0,
          split_feature: 0,
          threshold: 0.0,
          decision_type: '<=',
          left_child: { leaf_index: 0, leaf_value: -1e100 },
          right_child: { leaf_index: 1, leaf_value: 1e100 },
        },
      }],
    };
    const converter = LightGBMConverter.fromJSON(JSON.stringify(data));
    const tree = converter.model.trees[0];
    expect(tree[1].leafValue).toBeCloseTo(-1e100);
    expect(tree[2].leafValue).toBeCloseTo(1e100);
  });

  it('should handle regression model (num_class=1 mapped to 2)', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleSingleLeafModel()));
    expect(converter.model.numClasses).toBe(2);
  });

  it('should default to 0.5 threshold', () => {
    // Access the serialization to verify default threshold
    // We check via serialized bytes difference when threshold changes
    const zeroId = ('0x' + '00'.repeat(32)) as `0x${string}`;

    const conv1 = LightGBMConverter.fromJSON(JSON.stringify(sampleBinaryModel()));
    conv1.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]);

    const conv2 = LightGBMConverter.fromJSON(JSON.stringify(sampleBinaryModel()));
    conv2.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]).setThreshold(0.5);

    // Default threshold is 0.5, so these should be equal
    expect(conv1.serialize()).toEqual(conv2.serialize());
  });
});

// ============================================================================
// Test: Serialization binary format
// ============================================================================

describe('LightGBMConverter - Binary format', () => {
  it('should start with num_features as u32 LE', () => {
    const converter = LightGBMConverter.fromJSON(JSON.stringify(sampleBinaryModel()));
    const zeroId = ('0x' + '00'.repeat(32)) as `0x${string}`;
    converter.addSample(zeroId, [1.0, 2.0, 3.0, 4.0]);

    const bytes = converter.serialize();
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);

    // First 4 bytes: num_features = 4
    expect(view.getUint32(0, true)).toBe(4);
    // Next 4 bytes: num_classes = 2
    expect(view.getUint32(4, true)).toBe(2);
    // Next 8 bytes: base_score = 0.0
    expect(view.getFloat64(8, true)).toBeCloseTo(0.0);
    // Next 4 bytes: num_trees = 2
    expect(view.getUint32(16, true)).toBe(2);
  });
});
