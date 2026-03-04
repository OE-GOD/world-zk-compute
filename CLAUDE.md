# World ZK Compute

## Multi-Agent Coordination

When multiple Claude Code sessions are running on this project, they coordinate through a shared task board at `.claude/agent-state/tasks.json`.

### Every session MUST:

1. **On start**: Read `.claude/agent-state/tasks.json` to see what other agents are doing
2. **Before working**: Update `tasks.json` — set your task to `"in_progress"` and register files you'll edit in `file_ownership`
3. **Before editing a file**: Re-read `tasks.json` → check `file_ownership` → if another agent owns it, DON'T touch it
4. **After finishing**: Mark task `"done"` in `tasks.json`, pick the next unclaimed task
5. **If blocked**: Mark `"blocked"` with reason, skip to next available task
6. **Periodically**: Re-read `tasks.json` to see if other agents finished work you depend on

### Conflict Rules

- First agent to register a file in `file_ownership` owns it
- If another agent finished what you were about to do, skip it
- If another agent is blocked on YOUR output, prioritize that

## Project Structure

- `prover/` — Rust risc0-zkvm v3.0 prover
- `contracts/` — Foundry Solidity contracts (verifiers, tests)
- `examples/xgboost-remainder/` — XGBoost tree inference circuit + GKR/Hyrax/Groth16
- `programs/` — Pre-compiled guest program binaries
- `scripts/` — E2E test scripts
- `sdk/` — Client SDK
