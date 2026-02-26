"""Train an XGBoost fraud detection model and export to ONNX format.

Uses hummingbird-ml to convert tree ensembles into tensor operations,
producing an ONNX model with standard operators that EZKL can compile
into Halo2 circuits. Direct XGBoost-to-ONNX exports (via onnxmltools)
use ai.onnx.ml tree-ensemble operators which EZKL cannot parse.
"""

import json
import os

import numpy as np
import torch
import xgboost as xgb
from hummingbird.ml import convert as hb_convert
from sklearn.datasets import make_classification
from sklearn.model_selection import train_test_split
from sklearn.metrics import accuracy_score, classification_report

OUT_DIR = os.path.dirname(os.path.abspath(__file__))
MODEL_PATH = os.path.join(OUT_DIR, "model.onnx")
INPUT_PATH = os.path.join(OUT_DIR, "test_input.json")
CAL_DATA_PATH = os.path.join(OUT_DIR, "cal_data.json")

N_FEATURES = 5
N_SAMPLES = 500
N_ESTIMATORS = 10
MAX_DEPTH = 4
RANDOM_SEED = 42


def main():
    print(f"Generating synthetic fraud detection dataset ({N_SAMPLES} samples, {N_FEATURES} features)...")
    X, y = make_classification(
        n_samples=N_SAMPLES,
        n_features=N_FEATURES,
        n_informative=3,
        n_redundant=1,
        n_clusters_per_class=2,
        weights=[0.9, 0.1],  # 10% fraud rate
        random_state=RANDOM_SEED,
    )
    X = X.astype(np.float32)

    X_train, X_test, y_train, y_test = train_test_split(
        X, y, test_size=0.2, random_state=RANDOM_SEED, stratify=y,
    )

    print(f"Training XGBoost model (n_estimators={N_ESTIMATORS}, max_depth={MAX_DEPTH})...")
    model = xgb.XGBClassifier(
        n_estimators=N_ESTIMATORS,
        max_depth=MAX_DEPTH,
        objective="binary:logistic",
        eval_metric="logloss",
        random_state=RANDOM_SEED,
    )
    model.fit(X_train, y_train)

    y_pred = model.predict(X_test)
    accuracy = accuracy_score(y_test, y_pred)
    print(f"\nModel accuracy: {accuracy:.4f}")
    print(classification_report(y_test, y_pred, target_names=["normal", "fraud"]))

    # Convert XGBoost trees to PyTorch tensor operations via hummingbird-ml,
    # then export to ONNX with standard operators (not ai.onnx.ml tree ops).
    print("Converting model to tensor operations via hummingbird-ml...")
    hb_model = hb_convert(model, "torch", test_input=X_test[:1])

    print("Exporting to ONNX format...")
    dummy_input = torch.FloatTensor(X_test[:1])
    torch.onnx.export(
        hb_model.model,
        dummy_input,
        MODEL_PATH,
        input_names=["input"],
        output_names=["output"],
        opset_version=17,
    )
    print(f"Saved ONNX model to {MODEL_PATH}")

    # Save a single test sample for proving
    test_sample = X_test[0:1].tolist()
    test_label = int(y_test[0])
    input_data = {"input_data": test_sample}

    with open(INPUT_PATH, "w") as f:
        json.dump(input_data, f, indent=2)
    print(f"Saved test input to {INPUT_PATH} (true label: {test_label})")

    # Save calibration data (small subset for EZKL calibration step)
    cal_indices = np.random.RandomState(RANDOM_SEED).choice(len(X_test), size=min(20, len(X_test)), replace=False)
    cal_samples = X_test[cal_indices].tolist()
    cal_data = {"input_data": cal_samples}

    with open(CAL_DATA_PATH, "w") as f:
        json.dump(cal_data, f, indent=2)
    print(f"Saved calibration data ({len(cal_samples)} samples) to {CAL_DATA_PATH}")

    # Summary stats
    model_size = os.path.getsize(MODEL_PATH)
    print(f"\nSummary:")
    print(f"  Features:     {N_FEATURES}")
    print(f"  Trees:        {N_ESTIMATORS}")
    print(f"  Max depth:    {MAX_DEPTH}")
    print(f"  ONNX size:    {model_size:,} bytes")
    print(f"  Train/test:   {len(X_train)}/{len(X_test)} samples")


if __name__ == "__main__":
    main()
