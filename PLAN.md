# Phase 1: XGBoost Decision Tree Inference Circuit

## Goal

Replace the placeholder multiply-subtract circuit with a real circuit that proves:
"Given committed private features F and public model M, the XGBoost prediction is Y."

The current circuit (`features * leaf_values - expected == 0`) only proves an arithmetic
relationship. It does not verify tree traversal — the prover can supply arbitrary leaf values.

## Staged Approach

| Stage | What it proves | Rust | Solidity |
|-------|---------------|------|----------|
| **1a** | Valid binary paths select correct leaves → correct sum | Yes | No |
| **1b** | Feature-threshold comparisons match path bits | Yes | No |
| **1c** | On-chain verification of multi-layer circuits | — | Yes |

Phase 1a is the immediate target. 1b and 1c are follow-ups.

---

## Phase 1a: Leaf Selection + Binary Path Verification

### Circuit Architecture

For a model with T trees, max depth d, F features:

```
COMMITTED (private):
  path_bits[T * d]          — binary indicators: 0=left, 1=right at each node

PUBLIC:
  leaf_values[T * 2^d]      — all leaf values from the model (flattened, quantized)
  expected_sum               — aggregated prediction (quantized)

CIRCUIT LAYERS:
  1. Binary check:    path_bits * (path_bits - 1) == 0
  2. Leaf fold:       For each tree, fold 2^d leaves down to 1 using path bits (d levels)
  3. Aggregation:     Sum of T selected leaves == expected_sum
```

### Leaf Fold (MLE Evaluation via Routing + Arithmetic)

For each tree, select the correct leaf by folding the leaf array:

```
Level 0:  [l0, l1, l2, l3, l4, l5, l6, l7]   (2^d values)
Level 1:  [l0 + b0*(l1-l0), l2 + b0*(l3-l2), l4 + b0*(l5-l4), l6 + b0*(l7-l6)]
Level 2:  [v0 + b1*(v1-v0), v2 + b1*(v3-v2)]
Level 3:  [v0 + b2*(v1-v0)]                    = selected leaf
```

Each fold level:
1. **Route even/odd** — `add_identity_gate_node` to split v[2i] and v[2i+1]
2. **Diff** — `add_sector(v_odd - v_even)`
3. **Scale** — `add_sector(b_k_expanded * diff)`
4. **Sum** — `add_sector(v_even + scaled)`

All T trees are processed in parallel via `num_dataparallel_bits = ceil(log2(T))`.

### Data Layout

All arrays padded to next power of 2. Trees padded to uniform max depth (extra leaves = 0).

```
path_bits layout:  [tree0_b0, tree0_b1, ..., tree0_b{d-1}, tree1_b0, ..., treeT_b{d-1}]
leaf_values layout: [tree0_leaf0..leaf{2^d-1}, tree1_leaf0..leaf{2^d-1}, ...]
```

### What Phase 1a Proves (and doesn't)

**Proves:**
- The prover committed to valid binary paths (each bit is 0 or 1)
- The paths select specific leaves from the public model's leaf table
- The selected leaf values aggregate to the claimed prediction

**Does NOT prove:**
- That the path bits correspond to actual feature-threshold comparisons
- That the features are the actual input (features not used in circuit yet)

This is still meaningful: the prover is bound to the model's leaf values and must choose
valid paths. The missing piece (comparison verification) is Phase 1b.

---

## Phase 1b: Comparison Constraints (follow-up)

Adds committed witness:
- `selected_features[T*d]` — feature value at each comparison node
- `bit_decomp[T*d*K]` — K-bit decomposition of (feature - threshold + 2^K)

Additional circuit layers:
1. **Feature routing** — verify selected_features match features[feature_index] via identity gate
2. **Bit binary check** — `bit * (bit - 1) == 0` for all decomposition bits
3. **Reconstruction** — weighted sum of bits == selected_feature - threshold + offset
4. **Sign consistency** — top bit == path_bit (sign of difference matches direction)

---

