#!/usr/bin/env bash
set -e
set -u

ORACLE="/home/lzz/dac26/hdl_fl_data/dataset"
OUTDIR="./exps/metrics_out"
mkdir -p "$OUTDIR"

for pred_file in results/*/*.json; do
  base_name=$(basename "$pred_file" .json)
  out_file="$OUTDIR/${base_name}_metric.txt"

  echo "Running metric calc for: $pred_file → $out_file"

  if cargo run --quiet --bin cal_metric -- \
    --predictions="$pred_file" \
    --oracle="$ORACLE" \
    >"$out_file" 2>/dev/null; then
    echo "  OK"
  else
    echo "  FAILED (exit $?)"
  fi

done

echo "✅ All metrics computed and saved in '$OUTDIR/'"
