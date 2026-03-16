#!/bin/bash
# Multi-agent launcher for world-zk-compute
# Usage:
#   ./agents.sh plan "Build the XGBoost verifier"    — create a plan
#   ./agents.sh run builder-a builder-b               — launch agents in parallel
#   ./agents.sh status                                — check what agents are doing
#   ./agents.sh solo builder-a                        — launch one agent interactively
#   ./agents.sh verify                                — run full test suite
#   ./agents.sh clean                                 — remove per-agent build dirs

set -e
TASK_FILE=".claude/agent-state/tasks.json"

# ── Helpers ──────────────────────────────────────────────────────────────────

preflight_check() {
    echo "==> Pre-flight: verifying HEAD compiles..."

    echo "    Checking Rust workspace..."
    if ! cargo check --workspace 2>&1 | tail -5; then
        echo "ERROR: cargo check failed. Fix compilation errors before launching agents." >&2
        return 1
    fi
    echo "    Rust OK."

    echo "    Checking Solidity contracts..."
    if ! (cd contracts && forge build 2>&1 | tail -5); then
        echo "ERROR: forge build failed. Fix compilation errors before launching agents." >&2
        return 1
    fi
    echo "    Solidity OK."

    echo "==> Pre-flight passed."
}

post_verify() {
    echo ""
    echo "==> Post-verification: running full test suites..."

    local failed=0

    echo "    Running cargo test --workspace..."
    if cargo test --workspace 2>&1 | tail -20; then
        echo "    Rust tests: PASSED"
    else
        echo "    Rust tests: FAILED" >&2
        failed=1
    fi

    echo "    Running forge test..."
    if (cd contracts && forge test 2>&1 | tail -20); then
        echo "    Solidity tests: PASSED"
    else
        echo "    Solidity tests: FAILED" >&2
        failed=1
    fi

    if [ "$failed" -eq 1 ]; then
        echo "==> Post-verification: SOME TESTS FAILED. Review output above." >&2
        return 1
    fi
    echo "==> Post-verification: ALL TESTS PASSED."
}

# ── Subcommands ──────────────────────────────────────────────────────────────

case "$1" in
  plan)
    shift
    GOAL="$*"
    echo "==> Planning: $GOAL"

    # Archive completed phases before creating new tasks
    if [ -x ".claude/scripts/archive-tasks.sh" ] && [ -f "$TASK_FILE" ]; then
        echo "==> Archiving completed phases..."
        .claude/scripts/archive-tasks.sh
    fi

    claude -p "You are the planner. Read the codebase and create a detailed task plan for: $GOAL

Break it into concrete tasks. For each task specify:
- id (T1, T2, ...)
- title
- assignee (leave empty — agents will claim them)
- status: pending
- files: which files/dirs it touches
- depends_on: which task IDs must finish first
- description: what to do

Write the plan to .claude/agent-state/tasks.json. Print a summary when done."
    echo ""
    echo "==> Plan written to $TASK_FILE"
    echo "==> Now run: ./agents.sh run agent-name-1 agent-name-2 ..."
    ;;

  run)
    shift
    if [ $# -eq 0 ]; then
      echo "Usage: ./agents.sh run <agent1> <agent2> ..."
      echo "Example: ./agents.sh run builder-a builder-b tester"
      exit 1
    fi

    # Pre-flight compilation check
    preflight_check || exit 1

    PIDS=()
    for AGENT in "$@"; do
      echo "==> Launching $AGENT in background..."

      # Per-agent build isolation for Cargo
      export CARGO_TARGET_DIR="target/agent-${AGENT}"

      claude -p "You are $AGENT.

PROTOCOL:
1. Read .claude/agent-state/tasks.json
2. Register yourself in agent_status and file_ownership
3. Find unclaimed tasks (status=pending, no assignee or assigned to you)
4. Claim one by setting assignee to your name and status to in_progress
5. Do the work
6. Run relevant tests (cargo test / forge test)
7. Mark task done in tasks.json
8. Re-read tasks.json and pick next unclaimed task
9. Repeat until all tasks are done

BUILD ISOLATION:
- Your Cargo target directory is: target/agent-${AGENT}
- Always use CARGO_TARGET_DIR=target/agent-${AGENT} when running cargo commands
- Use .claude/scripts/cargo-test.sh for tests (handles isolation automatically)
- Use .claude/scripts/forge-test.sh for Solidity tests (handles locking)

RULES:
- Before editing any file: run .claude/scripts/claim-file.sh claim <path> $AGENT
- If claim fails, skip that file and move to next task
- Only modify files listed in your task's files array
- After every edit, run cargo check or forge build to verify compilation
- Check file_ownership before editing any file — don't touch other agents' files
- If blocked, set status to blocked with reason, skip to next task
- If all your tasks are done or blocked, update agent_status to idle and stop" \
      > ".claude/agent-state/${AGENT}.log" 2>&1 &
      PIDS+=($!)
      echo "    PID: ${PIDS[-1]} | Log: .claude/agent-state/${AGENT}.log"
    done
    echo ""
    echo "==> All agents launched. Monitor with:"
    echo "    ./agents.sh status"
    echo "    tail -f .claude/agent-state/<agent>.log"
    echo ""
    echo "==> Waiting for all agents to finish..."
    for PID in "${PIDS[@]}"; do
      wait "$PID" 2>/dev/null || true
    done
    echo "==> All agents done."

    # Post-verification
    post_verify || echo "==> Fix failures before merging."
    ;;

  solo)
    # Interactive mode — opens a terminal you can talk to
    AGENT="${2:?Usage: ./agents.sh solo <agent-name>}"
    echo "==> Starting $AGENT interactively..."

    # Per-agent build isolation
    export CARGO_TARGET_DIR="target/agent-${AGENT}"

    claude "You are $AGENT.

