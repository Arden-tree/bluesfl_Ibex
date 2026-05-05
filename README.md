# 🎷 Debug Like a Human: Scaling LLM-based Fault Localization to Processor Design via Block-Level Instruction-Oriented Slicing

> [!NOTE]
> We recently refactored BluesFL into **[sv-analyzer](https://github.com/pointerliu/sv-analyzer)** — a cleaner codebase with the core Blues slicing engine, plus TUI, CLI, MCP server, and VS Code extension support for IDE/agent integration. Note that `sv-analyzer` is still in active development and may not yet be stable.

This artifact includes:

- the code for our BluesFL tool in: `src/bin/sv_analysis`
- bug inject tool in : `src/bin/mutator`
- metric calculator: `src/cal_metric`
- our dataset: `dataset`
- results of BluesFL and other baselines: `results`
- run experiment scripts: `exps` and `scripts`

> [!NOTE]
> 
> You may find the name "BiosFL" in this artifact, which was the initial name for our "BluesFL". Since BIOS sounds very similar to the Basic I/O System (BIOS), we decided to update it 
> in our paper.

## 1. Installation

### Environment

We only test BluesFL in Ubuntu 22.04.

```shell
export SV_ANALYSIS_HOME=$(pwd)

cp .env.example .env
# fill in your api key in .env
```

### Python

Please make sure that you have Python in version 3.12 or newer installed. We have tested all scripts with
`Python 3.12.2`.

### Rust

Please make sure you have [a recent version of Rust installed](https://www.rust-lang.org/tools/install).
Update your version to the latest (most commonly through `rustup update`). We build `BluesFL` with
`rustc 1.89.0-nightly (5d707b07e 2025-06-02)`.

### Verilator

We are using a custom Verilator that dumps AST information after removing all parameter-controlled code.
But the modification is very little, so you can download the source of [Verilator](https://github.com/verilator/verilator)
and apply our patch to version `5.037`, then compile and install it.

```shell
git apply doc/patches/0001-verilator-dump-rm-params.patch

# Prerequisites:
#sudo apt-get install git help2man perl python3 make autoconf g++ flex bison ccache
#sudo apt-get install libgoogle-perftools-dev numactl perl-doc
#sudo apt-get install libfl2  # Ubuntu only (ignore if it gives an error)
#sudo apt-get install libfl-dev  # Ubuntu only (ignore if it gives an error)
#sudo apt-get install zlibc zlib1g zlib1g-dev  # Ubuntu only (ignore if it gives an error)

git clone https://github.com/verilator/verilator
cd verilator
git checkout v5.037
git apply $SV_ANALYSIS_HOME/doc/patches/0001-verilator-dump-rm-params.patch

autoconf         # Create ./configure script
./configure      # Configure and create Makefile
make -j `nproc`  # Build Verilator itself (if error, try just 'make')
sudo make install
```

```shell
$ verilator --version                                              
Verilator 5.037 devel rev UNKNOWN.REV (mod)
```

## 2. Setup Dataset

See the full guide in [setup_dataset.md](doc/1.setup_dataset.md).

## 3. Artifact Setup

Building BluesFL is very simple, just `cargo build`, it will build the final bin at `target/debug/sv_analysis`.

```shell
cd $SV_ANALYSIS_HOME
git submodule update --init --recursive
cargo build
```

## 4. Artifact Instructions

### Run BluesFL & Baselines

See the full guide in [run_bluesfl.md](doc/3.run_biosfl.md) and [baselines.md](doc/2.baselines.md).

After running all, results are all saved to `results` folder in respective folders.

### Calculate Metric

Run script `exps/metric_all.sh`, and all metric results will be saved in `exps/metrics_out`.

### Generate Tables

We provide scripts to plot Figure 4, 5 and 7 in our experiment section from the data we previously collected.

- Figure 4:

```shell
# After running BluesFL, a file `blocks.json` will be generated.
python scripts/plot_blocks.py 
```

- Figure 5:

```shell
python scripts/plot_trace_size_dis.py --root /home/lzz/dac26/hdl_fl_data/dataset \ 
    --prefix biosfl_res_gpt-4o_vt2_vk2 biosfl_res_b268051_ablation_rm_exe_path_gpt-4o_vt2_vk2 biosfl_res_fd36321_ablation_rm_sig_values_gpt-4o_vt2_vk2 \
    --name-map BluesFL "w/o instruction path" "w/o signal values"
``` 

- Figure 7:

```shell
python scripts/cal_hit_or_not.py 
```
