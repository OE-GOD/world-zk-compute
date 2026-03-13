World ZK Compute Python SDK
============================

API Documentation
-----------------

Generate API docs using pdoc3::

    # HTML (recommended)
    pdoc --html --output-dir docs/api worldzk

    # Serve locally for development
    pdoc --http : worldzk

    # Markdown output
    pdoc --pdf worldzk > docs/api.md

Modules
~~~~~~~

- ``worldzk`` - Package root, re-exports all public classes
- ``worldzk.client`` - Sync/async HTTP clients for the World ZK Compute API
- ``worldzk.tee_verifier`` - TEE ML Verifier on-chain client
- ``worldzk.event_watcher`` - TEE contract event watcher (sync, threaded)
- ``worldzk.async_client`` - Async TEE verifier and event watcher
- ``worldzk.batch_verifier`` - Low-level multi-tx batch verification
- ``worldzk.verifier`` - High-level batch verifier
- ``worldzk.hash_utils`` - Keccak256 hash utilities for model/input/result hashing
- ``worldzk.xgboost`` - XGBoost model converter (JSON to risc0 serde binary)
- ``worldzk.lightgbm`` - LightGBM model converter
- ``worldzk.cli`` - Command-line interface
- ``worldzk.models`` - Data models (requests, provers, programs)
- ``worldzk.errors`` - Error classes with WZK-XXXX error codes
- ``worldzk.abi`` - ABI definitions for DAGBatchVerifier contract
