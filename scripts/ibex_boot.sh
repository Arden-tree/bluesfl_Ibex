#!/bin/bash
source ~/.zshrc

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

export PKG_CONFIG_PATH=/home/lzz/exp_wkdir/ibex_test/ibex-spike-cosim/lib/pkgconfig:$PKG_CONFIG_PATH

rm -rf build

fusesoc --cores-root=. run --target=sim --setup --build lowrisc:ibex:ibex_simple_system_cosim --RV32E=0 --RV32M=ibex_pkg::RV32MFast > lint.log

# Check if the executable exists before proceeding
if [ ! -f "build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/Vibex_simple_system" ]; then
  echo "Error: Vibex_simple_system executable not found after build"
  exit 1
fi

cd build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator || exit 1

# set time bound to executable file. 
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
