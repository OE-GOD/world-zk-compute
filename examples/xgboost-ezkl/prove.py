"""Generate and verify a ZK proof of XGBoost inference using EZKL."""

import asyncio
import json
import os
import time

import ezkl
import psutil

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))

# Input files (produced by train_model.py)
MODEL_PATH = os.path.join(SCRIPT_DIR, "model.onnx")
INPUT_PATH = os.path.join(SCRIPT_DIR, "test_input.json")
CAL_DATA_PATH = os.path.join(SCRIPT_DIR, "cal_data.json")

# Intermediate artifacts
SETTINGS_PATH = os.path.join(SCRIPT_DIR, "settings.json")
COMPILED_MODEL_PATH = os.path.join(SCRIPT_DIR, "model.compiled")
SRS_PATH = os.path.join(SCRIPT_DIR, "kzg.srs")
PK_PATH = os.path.join(SCRIPT_DIR, "pk.key")
VK_PATH = os.path.join(SCRIPT_DIR, "vk.key")
WITNESS_PATH = os.path.join(SCRIPT_DIR, "witness.json")

# Output
PROOF_PATH = os.path.join(SCRIPT_DIR, "proof.json")


def timed(label):
    """Context manager to time a step and print duration."""
    class Timer:
        def __init__(self):
            self.elapsed = 0.0
        def __enter__(self):
            self.start = time.perf_counter()
            return self
        def __exit__(self, *args):
            self.elapsed = time.perf_counter() - self.start
            print(f"  {label}: {self.elapsed:.3f}s")
    return Timer()


async def main():
    for path in [MODEL_PATH, INPUT_PATH]:
        if not os.path.exists(path):
            print(f"Missing {path}. Run train_model.py first.")
            return

    process = psutil.Process()
    mem_start = process.memory_info().rss
    timings = {}

    print("EZKL XGBoost Proving Pipeline")
    print("=" * 50)

    # Step 1: Generate circuit settings
    print("\n[1/8] Generating circuit settings...")
    with timed("gen_settings") as t:
        py_run_args = ezkl.PyRunArgs()
        py_run_args.input_visibility = "public"
        py_run_args.output_visibility = "public"
        py_run_args.param_visibility = "fixed"
        ezkl.gen_settings(MODEL_PATH, SETTINGS_PATH, py_run_args=py_run_args)
    timings["gen_settings"] = t.elapsed

    # Step 2: Calibrate settings
    print("\n[2/8] Calibrating settings...")
    with timed("calibrate_settings") as t:
        ezkl.calibrate_settings(
            CAL_DATA_PATH, MODEL_PATH, SETTINGS_PATH, target="resources",
        )
    timings["calibrate_settings"] = t.elapsed

    # Step 3: Compile circuit
    print("\n[3/8] Compiling ONNX model to Halo2 circuit...")
    with timed("compile_circuit") as t:
        ezkl.compile_circuit(MODEL_PATH, COMPILED_MODEL_PATH, SETTINGS_PATH)
    timings["compile_circuit"] = t.elapsed

    # Step 4: Get SRS (trusted setup parameters) — async, downloads from network
    print("\n[4/8] Fetching structured reference string (SRS)...")
    with timed("get_srs") as t:
        await ezkl.get_srs(settings_path=SETTINGS_PATH, srs_path=SRS_PATH)
    timings["get_srs"] = t.elapsed

    # Step 5: Setup (generate pk/vk)
    print("\n[5/8] Generating proving and verifying keys...")
    with timed("setup") as t:
        ezkl.setup(COMPILED_MODEL_PATH, VK_PATH, PK_PATH, SRS_PATH)
    timings["setup"] = t.elapsed

    # Step 6: Generate witness
    print("\n[6/8] Generating witness from input...")
    with timed("gen_witness") as t:
        ezkl.gen_witness(INPUT_PATH, COMPILED_MODEL_PATH, WITNESS_PATH)
    timings["gen_witness"] = t.elapsed

    # Step 7: Prove
    print("\n[7/8] Generating ZK proof...")
    with timed("prove") as t:
        ezkl.prove(
            WITNESS_PATH, COMPILED_MODEL_PATH, PK_PATH, PROOF_PATH, SRS_PATH,
        )
    timings["prove"] = t.elapsed

    # Step 8: Verify
    print("\n[8/8] Verifying proof...")
    with timed("verify") as t:
        is_valid = ezkl.verify(PROOF_PATH, SETTINGS_PATH, VK_PATH, SRS_PATH)
    timings["verify"] = t.elapsed

    mem_end = process.memory_info().rss
    peak_mem = mem_end - mem_start

    # Results
    print("\n" + "=" * 50)
    print("Results")
    print("=" * 50)
    print(f"  Proof valid:    {is_valid}")

    proof_size = os.path.getsize(PROOF_PATH) if os.path.exists(PROOF_PATH) else 0
    print(f"  Proof size:     {proof_size:,} bytes")

    total_time = sum(timings.values())
    prove_time = timings.get("prove", 0)
    print(f"  Proving time:   {prove_time:.3f}s")
    print(f"  Total pipeline: {total_time:.3f}s")
    print(f"  Peak memory:    {peak_mem / 1024 / 1024:.1f} MB")

    print(f"\nArtifacts saved:")
    print(f"  Proof:          {PROOF_PATH}")
    print(f"  Verifying key:  {VK_PATH}")
    print(f"  Settings:       {SETTINGS_PATH}")

    # Print witness output (model prediction)
    if os.path.exists(WITNESS_PATH):
        with open(WITNESS_PATH) as f:
            witness = json.load(f)
        if "outputs" in witness:
            print(f"\nModel output (from witness): {witness['outputs']}")

    # Save timing summary
    summary = {
        "timings": timings,
        "proof_size_bytes": proof_size,
        "peak_memory_mb": peak_mem / 1024 / 1024,
        "proof_valid": is_valid,
    }
    summary_path = os.path.join(SCRIPT_DIR, "timing_summary.json")
    with open(summary_path, "w") as f:
        json.dump(summary, f, indent=2)


if __name__ == "__main__":
    asyncio.run(main())