BUILD ISOLATION: Your Cargo target directory is target/agent-${AGENT} (set via CARGO_TARGET_DIR).

Read .claude/agent-state/tasks.json first. Register yourself in agent_status and file_ownership. Then find and claim your tasks. Ask me if anything is unclear."
    ;;

  verify)
    echo "==> Running full verification suite..."
    post_verify
    ;;

  clean)
    echo "==> Cleaning per-agent build directories..."
    for DIR in target/agent-*; do
      if [ -d "$DIR" ]; then
        echo "    Removing $DIR..."
        rm -rf "$DIR"
      fi
    done
    echo "==> Cleaning file claim locks..."
    rm -rf .claude/locks/files/*.owner 2>/dev/null || true
    rm -rf .claude/agent-state/locks/ 2>/dev/null || true
    echo "==> Clean complete."
    ;;

  status)
    echo "==> Task board:"
    if [ -f "$TASK_FILE" ]; then
      # Show summary stats using python for accurate JSON parsing
      python3 -c "
import json, os
with open('$TASK_FILE') as f: data = json.load(f)
tasks = [t for p in data.get('phases', []) for t in p.get('tasks', [])]
total = len(tasks)
done = sum(1 for t in tasks if t.get('status') in ('done', 'completed', 'skipped'))
prog = sum(1 for t in tasks if t.get('status') == 'in_progress')
pend = sum(1 for t in tasks if t.get('status') == 'pending')
size = os.path.getsize('$TASK_FILE')
print(f'    Tasks: {total} total | {done} done | {prog} in-progress | {pend} pending')
print(f'    File: $TASK_FILE ({size // 1024}KB)')
" 2>/dev/null || echo "    (could not parse tasks.json)"
    else
      echo "(no tasks.json yet — run ./agents.sh plan first)"
    fi
    echo ""
    echo "==> File claims:"
    .claude/scripts/claim-file.sh list 2>/dev/null || echo "    (no claims)"
    echo ""
    echo "==> Agent logs:"
    for LOG in .claude/agent-state/*.log; do
      if [ -f "$LOG" ]; then
        AGENT=$(basename "$LOG" .log)
        LINES=$(wc -l < "$LOG" | tr -d ' ')
        echo "    $AGENT: $LINES lines (tail -f $LOG)"
      fi
    done
    ;;

  *)
    echo "Multi-agent launcher"
    echo ""
    echo "Commands:"
    echo "  ./agents.sh plan \"<goal>\"           Create a task plan"
    echo "  ./agents.sh run <a1> <a2> ...       Launch agents in parallel (headless)"
    echo "  ./agents.sh solo <agent>             Launch one agent interactively"
    echo "  ./agents.sh status                   Check task board and agent logs"
    echo "  ./agents.sh verify                   Run full test suite (cargo + forge)"
    echo "  ./agents.sh clean                    Remove per-agent build dirs and locks"
    echo ""
    echo "Example workflow:"
    echo "  ./agents.sh plan \"Add authentication to the API\""
    echo "  ./agents.sh run builder-a builder-b tester"
    echo "  ./agents.sh verify"
    echo ""
    echo "Or interactive:"
    echo "  ./agents.sh solo builder-a           # Terminal 1"
    echo "  ./agents.sh solo builder-b           # Terminal 2"
    ;;
esac
