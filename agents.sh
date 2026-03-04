#!/bin/bash
# Multi-agent launcher for world-zk-compute
# Usage:
#   ./agents.sh plan "Build the XGBoost verifier"    — create a plan
#   ./agents.sh run builder-a builder-b               — launch agents in parallel
#   ./agents.sh status                                — check what agents are doing
#   ./agents.sh solo builder-a                        — launch one agent interactively

set -e
TASK_FILE=".claude/agent-state/tasks.json"

case "$1" in
  plan)
    shift
    GOAL="$*"
    echo "==> Planning: $GOAL"
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
    PIDS=()
    for AGENT in "$@"; do
      echo "==> Launching $AGENT in background..."
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

RULES:
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
    ;;

  solo)
    # Interactive mode — opens a terminal you can talk to
    AGENT="${2:?Usage: ./agents.sh solo <agent-name>}"
    echo "==> Starting $AGENT interactively..."
    claude "You are $AGENT.

Read .claude/agent-state/tasks.json first. Register yourself in agent_status and file_ownership. Then find and claim your tasks. Ask me if anything is unclear."
    ;;

  status)
    echo "==> Task board:"
    cat "$TASK_FILE" 2>/dev/null || echo "(no tasks.json yet — run ./agents.sh plan first)"
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
    echo ""
    echo "Example workflow:"
    echo "  ./agents.sh plan \"Add authentication to the API\""
    echo "  ./agents.sh run builder-a builder-b tester"
    echo ""
    echo "Or interactive:"
    echo "  ./agents.sh solo builder-a           # Terminal 1"
    echo "  ./agents.sh solo builder-b           # Terminal 2"
    ;;
esac
