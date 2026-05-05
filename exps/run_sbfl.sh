#!/usr/bin/env bash
set -u  # treat unset variables as errors

metrics=(
  tarantula
  ochiai
  jaccard
  dstar
)

mkdir -p exp_logs

for metric in "${metrics[@]}"; do
  echo "=============================="
  echo "Running metric: $metric"
  echo "=============================="

  if python ../scripts/sbfl_run_all.py \
      --path=/home/lzz/dac26/hdl_fl_data/dataset \
      --localizer=/home/lzz/RustProjects/sv-analysis/target/debug/sbfl \
      --prefix="sbfl_res_${metric}" \
      --metric="$metric" \
      -t 16 \
      >"exp_logs/${metric}.out" 2>"exp_logs/${metric}.err"; then
    echo "[OK] Metric $metric completed successfully."
  else
    echo "[ERROR] Metric $metric failed — see exp_logs/${metric}.err"
    continue  # move on to the next metric
  fi

done

echo "All metrics processed."
