#!/bin/bash
# BluesFL runner for NutShell processor
# Usage: ./run_bluesfl_nutshell.sh --bug-id <id> --start-sig <signal> --start-time <time> --test-info <info>

set -e

NUTSHELL_HOME=${NUTSHELL_HOME:-/home/yuan/NutShell}
BLUESFL_HOME=${BLUESFL_HOME:-/home/yuan/bluesfl}
RTL_DIR=$NUTSHELL_HOME/build/rtl
WAVE_PATH=${WAVE_PATH:-$NUTSHELL_HOME/build/bug_test.fst}

# Build BluesFL if needed
if [ ! -f "$BLUESFL_HOME/target/debug/sv-analysis" ]; then
    echo "Building BluesFL..."
    cd $BLUESFL_HOME && cargo build 2>&1 | tail -3
fi

# Default arguments for NutShell
TOP_MODULE="SimTop"
TOP_SCOPE="TOP.SimTop.cpu.soc.nutcore"
AGENT_TYPE=${AGENT_TYPE:-open-ai}
MODEL=${MODEL:-gpt-4o}

# Parse arguments
BUG_ID=""
START_SIG=""
START_TIME=""
START_SCOPE=""
TEST_INFO=""
TIME_BOUND=15
TIME_STEP=2

while [[ $# -gt 0 ]]; do
    case $1 in
        --bug-id) BUG_ID="$2"; shift 2 ;;
        --start-sig) START_SIG="$2"; shift 2 ;;
        --start-time) START_TIME="$2"; shift 2 ;;
        --start-scope) START_SCOPE="$2"; shift 2 ;;
        --test-info) TEST_INFO="$2"; shift 2 ;;
        --time-bound) TIME_BOUND="$2"; shift 2 ;;
        --time-step) TIME_STEP="$2"; shift 2 ;;
        --wave-path) WAVE_PATH="$2"; shift 2 ;;
        --model) MODEL="$2"; shift 2 ;;
        *) echo "Unknown arg: $1"; exit 1 ;;
    esac
done

# Set default start scope to backend EXU if not specified
if [ -z "$START_SCOPE" ]; then
    START_SCOPE="$TOP_SCOPE.backend.exu"
fi

echo "=== BluesFL for NutShell ==="
echo "Bug ID: $BUG_ID"
echo "Top Module: $TOP_MODULE"
echo "Top Scope: $TOP_SCOPE"
echo "Start Signal: $START_SIG"
echo "Start Time: $START_TIME"
echo "Wave: $WAVE_PATH"
echo ""

cd $BLUESFL_HOME

cargo run --bin sv-analysis -- \
    --bug-id="$BUG_ID" \
    --agent-type="$AGENT_TYPE" \
    --model="$MODEL" \
    --project-path="$RTL_DIR" \
    --wave-path="$WAVE_PATH" \
    --top-module="$TOP_MODULE" \
    --top-scope="$TOP_SCOPE" \
    --start-scope="$START_SCOPE" \
    --start-sig="$START_SIG" \
    --start-time="$START_TIME" \
    --test-info "$TEST_INFO" \
    --time-bound="$TIME_BOUND" \
    --time-step="$TIME_STEP"
