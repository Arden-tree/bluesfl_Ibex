#!/bin/bash
set -u

# === Ibex BluesFL Experiment ===
# Runs the full localization pipeline on Ibex bugs.
#
# Prerequisites:
#   1. Spike cosim built at $HOME/ibex-spike-cosim/install
#   2. Ibex cosim built (ibex_simple_system_cosim)
#   3. CoreMark ELF built
#   4. Mutator dataset generated at $DATASET_PATH
#
# Usage:
#   ./exps/run_ibex.sh                          # All bugs
#   ./exps/run_ibex.sh 0 1 2                    # Specific bug indices
#   ./exps/run_ibex.sh --no-sim 0               # Skip sim rerun

# === Configuration ===
BLUESFL_HOME="${BLUESFL_HOME:-/home/yuan/bluesfl}"
LOCALIZER="${LOCALIZER:-$BLUESFL_HOME/target/debug/sv_analysis}"
DATASET_PATH="${DATASET_PATH:-$BLUESFL_HOME/ibex_dataset}"
RESULT_DIR="./results/ibex"
LOG_DIR="./logs"
COMMIT_HASH="$(cd $BLUESFL_HOME && git rev-parse --short HEAD 2>/dev/null || echo 'unknown')"

# LLM configuration
MODEL="${MODEL:-deepseek-v4-pro}"
AGENT_TYPE="${AGENT_TYPE:-open-ai}"
VOTE_TOTAL="${VOTE_TOTAL:-1}"
VOTE_TOP_K="${VOTE_TOP_K:-1}"
ENV_FILE="${ENV_FILE:-$BLUESFL_HOME/.env}"

# === Argument parsing ===
BUGS=()
NO_SIM=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-sim)
            NO_SIM="--no-sim"
            ;;
        *)
            BUGS+=("$1")
            ;;
    esac
    shift
done

cd "$BLUESFL_HOME"
mkdir -p "$RESULT_DIR" "$LOG_DIR"

CONFIG="ibex_res_${COMMIT_HASH}_${MODEL}_vt${VOTE_TOTAL}_vk${VOTE_TOP_K}"
LOG_FILE="${LOG_DIR}/${CONFIG}.log"

echo "==========================================" | tee "$LOG_FILE"
echo "Ibex Experiment: $CONFIG" | tee -a "$LOG_FILE"
echo "Agent: $AGENT_TYPE | Model: $MODEL | vote_total=$VOTE_TOTAL | vote_top_k=$VOTE_TOP_K" | tee -a "$LOG_FILE"
echo "Dataset: $DATASET_PATH" | tee -a "$LOG_FILE"
echo "==========================================" | tee -a "$LOG_FILE"

# --- Step 1: Run BluesFL localization ---
echo "[Step 1] Running ibex_fl_run_all.py..." | tee -a "$LOG_FILE"
CMD="python3 scripts/ibex_fl_run_all.py \
    --path=$DATASET_PATH \
    --localizer=$LOCALIZER \
    --model=$MODEL \
    --agent-type=$AGENT_TYPE \
    --vote-total=$VOTE_TOTAL \
    --vote-top-k=$VOTE_TOP_K \
    --prefix=llm \
    --env=$ENV_FILE"

if [ -n "$NO_SIM" ]; then
    CMD="$CMD $NO_SIM"
fi

# Add bug range if specified
if [ ${#BUGS[@]} -gt 0 ]; then
    CMD="$CMD --start ${BUGS[0]} --end $((${BUGS[-1]} + 1))"
fi

if ! eval "$CMD" >> "$LOG_FILE" 2>&1; then
    echo "Step 1 failed" | tee -a "$LOG_FILE"
    exit 1
fi

# --- Step 2: Collect results ---
echo "[Step 2] Collecting results..." | tee -a "$LOG_FILE"
if ! python3 scripts/collect_loc_results.py \
    --root="$DATASET_PATH" \
    --output="${RESULT_DIR}/${CONFIG}_merged_results.json" \
    --prefix="llm" >> "$LOG_FILE" 2>&1; then
    echo "Step 2 failed" | tee -a "$LOG_FILE"
    exit 1
fi

# --- Step 3: Calculate metrics ---
echo "[Step 3] Calculating metrics..." | tee -a "$LOG_FILE"
if ! cargo run --bin cal_metric -- \
    --predictions="${RESULT_DIR}/${CONFIG}_merged_results.json" \
    --oracle="$DATASET_PATH" >> "$LOG_FILE" 2>&1; then
    echo "Step 3 failed" | tee -a "$LOG_FILE"
    exit 1
fi

echo "Finished: $CONFIG" | tee -a "$LOG_FILE"
echo "Check results in: $RESULT_DIR/"
