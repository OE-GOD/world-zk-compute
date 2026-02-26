"""Benchmark EZKL proving for XGBoost and compare with risc0 estimates."""

import json
import os
import statistics
import time

import ezkl
import psutil

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))

COMPILED_MODEL_PATH = os.path.join(SCRIPT_DIR, "model.compiled")
PK_PATH = os.path.join(SCRIPT_DIR, "pk.key")
SRS_PATH = os.path.join(SCRIPT_DIR, "kzg.srs")
SETTINGS_PATH = os.path.join(SCRIPT_DIR, "settings.json")
VK_PATH = os.path.join(SCRIPT_DIR, "vk.key")
INPUT_PATH = os.path.join(SCRIPT_DIR, "test_input.json")
WITNESS_PATH = os.path.join(SCRIPT_DIR, "witness.json")
PROOF_PATH = os.path.join(SCRIPT_DIR, "proof.json")

N_RUNS = 5

# risc0 reference values from existing xgboost-inference example
RISC0_PROVE_TIME_S = 10.0
RISC0_GROTH16_PROOF_BYTES = 260
RISC0_STARK_PROOF_BYTES = 200_000
RISC0_MEMORY_MB = 1300.0


def run_prove_iteration():
    """Run witness generation + proving and return (duration, peak_memory_mb)."""
    process = psutil.Process()
    mem_before = process.memory_info().rss
    start = time.perf_counter()

    ezkl.gen_witness(INPUT_PATH, COMPILED_MODEL_PATH, WITNESS_PATH)
    ezkl.prove(WITNESS_PATH, COMPILED_MODEL_PATH, PK_PATH, PROOF_PATH, SRS_PATH)

    elapsed = time.perf_counter() - start
    mem_after = process.memory_info().rss
    mem_used_mb = (mem_after - mem_before) / 1024 / 1024
    return elapsed, max(mem_used_mb, 0)


def main():
    required = [COMPILED_MODEL_PATH, PK_PATH, SRS_PATH, SETTINGS_PATH, VK_PATH, INPUT_PATH]
    for path in required:
        if not os.path.exists(path):
            print(f"Missing {os.path.basename(path)}. Run prove.py first to generate artifacts.")
            return

    print(f"Benchmarking EZKL proving ({N_RUNS} iterations)")
    print("=" * 60)

    # Warmup
    print("Warmup run...")
    run_prove_iteration()

    # Benchmark runs
    times = []
    peak_mems = []
    for i in range(N_RUNS):
        elapsed, peak = run_prove_iteration()
        times.append(elapsed)
        peak_mems.append(peak)
        print(f"  Run {i + 1}/{N_RUNS}: {elapsed:.3f}s, mem delta {peak:.1f} MB")

    proof_size = os.path.getsize(PROOF_PATH) if os.path.exists(PROOF_PATH) else 0

    # Verify the last proof
    is_valid = ezkl.verify(PROOF_PATH, SETTINGS_PATH, VK_PATH, SRS_PATH)

    # EZKL stats
    ezkl_mean = statistics.mean(times)
    ezkl_median = statistics.median(times)
    ezkl_min = min(times)
    ezkl_max = max(times)
    ezkl_mem = max(peak_mems)

    print("\n" + "=" * 60)
    print("EZKL Proving Statistics")
    print("=" * 60)
    print(f"  Mean time:      {ezkl_mean:.3f}s")
    print(f"  Median time:    {ezkl_median:.3f}s")
    print(f"  Min time:       {ezkl_min:.3f}s")
    print(f"  Max time:       {ezkl_max:.3f}s")
    print(f"  Proof size:     {proof_size:,} bytes")
    print(f"  Peak memory:    {ezkl_mem:.1f} MB")
    print(f"  Proof valid:    {is_valid}")

    # Comparison table
    speedup = RISC0_PROVE_TIME_S / ezkl_mean if ezkl_mean > 0 else float("inf")
    mem_ratio = RISC0_MEMORY_MB / ezkl_mem if ezkl_mem > 0 else float("inf")

    print("\n" + "=" * 60)
    print("Comparison: EZKL vs risc0-zkvm")
    print("=" * 60)
    print(f"{'Metric':<25} {'EZKL':<20} {'risc0-zkvm':<20} {'Ratio':<15}")
    print("-" * 80)
    print(f"{'Proving time':<25} {ezkl_mean:<20.3f} {RISC0_PROVE_TIME_S:<20.1f} {speedup:.1f}x faster")
    print(f"{'Proof size (bytes)':<25} {proof_size:<20,} {RISC0_GROTH16_PROOF_BYTES:<20,} {'N/A':<15}")
    print(f"{'Peak memory (MB)':<25} {ezkl_mem:<20.1f} {RISC0_MEMORY_MB:<20.1f} {mem_ratio:.1f}x less")

    print("\nNotes:")
    print("  - risc0 values are estimates from the existing xgboost-inference example")
    print("  - risc0 Groth16 proof (260 bytes) is smaller but requires ~6 min Groth16 wrapping")
    print("  - risc0 STARK proof (~200 KB) is the unwrapped proof before Groth16 compression")
    print("  - EZKL proof is a Halo2 proof with KZG commitments (no wrapping needed)")

    # Save results
    results = {
        "ezkl": {
            "mean_time_s": ezkl_mean,
            "median_time_s": ezkl_median,
            "min_time_s": ezkl_min,
            "max_time_s": ezkl_max,
            "proof_size_bytes": proof_size,
            "peak_memory_mb": ezkl_mem,
            "proof_valid": is_valid,
            "n_runs": N_RUNS,
        },
        "risc0_reference": {
            "prove_time_s": RISC0_PROVE_TIME_S,
            "groth16_proof_bytes": RISC0_GROTH16_PROOF_BYTES,
            "stark_proof_bytes": RISC0_STARK_PROOF_BYTES,
            "memory_mb": RISC0_MEMORY_MB,
        },
    }
    results_path = os.path.join(SCRIPT_DIR, "benchmark_results.json")
    with open(results_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nResults saved to {results_path}")


if __name__ == "__main__":
    main()
