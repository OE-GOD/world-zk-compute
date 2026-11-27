World ZK Compute Python SDK
============================

API Documentation
-----------------

Generate API docs using pdoc (``pip install -e '.[docs,web3]'``)::

    # HTML (recommended)
    pdoc -o docs/api worldzk

    # Serve locally for development
    pdoc worldzk

Modules
~~~~~~~

- ``worldzk`` - Package root, re-exports all public classes
- ``worldzk.client`` - HTTP clients for the API gateway (``WorldZK``) and the individual services (``Client``)
- ``worldzk.indexer`` - Sync/async HTTP clients for the TEE indexer REST API
- ``worldzk.tee_verifier`` - TEE ML Verifier on-chain client
- ``worldzk.event_watcher`` - TEE contract event watcher (sync, threaded)
- ``worldzk.async_client`` - Async TEE verifier and event watcher
- ``worldzk.batch_verifier`` - Low-level multi-tx batch verification
- ``worldzk.verifier`` - High-level batch verifier
- ``worldzk.execution_engine`` - ExecutionEngine on-chain client
- ``worldzk.prover_registry`` - ProverRegistry on-chain client
- ``worldzk.hash_utils`` - Keccak256 hash utilities for model/input/result hashing
- ``worldzk.xgboost`` - XGBoost model converter (JSON to risc0 serde binary)
- ``worldzk.lightgbm`` - LightGBM model converter
- ``worldzk.networks`` - Known network configurations
- ``worldzk.cli`` - Command-line interface
- ``worldzk.models`` - Data models (gateway results and receipts, indexer rows)
- ``worldzk.errors`` - Error classes with WZK-XXXX error codes
- ``worldzk.abi`` - ABI definitions for DAGBatchVerifier contract
