#!/bin/bash
set -u

# === NutShell BluesFL Experiment ===
# Runs the full pipeline (Steps 3-5) for NutShell bugs.
#
# Usage:
#   ./exps/run_nutshell.sh                          # All bugs
#   ./exps/run_nutshell.sh U6                       # Single bug
#   ./exps/run_nutshell.sh U6 U1 M1                 # Multiple bugs
#   ./exps/run_nutshell.sh --skip-build U6           # Skip rebuild
#
# For a single bug with manual start_time:
#   START_TIME=515 ./exps/run_nutshell.sh --skip-sim U6

# === Configuration ===
NUTSHELL_PATH="${NUTSHELL_PATH:-/home/yuan/nutshell-sbfl}"
LOCALIZER="${LOCALIZER:-$(dirname "$0")/../target/debug/sv_analysis}"
RESULT_DIR="./e2e_results/nutshell"
LOG_DIR="./logs"
COMMIT_HASH="$(git rev-parse --short HEAD 2>/dev/null || echo 'unknown')"

# LLM configuration
MODELS=("gpt-4o-mini")
AGENT_TYPES=("open-ai")
VOTE_TOTALS=(2)
VOTE_TOP_KS=(1)

# Optional: override start_time (FST time unit)
START_TIME="${START_TIME:-}"

# === Argument parsing ===
BUGS=()
SKIP_FLAGS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build|--skip-sim|--skip-asm|--skip-analysis)
            SKIP_FLAGS+=("$1")
            ;;
        *)
            BUGS+=("$1")
            ;;
    esac
    shift
done

mkdir -p "$RESULT_DIR"
mkdir -p "$LOG_DIR"

FAILED_CONFIGS=()

for i in "${!MODELS[@]}"; do
    MODEL="${MODELS[$i]}"
    AGENT_TYPE="${AGENT_TYPES[$i]}"

    for VT in "${VOTE_TOTALS[@]}"; do
        for VK in "${VOTE_TOP_KS[@]}"; do
            CONFIG="nutshell_res_${COMMIT_HASH}_${MODEL}_vt${VT}_vk${VK}"
            LOG_FILE="${LOG_DIR}/${CONFIG}.log"

            echo "==========================================" | tee "$LOG_FILE"
            echo "NutShell Experiment: $CONFIG" | tee -a "$LOG_FILE"
            echo "Agent: $AGENT_TYPE | Model: $MODEL | vote_total=$VT | vote_top_k=$VK" | tee -a "$LOG_FILE"
            echo "==========================================" | tee -a "$LOG_FILE"

            # --- Step 1: Run pipeline ---
            echo "[Step 1] Running nutshell_fl_run_all.py..." | tee -a "$LOG_FILE"
            CMD="python3 scripts/nutshell_fl_run_all.py \
                --nutshell-path=$NUTSHELL_PATH \
                --localizer=$LOCALIZER \
                --output-dir=$RESULT_DIR \
                --agent-type=$AGENT_TYPE \
                --model=$MODEL \
                --vote-total=$VT \
                --vote-top-k=$VK \
                --prefix=llm"

            # Add bug selection
            if [ ${#BUGS[@]} -gt 0 ]; then
                CMD="$CMD --bug ${BUGS[*]}"
            fi

            # Add skip flags
            for flag in "${SKIP_FLAGS[@]}"; do
                CMD="$CMD $flag"
            done

            # Add start_time override if set
            if [ -n "$START_TIME" ]; then
                CMD="$CMD --start-time=$START_TIME"
            fi

            if ! eval "$CMD" >> "$LOG_FILE" 2>&1; then
                echo "Step 1 failed for $CONFIG" | tee -a "$LOG_FILE"
                FAILED_CONFIGS+=("$CONFIG (step1)")
                continue
            fi

            # --- Step 2: Collect results ---
            echo "[Step 2] Collecting results..." | tee -a "$LOG_FILE"
            if ! python3 scripts/collect_loc_results.py \
                --root="$RESULT_DIR" \
                --output="${RESULT_DIR}/${CONFIG}_merged_results.json" \
                --prefix="llm" >> "$LOG_FILE" 2>&1; then
                echo "Step 2 failed for $CONFIG" | tee -a "$LOG_FILE"
                FAILED_CONFIGS+=("$CONFIG (step2)")
                continue
            fi

            # --- Step 3: Calculate metrics ---
            echo "[Step 3] Calculating metrics..." | tee -a "$LOG_FILE"
            ORACLE_DIR="scripts/nutshell_oracle"
            if ! cargo run --bin cal_metric -- \
                --predictions="${RESULT_DIR}/${CONFIG}_merged_results.json" \
                --oracle="$ORACLE_DIR" >> "$LOG_FILE" 2>&1; then
                echo "Step 3 failed for $CONFIG" | tee -a "$LOG_FILE"
                FAILED_CONFIGS+=("$CONFIG (step3)")
                continue
            fi

            echo "Finished: $CONFIG" | tee -a "$LOG_FILE"
            echo
        done
    done
done

# === Summary ===
echo
echo "=========================================="
echo "Experiment Summary"
echo "=========================================="
if [ ${#FAILED_CONFIGS[@]} -eq 0 ]; then
    echo "All configurations completed successfully!"
else
    echo "Some configurations failed:"
    for FAIL in "${FAILED_CONFIGS[@]}"; do
        echo "  - $FAIL"
    done
    echo
    echo "Check logs in: $LOG_DIR/"
fi
