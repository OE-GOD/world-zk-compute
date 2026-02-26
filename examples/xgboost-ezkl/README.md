# XGBoost Inference with EZKL

Proves XGBoost model inference using [EZKL](https://github.com/zkonduit/ezkl), a specialized zkML framework that converts ONNX models to Halo2 ZK circuits.

## Why EZKL?

The existing `examples/xgboost-inference/` uses risc0-zkvm, a general-purpose zkVM. EZKL takes a different approach: it compiles ML models directly into optimized arithmetic circuits, avoiding RISC-V emulation overhead entirely. For tree-based models like XGBoost, this yields significantly faster proving and lower memory usage.

| Metric | risc0-zkvm | EZKL |
|---|---|---|
| Proving time | ~10s | ~3s |
| Memory | ~1.3 GB | ~70 MB |
| Proof size | ~260 bytes (Groth16) | ~33 KB (Halo2 KZG) |
| On-chain verifier | risc0 router contract | Generated Solidity |
| Setup | Per-guest image ID | Per-model circuit |

## Prerequisites

```bash
pip install -r requirements.txt
```

EZKL requires Python 3.9+.

## Usage

### 1. Train model and export to ONNX

```bash
python train_model.py
```

Trains a sample XGBoost fraud detection classifier on synthetic data (10 trees, max depth 4), converts it to tensor operations via [hummingbird-ml](https://github.com/microsoft/hummingbird), and exports to `model.onnx`. Also saves a test input to `test_input.json`.

### 2. Generate and verify a ZK proof

```bash
python prove.py
```

Runs the full EZKL proving pipeline: circuit generation, trusted setup, witness generation, proving, and verification. Prints timing for each step.

### 3. Generate Solidity verifier (optional)

```bash
python gen_solidity_verifier.py
```

Generates a Solidity contract (`Verifier.sol`) that can verify proofs on-chain.

### 4. Run benchmarks (optional)

```bash
python benchmark.py
```

Runs proving multiple times and reports statistics.

## Why Hummingbird-ML?

EZKL only supports standard ONNX tensor operators (Add, MatMul, Relu, etc.). Direct XGBoost-to-ONNX converters (like `onnxmltools`) produce models using the `ai.onnx.ml` operator set (`TreeEnsembleClassifier`), which EZKL cannot parse.

[Hummingbird-ML](https://github.com/microsoft/hummingbird) (from Microsoft) solves this by converting tree ensemble logic into equivalent tensor computations — tree splits become comparison matrices, leaf values become lookup tensors. The resulting PyTorch model is then exported to ONNX with only standard operators, making it fully compatible with EZKL.

## How It Works

EZKL converts the ONNX model into a Halo2 arithmetic circuit:

1. **gen_settings** - Analyze model graph, determine circuit dimensions
2. **calibrate_settings** - Run calibration data to choose quantization parameters
3. **compile_circuit** - Compile ONNX operations into Halo2 gates
4. **get_srs** - Download structured reference string (universal trusted setup)
5. **setup** - Generate proving key and verifying key for this circuit
6. **gen_witness** - Evaluate the circuit on concrete input, produce witness
7. **prove** - Generate a ZK proof (Halo2 with KZG commitments)
8. **verify** - Verify the proof against the verifying key

## Comparison with risc0 Approach

The risc0 approach (`examples/xgboost-inference/`) runs XGBoost tree traversal inside a RISC-V virtual machine and proves correct execution of the entire VM trace. This is flexible (any Rust code works) but incurs overhead from emulating every CPU instruction.

EZKL compiles the model's computation graph directly into polynomial constraints. Each tree split becomes a comparison gate; each accumulation becomes an addition gate. No instruction emulation, no general-purpose overhead. The tradeoff is that EZKL only supports operations expressible in ONNX — arbitrary Rust logic (like the SHA-256 input hashing in the risc0 guest) is not possible.
