#!/bin/bash
# Ibex co-simulation boot script for BluesFL mutator.
# Called by the mutator to: build mutated Ibex → run CoreMark → detect mismatch.
#
# Usage: ibex_boot.sh <ibex_project_dir>
# Exit 0 = mismatch found (bug triggered), Exit 1 = no mismatch

set -e

if [ -z "$1" ]; then
  echo "Error: No path provided."
  exit 1
fi

if [ ! -d "$1" ]; then
  echo "Error: Directory '$1' does not exist."
  exit 1
fi

if [ ! -f "$1/ibex_core.core" ]; then
  echo "Error: File 'ibex_core.core' not found in '$1'."
  exit 1
fi

cd "$1" || exit 1

# Spike co-simulator must be built and installed
SPIKE_PREFIX="${SPIKE_COSIM_PREFIX:-$HOME/ibex-spike-cosim/install}"
export PKG_CONFIG_PATH="$SPIKE_PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
export LD_LIBRARY_PATH="$SPIKE_PREFIX/lib:${LD_LIBRARY_PATH:-}"

rm -rf build

fusesoc --cores-root=. run --target=sim --setup --build \
    lowrisc:ibex:ibex_simple_system_cosim \
    --RV32E=0 --RV32M=ibex_pkg::RV32MFast > lint.log

# Check if the executable exists before proceeding
if [ ! -f "build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/Vibex_simple_system" ]; then
  echo "Error: Vibex_simple_system executable not found after build"
  exit 1
fi

cd build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator || exit 1

# Run CoreMark with co-simulation checking (timeout 1m)
output=$(timeout 1m ./Vibex_simple_system --meminit=ram,../../../examples/sw/benchmarks/coremark/coremark.elf -t)

echo "$output"

if echo "$output" | grep -q "FAILURE: Co-simulation mismatch at time"; then
  echo "$output" > ../../../mismatch_log.txt
  echo "Found mismatch"
  exit 0
else
  echo "Not found mismatch"
  exit 1
fi
