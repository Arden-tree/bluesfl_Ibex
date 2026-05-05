#!/bin/bash

# Stop on unset vars, but don't stop on errors (we handle them manually)
set -u

# === Experiment parameters ===
#MODELS=("gpt-4o-mini" "gpt-4o" "gpt-5" "claude-3-5-sonnet")
#AGENT_TYPES=("open-ai" "open-ai" "open-ai" "claude")
MODELS=("gpt-4o")
AGENT_TYPES=("open-ai")
#MODELS=("claude-3-5-sonnet-20241022")
#AGENT_TYPES=("claude")
#MODELS=("gpt-4o-mini" "gpt-5" "claude-3-5-sonnet")
#AGENT_TYPES=("open-ai" "open-ai" "claude")
VOTE_TOTALS=(2)
VOTE_TOP_KS=(2)

# === Paths ===
DATA_PATH="/home/lzz/dac26/hdl_fl_data/dataset"
LOCALIZER="/home/lzz/RustProjects/sv-analysis/target/debug/sv_analysis_ab1_rm_values"
ENV_FILE="/home/lzz/RustProjects/sv-analysis/.env"
RESULT_DIR="./results/biosfl"
LOG_DIR="./logs"
COMMIT_HASH="$(git rev-parse --short HEAD)"
# Optional argument to append to PREFIX
SUFFIX="${1:-}"  # if no argument passed, use empty string
if [ -n "$SUFFIX" ]; then
  PREFIX="${COMMIT_HASH}_${SUFFIX}"
else
  PREFIX="${COMMIT_HASH}"
fi

mkdir -p "$RESULT_DIR"
mkdir -p "$LOG_DIR"

# === Track failed configs ===
FAILED_CONFIGS=()

# === Main Loop ===
for i in "${!MODELS[@]}"; do
  MODEL="${MODELS[$i]}"
  AGENT_TYPE="${AGENT_TYPES[$i]}"

  for VT in "${VOTE_TOTALS[@]}"; do
    for VK in "${VOTE_TOP_KS[@]}"; do
      CONFIG="biosfl_res_${PREFIX}_${MODEL}_vt${VT}_vk${VK}"
      LOG_FILE="${LOG_DIR}/${CONFIG}.log"

      echo "==========================================" | tee "$LOG_FILE"
      echo "Running experiment: $CONFIG" | tee -a "$LOG_FILE"
      echo "Agent: $AGENT_TYPE | Model: $MODEL | vote_total=$VT | vote_top_k=$VK" | tee -a "$LOG_FILE"
      echo "==========================================" | tee -a "$LOG_FILE"

      # --- Step 1 ---
      echo "[Step 1] Running fl_run_all.py..." | tee -a "$LOG_FILE"
      if ! python scripts/fl_run_all.py \
        --path="$DATA_PATH" \
        --localizer="$LOCALIZER" \
        --agent-type="$AGENT_TYPE" \
        --env="$ENV_FILE" \
        --model="$MODEL" \
        --vote-total="$VT" \
        --vote-top-k="$VK" \
        --no-sim \
        --prefix="$CONFIG" >> "$LOG_FILE" 2>&1; then
        echo "❌ Step 1 failed for $CONFIG" | tee -a "$LOG_FILE"
        FAILED_CONFIGS+=("$CONFIG (step1)")
        continue
      fi

      # --- Step 2 ---
      echo "[Step 2] Collecting results..." | tee -a "$LOG_FILE"
      if ! python scripts/collect_loc_results.py \
        --root="$DATA_PATH" \
        --output="${RESULT_DIR}/${CONFIG}_merged_results.json" \
        --prefix="$CONFIG" >> "$LOG_FILE" 2>&1; then
        echo "❌ Step 2 failed for $CONFIG" | tee -a "$LOG_FILE"
        FAILED_CONFIGS+=("$CONFIG (step2)")
        continue
      fi

      # --- Step 3 ---
      echo "[Step 3] Calculating metrics..." | tee -a "$LOG_FILE"
      if ! cargo run --bin cal_metric -- \
        --predictions="${RESULT_DIR}/${CONFIG}_merged_results.json" \
        --oracle="$DATA_PATH" >> "$LOG_FILE" 2>&1; then
        echo "❌ Step 3 failed for $CONFIG" | tee -a "$LOG_FILE"
        FAILED_CONFIGS+=("$CONFIG (step3)")
        continue
      fi

      echo "✅ Finished: $CONFIG" | tee -a "$LOG_FILE"
      echo
    done
  done
done

# === Summary ===
echo
echo "=========================================="
echo "🎯 Experiment Summary"
echo "=========================================="

if [ ${#FAILED_CONFIGS[@]} -eq 0 ]; then
  echo "✅ All configurations completed successfully!"
else
  echo "⚠️ Some configurations failed:"
  for FAIL in "${FAILED_CONFIGS[@]}"; do
    echo "  - $FAIL"
  done
  echo
  echo "Check logs in: $LOG_DIR/"
fi