## Implementation Plan (Phase 1a)

### Step 1: Add tree padding utilities to `model.rs`

```rust
// Pad all trees to uniform depth by duplicating leaf values
pub fn pad_trees_to_max_depth(model: &XgboostModel) -> Vec<Vec<i64>> { ... }

// Extract path bits for all trees given features
pub fn compute_path_bits(model: &XgboostModel, features: &[f64]) -> Vec<Vec<bool>> { ... }

// Compute expected sum of selected leaves
pub fn compute_leaf_sum(model: &XgboostModel, features: &[f64]) -> i64 { ... }
```

### Step 2: New circuit builder in `circuit.rs`

```rust
/// Build a GKR circuit that verifies XGBoost leaf selection and aggregation
fn build_tree_inference_circuit(
    num_trees_padded: usize,   // T, padded to power of 2
    max_depth: usize,          // d
) -> Circuit<Fr> {
    let mut builder = CircuitBuilder::<Fr>::new();

    let private = builder.add_input_layer("private", LayerVisibility::Committed);
    let public = builder.add_input_layer("public", LayerVisibility::Public);

    // Path bits: T * d values
    let path_nv = next_log2(num_trees_padded * max_depth);
    let path_bits = builder.add_input_shred("path_bits", path_nv, &private);

    // Leaf values: T * 2^d values
    let leaves_nv = next_log2(num_trees_padded * (1 << max_depth));
    let leaf_values = builder.add_input_shred("leaf_values", leaves_nv, &public);

    // Expected sum: scalar
    let expected_sum = builder.add_input_shred("expected_sum", 0, &public);

    // --- Layer 1: Binary check ---
    let binary_check = builder.add_sector(
        path_bits.expr() * path_bits.expr() - path_bits.expr()
    );
    // binary_check must be zero

    // --- Layers 2..2+d: Leaf fold ---
    // For each depth level k=0..d-1, fold the leaf array
    let tree_parallel_bits = next_log2(num_trees_padded);
    let mut current = leaf_values;  // Start with all leaves

    for k in 0..max_depth {
        let nv = max_depth - k;  // current array has 2^nv elements per tree

        // Extract path bit b_k for each tree
        // path_bits layout: [t0_b0, t0_b1, ..., t0_b{d-1}, t1_b0, ...]
        // Need to route: for tree t, select path_bits[t*d + k]
        let b_k = route_path_bit(&mut builder, &path_bits, k, max_depth,
                                  num_trees_padded, tree_parallel_bits);

        // Even/odd split + fold
        current = fold_level(&mut builder, &current, &b_k, nv, tree_parallel_bits);
    }
    // current now has 1 value per tree = selected leaf values

    // --- Aggregation: sum across trees ---
    let total = aggregate_sum(&mut builder, &current, tree_parallel_bits);

    // --- Output: total - expected_sum == 0 ---
    let output = builder.add_sector(total.expr() - expected_sum.expr());
    builder.set_output(&output);

    builder.build().expect("Failed to build tree inference circuit")
}
```

### Step 3: Helper functions for fold and aggregation

