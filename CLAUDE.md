# World ZK Compute

## Multi-Agent Coordination

When multiple Claude Code sessions are running on this project, they coordinate through scripts in `.claude/scripts/` and lock files in `.claude/locks/`.

### Every session MUST:

1. **On start**: `source .claude/scripts/agent-init.sh $AGENT_ID` (sets up build isolation, cleans stale locks)
2. **Before working**: Claim your task: `.claude/scripts/task-claim.sh claim T### $AGENT_ID`
3. **Before editing a file**: Claim it: `.claude/scripts/claim-file.sh claim <path> $AGENT_ID` — **MANDATORY, not optional**. If claim fails, **do NOT edit the file**. Skip to next task.
4. **After finishing**: Mark done: `.claude/scripts/task-claim.sh done T### $AGENT_ID` and release files: `.claude/scripts/claim-file.sh release <path> $AGENT_ID`
5. **If blocked**: Skip to next available task (`.claude/scripts/task-claim.sh list-available`)
6. **Periodically**: Check `.claude/scripts/claim-file.sh list` and `.claude/scripts/task-claim.sh list-available`

### Build Tool Wrappers

| Instead of... | Use... |
|---|---|
| `forge build` | `.claude/scripts/forge-build.sh` |
| `forge test` | `.claude/scripts/forge-test.sh` |
| `forge fmt` | `.claude/scripts/forge-fmt.sh` |
| `cargo test` | `.claude/scripts/cargo-test.sh` |
| Edit tasks.json | `.claude/scripts/task-edit.sh <command>` |
| Read tasks.json | `.claude/scripts/task-summary.sh [agent-id]` |

These wrappers handle locking (forge), per-agent build isolation (cargo), and atomic JSON writes (task-edit) automatically.

### Build Isolation (MANDATORY)

- `agents.sh run` sets `CARGO_TARGET_DIR=target/agent-$AGENT` automatically for each agent
- Solo agents MUST set it manually: `export CARGO_TARGET_DIR=target/agent-$AGENT_ID`
- Never use the shared `target/` directory when running in parallel — it causes lock contention
- Use `.claude/scripts/cargo-test.sh` which respects `CARGO_TARGET_DIR`

### Scope Boundaries (MANDATORY)

Agents MUST ONLY modify files listed in their task's `files` array. To edit a file not in your task:
1. Update tasks.json to add the file to your task's `files` list
2. Claim the file with `claim-file.sh`
3. Only then edit it

### Compilation Check (MANDATORY)

After EVERY file edit, verify compilation before proceeding:
- **Rust files**: Run `cargo check --workspace` (or `cargo check -p <crate>`)
- **Solidity files**: Run `.claude/scripts/forge-build.sh` (NOT raw `cd contracts && forge build`)
- If compilation fails, **fix it immediately** before touching any other file

### Re-read After Edits (MANDATORY)

Linters and formatters (`forge fmt`, `cargo fmt`, `rustfmt`) rewrite files. After running any formatter:
1. **Re-read the file** before making further edits — your cached version is stale
2. **Never assume** the file still matches what you wrote — the formatter may have changed whitespace, line breaks, or import ordering
3. This prevents "old_string not found" errors that waste turns

### Domain Enforcement

Check if you're allowed to edit a file: `.claude/scripts/check-domain.sh <role> <path>`

| Role | Allowed paths |
|---|---|
| builder-a | prover/, programs/, examples/*/src/, sdk/src/, crates/, tee/, services/ |
| builder-b | contracts/, scripts/ |
| planner/researcher | .claude/, *.md files |
| All agents | .github/, Makefile, *.md |

### When to Use Multi-Terminal vs Sub-Agents

**Prefer single agent spawning sub-agents** (higher code quality):
- Feature development, bug fixes, refactoring
- Tasks that touch multiple files that depend on each other
- Work requiring cross-domain coordination (Rust outputs → Solidity inputs)
- Anything where coherence and consistency matter

**Multi-terminal agents are only better for**:
- 10+ independent mechanical tasks (add NatSpec, rename variables, format files)
- Tasks that are fully isolated with zero cross-file dependencies
- Bulk operations where speed matters more than coherence

**How to tell from a task description**:
- If a task says "implement feature X" → single agent with sub-agents
- If a task says "add docs to 15 contracts" → multi-terminal is fine
- If tasks have dependencies between them → single agent must orchestrate
- If unsure → default to single agent with sub-agents

### CRITICAL: Never Stop Working

- **DO NOT stop after completing a task.** Always check for the next unclaimed task and start it immediately.
- **DO NOT ask the user for permission** to continue. Just pick up the next task and go.
- **DO NOT summarize and wait.** After marking a task done, claim the next one and execute it.
- **If all tasks are done**, look for follow-up work: run tests, fix any failures, improve what was built.
- **If truly nothing left to do**, report completion and stop.
- **Never ask "should I continue?"** — the answer is always yes.

### Conflict Rules

- First agent to claim a file via `claim-file.sh` owns it (atomic `mkdir` — second claimer gets an error)
- Task claims are atomic via `mkdir` — second claimer gets an error
- If another agent finished what you were about to do, skip it
- If another agent is blocked on YOUR output, prioritize that

### Task Archival

When `tasks.json` exceeds 100KB, run `.claude/scripts/archive-tasks.sh` to move completed phases to `.claude/agent-state/archive/`. The `agents.sh plan` command and `task-summary.sh` do this automatically.

### Subagent Workflow

Subagents cannot `source` shell scripts (no persistent shell state). Use these patterns instead:

```bash
# Instead of: source .claude/scripts/agent-init.sh builder-a
# Use:
eval $(.claude/scripts/agent-init.sh --env builder-a)

# Or set vars inline:
AGENT_ID=builder-a PROJECT_ROOT=$(pwd) CARGO_TARGET_DIR=$(pwd)/target/agent-builder-a .claude/scripts/cargo-test.sh -p my-crate
```

All wrapper scripts (`forge-build.sh`, `forge-test.sh`, `cargo-test.sh`, `task-edit.sh`) work without sourcing — they only need `PROJECT_ROOT` and `AGENT_ID` env vars.

### Agent Signals

Agents can notify each other about completed work:

- **On task completion**: `task-edit.sh done-task` automatically writes a signal file
- **On file release**: `task-edit.sh release-file` automatically writes a signal file
- **Check signals**: `.claude/scripts/check-signals.sh` — lists all pending notifications
- **Wait for file**: `.claude/scripts/check-signals.sh --wait <path> [timeout]`
- **Wait for task**: `.claude/scripts/check-signals.sh --wait-task <id> [timeout]`

Signal files live in `.claude/agent-state/signals/` and accumulate until cleared.

### Commit Coordination

Only ONE agent (the orchestrator) should run `git commit`. All other agents signal readiness:

1. **Background agent finishes work**: `.claude/scripts/commit-ready.sh signal <agent> <task> "<msg>"`
2. **Orchestrator checks**: `.claude/scripts/commit-ready.sh list`
3. **Orchestrator commits**: `git add` + `git commit` (only the orchestrator does this)
4. **Orchestrator clears**: `.claude/scripts/commit-ready.sh clear <task>`

This prevents merge conflicts from concurrent commits.

## Project Structure

- `prover/` — Rust risc0-zkvm v3.0 prover
- `contracts/` — Foundry Solidity contracts (verifiers, tests)
- `examples/xgboost-remainder/` — XGBoost tree inference circuit + GKR/Hyrax/Groth16
- `programs/` — Pre-compiled guest program binaries
- `scripts/` — E2E test scripts
- `sdk/` — Client SDK
