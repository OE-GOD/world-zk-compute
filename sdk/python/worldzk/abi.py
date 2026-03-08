"""
ABI definition for the DAGBatchVerifier contract.

Contains the minimal ABI needed for batch verification:
6 functions (start, continue, finalize, cleanup, getSession, isCircuitActive)
and 3 events (SessionStarted, BatchCompleted, BatchFinalized).
"""

DAG_BATCH_VERIFIER_ABI = [
    {
        "type": "function",
        "name": "startDAGBatchVerify",
        "inputs": [
            {"name": "proof", "type": "bytes"},
            {"name": "circuitHash", "type": "bytes32"},
            {"name": "publicInputs", "type": "bytes"},
            {"name": "gensData", "type": "bytes"},
        ],
        "outputs": [{"name": "sessionId", "type": "bytes32"}],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "continueDAGBatchVerify",
        "inputs": [
            {"name": "sessionId", "type": "bytes32"},
            {"name": "proof", "type": "bytes"},
            {"name": "publicInputs", "type": "bytes"},
            {"name": "gensData", "type": "bytes"},
        ],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "finalizeDAGBatchVerify",
        "inputs": [
            {"name": "sessionId", "type": "bytes32"},
            {"name": "proof", "type": "bytes"},
            {"name": "publicInputs", "type": "bytes"},
            {"name": "gensData", "type": "bytes"},
        ],
        "outputs": [{"name": "finalized", "type": "bool"}],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "cleanupDAGBatchSession",
        "inputs": [{"name": "sessionId", "type": "bytes32"}],
        "outputs": [],
        "stateMutability": "nonpayable",
    },
    {
        "type": "function",
        "name": "getDAGBatchSession",
        "inputs": [{"name": "sessionId", "type": "bytes32"}],
        "outputs": [
            {"name": "circuitHash", "type": "bytes32"},
            {"name": "nextBatchIdx", "type": "uint256"},
            {"name": "totalBatches", "type": "uint256"},
            {"name": "finalized", "type": "bool"},
            {"name": "finalizeInputIdx", "type": "uint256"},
            {"name": "finalizeGroupsDone", "type": "uint256"},
        ],
        "stateMutability": "view",
    },
    {
        "type": "function",
        "name": "isDAGCircuitActive",
        "inputs": [{"name": "circuitHash", "type": "bytes32"}],
        "outputs": [{"name": "", "type": "bool"}],
        "stateMutability": "view",
    },
    {
        "type": "event",
        "name": "DAGBatchSessionStarted",
        "inputs": [
            {"name": "sessionId", "type": "bytes32", "indexed": True},
            {"name": "sender", "type": "address", "indexed": True},
            {"name": "circuitHash", "type": "bytes32", "indexed": False},
        ],
    },
    {
        "type": "event",
        "name": "DAGBatchCompleted",
        "inputs": [
            {"name": "sessionId", "type": "bytes32", "indexed": True},
            {"name": "batchIdx", "type": "uint256", "indexed": False},
        ],
    },
    {
        "type": "event",
        "name": "DAGBatchFinalized",
        "inputs": [
            {"name": "sessionId", "type": "bytes32", "indexed": True},
        ],
    },
]