```rust
/// Route path bit b_k for each tree (identity gate)
fn route_path_bit(builder, path_bits, k, d, T, tree_nv) -> NodeRef {
    // From path_bits[t*d + k] → b_k[t] for each tree t
    let gates: Vec<(u32, u32)> = (0..T)
        .map(|t| (t as u32, (t * d + k) as u32))
        .collect();
    builder.add_identity_gate_node(path_bits, gates, tree_nv, None)
}

/// One fold level: current[2i] + b * (current[2i+1] - current[2i])
fn fold_level(builder, current, b_k, nv, tree_parallel_bits) -> NodeRef {
    let half = 1 << (nv - 1);
    let T = 1 << tree_parallel_bits;

    // Route even elements: current[2i] → v_even[i]
    let even_gates = ...;
    let v_even = builder.add_identity_gate_node(current, even_gates, nv-1+tree_parallel_bits, None);

    // Route odd elements: current[2i+1] → v_odd[i]
    let odd_gates = ...;
    let v_odd = builder.add_identity_gate_node(current, odd_gates, nv-1+tree_parallel_bits, None);

    // Expand b_k from T elements to T * half elements (replicate for each position)
    let b_expanded = ...;

    // diff = v_odd - v_even
    let diff = builder.add_sector(v_odd.expr() - v_even.expr());
    // scaled = b_k * diff
    let scaled = builder.add_sector(b_expanded.expr() * diff.expr());
    // v_new = v_even + scaled
    builder.add_sector(v_even.expr() + scaled.expr())
}

/// Sum T values down to 1 using tree of additions
fn aggregate_sum(builder, values, tree_nv) -> NodeRef {
    let mut current = values;
    for level in (0..tree_nv).rev() {
        // Pair up and add: sum[i] = current[2i] + current[2i+1]
        let nv = level + 1;
        let even_gates = (0..(1<<level)).map(|i| (i as u32, (2*i) as u32)).collect();
        let odd_gates = (0..(1<<level)).map(|i| (i as u32, (2*i+1) as u32)).collect();
        let v_even = builder.add_identity_gate_node(&current, even_gates, level, None);
        let v_odd = builder.add_identity_gate_node(&current, odd_gates, level, None);
        current = builder.add_sector(v_even.expr() + v_odd.expr());
    }
    current
}
```

### Step 4: Wire `build_and_prove` to new circuit

Update `build_and_prove()` in `circuit.rs`:
1. Call `pad_trees_to_max_depth(model)` to get leaf tables
2. Call `compute_path_bits(model, features)` to get witness
3. Build circuit with `build_tree_inference_circuit(T_padded, d)`
4. Set inputs: path_bits (committed), leaf_values (public), expected_sum (public)
5. Prove and verify

### Step 5: Tests

| Test | Description |
|------|-------------|
| `test_single_tree_depth1` | 1 tree, 2 leaves, 1 path bit → verify correct leaf selected |
| `test_single_tree_depth3` | 1 tree, 8 leaves, 3 path bits → verify correct leaf |
| `test_two_trees` | 2 trees, depth 2 → verify sum of 2 selected leaves |
| `test_sample_model` | Full sample_model (2 trees, depth 3) → correct prediction |
| `test_wrong_path_bits` | Tampered path bits → proof fails or wrong output |
| `test_wrong_expected_sum` | Wrong prediction → proof fails |

### Step 6: Benchmark

Measure proving time and proof size for sample_model. Compare to placeholder circuit.

---

## Files Modified

| File | Change |
|------|--------|
| `examples/xgboost-remainder/src/model.rs` | Add tree padding, path bit extraction, leaf sum computation |
| `examples/xgboost-remainder/src/circuit.rs` | New `build_tree_inference_circuit` + fold/aggregate helpers |
| `examples/xgboost-remainder/src/main.rs` | Wire to new circuit builder |

## Files NOT Modified (Phase 1a)

- Solidity contracts — on-chain verification deferred to Phase 1c
- gnark-wrapper — Groth16 wrapper unchanged until circuit stabilizes
- ABI encoding — proof format may change but kept compatible for now

---

## Key Risks

1. **CircuitBuilder API gaps**: The identity gate and sector interaction may have edge cases
   we haven't seen in the simple placeholder circuit. Mitigation: start with depth-1 tree
   and build up incrementally.

2. **Proof size growth**: More layers = larger proof. The fold creates ~5 layers per depth
   level + aggregation layers. For depth 5, that's ~27 layers. May impact verification gas.
   Mitigation: measure and optimize if needed.

3. **Negative values**: Quantized features can be negative. Field arithmetic handles this
   (negative = large positive mod p), but we need consistent encoding.
   Mitigation: use signed-to-field conversion consistently.

4. **Data-parallel gate API**: The `num_dataparallel_bits` parameter may not work exactly
   as expected for our tree-parallel pattern. Mitigation: fall back to manual unrolling
   if needed.
