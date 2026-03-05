# World ZK Compute

## Multi-Agent Coordination

When multiple Claude Code sessions are running on this project, they coordinate through a shared task board at `.claude/agent-state/tasks.json`.

### Every session MUST:

1. **On start**: Read `.claude/agent-state/tasks.json` to see what other agents are doing
2. **Before working**: Update `tasks.json` — set your task to `"in_progress"` and register files you'll edit in `file_ownership`
3. **Before editing a file**: Re-read `tasks.json` → check `file_ownership` → if another agent owns it, DON'T touch it
4. **After finishing**: Mark task `"done"` in `tasks.json`, **immediately** pick the next unclaimed task and start working on it
5. **If blocked**: Mark `"blocked"` with reason, skip to next available task
6. **Periodically**: Re-read `tasks.json` to see if other agents finished work you depend on

### CRITICAL: Never Stop Working

- **DO NOT stop after completing a task.** Always check `tasks.json` for the next unclaimed task and start it immediately.
- **DO NOT ask the user for permission** to continue. Just pick up the next task and go.
- **DO NOT summarize and wait.** After marking a task done, read `tasks.json`, claim the next one, and execute it.
- **If all tasks are done**, look for follow-up work: run tests, fix any failures, improve what was built.
- **If truly nothing left to do**, report completion and stop.
- **Never ask "should I continue?"** — the answer is always yes.

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
