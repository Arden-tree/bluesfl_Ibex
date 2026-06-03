# NutShell BluesFL 端到端测试指南

> 基于 U6_jalr_bit0_not_cleared (bid=5841, ALU) 成功运行编写
> 日期: 2026-06-03
> 分支: `feat/nutshell-pipeline`

---

## 1. 概览

BluesFL 在 NutShell 上的端到端流程分 5 步：

```
Step 1: make verilog      → 生成带 bug 的分文件 RTL (Chisel → SystemVerilog)
Step 2: make emu           → 编译 Verilator 仿真器 (含 DiffTest + FST 波形)
Step 3: 仿真               → emu 运行测试程序, 生成 FST 波形 + 仿真日志
Step 4: test_analysis      → 解析仿真日志, 生成 test_info.json
Step 5: sv_analysis        → RTL 数据流分析 + LLM 引导追踪 → 定位 bug block
```

### U6 成功结果

```
bug_id: U6_backend
block:  ALU Assign, bid=5841, score=1.0
tokens: 227K input + 20K output = 247K total
model:  deepseek-chat
```

---

## 2. 前置条件

### 2.1 仓库

| 仓库 | 路径 | 用途 |
|------|------|------|
| BluesFL | `/home/yuan/bluesfl` | 定位工具 (sv_analysis) |
| NutShell | `/home/yuan/NutShell` | 处理器源码 (Chisel) |
| nutshell-sbfl | `/home/yuan/nutshell-sbfl` | Bug 数据集 (patch + 测试用例) |

### 2.2 依赖

```bash
# RISC-V 交叉编译器 (汇编测试用例)
riscv64-linux-gnu-gcc 或 riscv64-unknown-elf-gcc

# JDK (Chisel 编译)
java >= 11

# Verilator (RTL 仿真)
verilator >= 4.200

# Rust (编译 sv_analysis)
rustc >= 1.70
```

### 2.3 编译 sv_analysis

```bash
cd /home/yuan/bluesfl
cargo build --bin sv_analysis    # 生成 target/debug/sv_analysis
```

---

## 3. 配置文件

### 3.1 `.env` (LLM API 配置)

路径: `/home/yuan/bluesfl/.env`

```bash
# === LLM Configuration ===
AGENT_TYPE=open-ai
MODEL=deepseek-v4-flash                  # 1M context, 推荐
API_KEY=sk-xxx                           # DeepSeek API key
API_BASE=https://api.deepseek.com
```

**模型选择**:
| 模型 | Context Window | Token 费用 | 推荐 |
|------|---------------|-----------|------|
| `deepseek-v4-flash` | 1M | 低 | 推荐 |
| `deepseek-chat` | ~256K (API实际) | 中 | 可用, 大模块可能超限 |

> NutShell ALU 模块的 prompt 实际用了 ~227K tokens, 所以 `deepseek-chat` 勉强够用,
> 但推荐 `deepseek-v4-flash` 避免偶发超限。

### 3.2 `signal_name_map.json`

路径: `/home/yuan/bluesfl/signal_name_map.json` (7375 行)

Chisel 生成的 Verilog 信号名与源码名不同, 此文件提供映射。
sv_analysis 自动从项目根目录读取 (`SV_ANALYSIS_HOME`)。

### 3.3 Oracle 文件

路径: `/home/yuan/bluesfl/scripts/nutshell_oracle/`

每个 bug 一个 JSON + `oracle_info.json` 汇总:
```json
{
  "bid": 5841,
  "module_name": "ALU",
  "scope_name": "TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend.exu.alu",
  "description": "JALR target bit0 not cleared",
  "bug_file": "src/main/scala/nutcore/backend/fu/ALU.scala"
}
```

---

## 4. 逐步操作 (以 U6 为例)

### Step 1: 应用 Patch 并生成 Verilog

```bash
cd /home/yuan/nutshell-sbfl

# 恢复干净状态
git checkout -- .

# 应用 bug patch
git apply patch/U6_jalr_bit0_not_cleared.patch

# Chisel → 分文件 SystemVerilog
# 生成 build/rtl/*.sv (NutCore.sv ~1,665 行, 不是扁平化的 34K 行)
make verilog
```

**验证**:
```bash
wc -l build/rtl/NutCore.sv
# 应输出 ~1,665 行。如果是 34,000+ 行说明用了预构建的扁平化 RTL, 不对。
```

### Step 2: 编译 emu (Verilator + FST)

```bash
cd /home/yuan/nutshell-sbfl

# 编译仿真器, 启用 FST 波形输出
EMU_TRACE=fst make emu RTL_SUFFIX=sv
```

**输出**: `build/emu` 可执行文件

**注意**: `RTL_SUFFIX=sv` 确保使用分文件 RTL 而非 Chisel 中间文件。

### Step 3: 运行仿真

```bash
cd /home/yuan/nutshell-sbfl

# 汇编测试用例
riscv64-linux-gnu-gcc -nostdlib -nostartfiles -T case/link.ld \
    -o build/U6_test.bin case/U6_jalr_bit0_not_cleared.S

# 运行仿真 (DiffTest 对比)
./build/emu \
    -i build/U6_test.bin \
    --diff build/riscv64-nemu-interpreter-so \
    --dump-commit-trace \
    --dump-wave \
    --wave-path=build/U6_wave.fst \
    2>&1 | tee build/emu_output.log
```

**预期输出**:
```
Core 0: ABORT at pc = 0x80000012
Core-0 instrCnt = 4, cycleCnt = 1,536
[03] commit pc 000000008000000c inst 00008067 wen 0 dst 00 data 0000000080000011
```

**输出文件**:
- `build/U6_wave.fst` — FST 波形文件
- `build/emu_output.log` — 仿真日志

### Step 4: 生成 test_info.json

```bash
cd /home/yuan/bluesfl

python3 scripts/nutshell_test_analysis.py \
    --emu-log /home/yuan/nutshell-sbfl/build/emu_output.log \
    --bug-id U6_jalr_bit0_not_cleared \
    --output e2e_results/nutshell/U6_jalr_bit0_not_cleared/test_info.json
```

**输出** (`test_info.json`):
```json
{
  "bug_id": "U6_jalr_bit0_not_cleared",
  "start_scope": "TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend",
  "start_sig": "io_redirect_target",
  "start_time": 0,
  "test_info": "ABORT at pc=0x80000012 after 4 instructions, ...",
  "time_bound": 0,
  "time_step": 2,
  "top_module": "SimTop",
  "top_scope": "TOP.SimTop.cpu.soc.nutcore"
}
```

**重要**: `start_time` 和 `time_bound` 需要手动指定!

#### start_time 探测方法

NutShell FST 时间单位 ≠ cycle × 2, 必须手动确定:

1. 先用 `start_time=0` 运行一次 sv_analysis (会很快因为不匹配任何波形数据)
2. 观察日志中信号值首次出现异常的时间
3. 或者用 GTKWave 打开 FST 文件, 查看 `io_redirect_target` 信号首次出错的时间

U6 已知值: `start_time=515`, `time_bound=485`

### Step 5: 运行 sv_analysis

```bash
cd /home/yuan/bluesfl

# 设置环境变量 (signal_name_map.json 查找路径)
export SV_ANALYSIS_HOME=/home/yuan/bluesfl

./target/debug/sv_analysis \
    --bug-id U6_backend \
    --agent-type open-ai \
    --model deepseek-chat \
    --project-path /home/yuan/NutShell/build/rtl \
    --include-paths /home/yuan/NutShell/build/rtl,/home/yuan/NutShell/build/generated-src \
    --rm-params-path /dev/null \
    --coverage-path /home/yuan/NutShell/build/verilator-compile \
    --wave-path /home/yuan/NutShell/build/U6_wave.fst \
    --top-module SimTop \
    --top-scope "TOP.SimTop.cpu.soc.nutcore" \
    --start-scope "TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend" \
    --start-sig io_redirect_target \
    --start-time 515 \
    --time-bound 485 \
    --time-step 2 \
    --output-path ./e2e_results/U6_t515 \
    --vote-top-k 1 \
    --vote-total 2 \
    --test-info "Backend redirect_target is wrong"
```

**参数说明**:

| 参数 | 值 | 说明 |
|------|-----|------|
| `--bug-id` | `U6_backend` | 结果文件名后缀 |
| `--agent-type` | `open-ai` | LLM 后端类型 |
| `--model` | `deepseek-chat` | 模型名 (从 .env 读取 API 配置) |
| `--project-path` | `.../build/rtl` | 分文件 RTL 目录 |
| `--include-paths` | `rtl,generated-src` | 必须包含 generated-src (DifftestMacros.svh) |
| `--wave-path` | `.../U6_wave.fst` | FST 波形文件 |
| `--start-scope` | `...backend` | 追踪起点模块 |
| `--start-sig` | `io_redirect_target` | 追踪起点信号 |
| `--start-time` | `515` | FST 时间单位, 需手动确定 |
| `--time-bound` | `485` | 回溯上限 (= start_time - 30) |
| `--time-step` | `2` | NutShell 时钟周期 = 2 个时间单位 |
| `--vote-total` | `2` | LLM 投票次数 |
| `--vote-top-k` | `1` | Top-K 选择 |
| `--test-info` | 描述文本 | LLM 提示词中的场景描述 |

**运行耗时** (U6 参考值):

| 阶段 | 耗时 |
|------|------|
| parse_sv (119 files) | ~337s |
| BlockManager (数据流分析) | ~43s |
| ModuleChecker + BFS 追踪 | <1s |
| LLM 投票 (2 votes × 多轮) | ~120s |
| **总计** | ~8-10 min |

**输出文件**:
```
e2e_results/U6_t515/
├── llm_loc_results_U6_backend.json    # 定位结果 (choices 排名)
├── suspicious_blocks.json             # 可疑 block 列表
├── suspicious_modules.json            # 可疑模块列表
└── trace.json                         # 完整追踪路径
```

**成功输出** (`llm_loc_results_U6_backend.json`):
```json
{
  "bug_id": "U6_backend",
  "token_usage": { "input_tokens": 227332, "output_tokens": 20226, "total_tokens": 247558 },
  "choices": [
    { "module_name": "ALU", "block_id": 5841, "score": 1.0 }
  ]
}
```

---

## 5. 自动化脚本 (批量测试)

### 5.1 一键脚本: `nutshell_fl_run_all.py`

```bash
cd /home/yuan/bluesfl

python3 scripts/nutshell_fl_run_all.py \
    --nutshell-path /home/yuan/nutshell-sbfl \
    --localizer ./target/debug/sv_analysis \
    --output-dir ./e2e_results \
    --agent-type open-ai \
    --model deepseek-v4-flash \
    --bug U6_jalr_bit0_not_cleared \
    --start-time 515
```

**参数**:
- `--bug` — 指定 bug ID, 不传则处理所有 patch
- `--start-time` — 覆盖 test_info.json 中的 start_time
- `--skip-build` — 跳过 patch + build
- `--skip-sim` — 跳过仿真
- `--skip-asm` — 跳过汇编 (用预编译二进制)
- `--skip-analysis` — 只生成 test_info, 不跑 sv_analysis
- `--prefix llm` — 结果目录前缀 (默认 llm)

### 5.2 批量实验: `run_nutshell.sh`

```bash
cd /home/yuan/bluesfl

# 单个 bug
START_TIME=515 ./exps/run_nutshell.sh --skip-build U6

# 多个 bug
START_TIME=515 ./exps/run_nutshell.sh U6 U1 M1

# 跳过仿真 (已有 FST)
START_TIME=515 ./exps/run_nutshell.sh --skip-sim U6
```

> 注意: `run_nutshell.sh` 默认模型是 `gpt-4o-mini`, 需修改脚本中的 `MODELS` 数组改为 `deepseek-v4-flash`。

---

## 6. 计算 Metric

### 6.1 收集结果

```bash
cd /home/yuan/bluesfl

python3 scripts/collect_loc_results.py \
    --root ./e2e_results \
    --output ./e2e_results/merged_results.json \
    --prefix llm
```

### 6.2 计算 Top-1/Top-5/Top-10

```bash
cd /home/yuan/bluesfl

cargo run --bin cal_metric -- \
    --predictions ./e2e_results/merged_results.json \
    --oracle scripts/nutshell_oracle \
    --model-name deepseek-v4-flash
```

**Oracle 目录结构** (`scripts/nutshell_oracle/`):
```
nutshell_oracle/
├── oracle_info.json              # 汇总所有 bug 的 ground truth
├── U6_jalr_bit0_not_cleared.json # 单个 bug oracle
├── U1_addiw_signext.json
├── M1_div_by_zero.json
└── ...
```

---

## 7. 关键脚本索引

| 文件 | 功能 | 入口 |
|------|------|------|
| `scripts/nutshell_test_analysis.py` | Step 3: 解析 emu 日志 → test_info.json | `python3` |
| `scripts/nutshell_fl_run_all.py` | Step 4: 批量 build→sim→analyze | `python3` |
| `exps/run_nutshell.sh` | 实验入口: 跑 pipeline + 收集 + 计算 metric | `bash` |
| `src/bin/sv_analysis.rs` | Step 5: RTL 分析 + LLM 追踪 | `cargo build` |
| `src/bin/cal_metric.rs` | 计算 Top-K accuracy | `cargo run --bin cal_metric` |
| `scripts/nutshell_oracle/generate_oracles.py` | (重新)生成 oracle JSON | `python3` |

---

## 8. NutShell Scope 格式

NutShell 分文件 RTL 的 scope 比论文 (Ibex) 多一层:

```
# Ibex (论文)
TOP.ibex_simple_system.u_top.<module>

# NutShell (分文件 RTL)
TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.<module>
                          ^^^^^^^^^^^^^^^^^ 多了这层
```

常用 scope:
```
TOP_SCOPE   = TOP.SimTop.cpu.soc.nutcore
BACKEND     = ...cpu.soc.nutcore.backend
ALU         = ...backend.exu.alu
BRU         = ...backend.exu.bru
MDU         = ...backend.exu.mdu
CSR         = ...backend.exu.csr
LSU         = ...backend.exu.lsu
ISU         = ...backend.isu
IFU         = ...frontend
ICache      = ...mem.icache
DCache      = ...mem.dcache
TLB         = ...mem.EmbeddedTLB
```

---

## 9. 每个 Bug 的 start_time 参考

> start_time 需要手动探测或从 FST 波形中确定。
> 以下为已知值, 未测试的标为 `?`。

| Bug ID | Oracle Module | start_time | start_sig |
|--------|--------------|------------|-----------|
| U6_jalr_bit0_not_cleared | ALU | 515 | io_redirect_target |
| U1_addiw_signext | ALU | ? | ? |
| U7_lb_lbu_ext_swap | UnpipelinedLSU | ? | ? |
| M1_div_by_zero | MDU | ? | ? |
| D1_sub_sra_decode | RVI | ? | ? |
| C1_alu_forwarding_disabled | ISU | ? | ? |
| C3_branch_no_redirect | ALU | ? | ? |
| RE1_branch_misaligned | BRU | ? | ? |
| P4_mepc_save_error | CSR | ? | ? |
| P6_mret_privilege_mode | CSR | ? | ? |

其他 bug 的 start_time 需按以下流程探测:
1. `--start-time 0` 运行 sv_analysis
2. 在日志中找到目标信号首次出错的时间
3. 用确定的时间重新运行

---

## 10. 常见问题

### Q: NutCore.sv 有 34,000 行而不是 1,600 行
**A**: 用了 nutshell-sbfl 预构建的扁平化 RTL。必须用 `make verilog` 重新生成分文件 RTL。

### Q: DifftestMacros.svh 找不到
**A**: `--include-paths` 必须包含 `build/generated-src`。

### Q: LLM context window limit
**A**: 换用 `deepseek-v4-flash` (1M context), 或减少 `input_wave` 中的信号数量。

### Q: token_usage=0, choices 为空
**A**: LLM 没被调用, 通常是 trace 没走到 Assign/Always block 就停止了。检查 start_time 是否正确。

### Q: SV_ANALYSIS_HOME 未设置
**A**: `export SV_ANALYSIS_HOME=/home/yuan/bluesfl` (signal_name_map.json 所在目录)。

### Q: 追踪 trace 太短 (只有几步)
**A**: 检查 `--start-scope` 和 `--start-sig` 是否匹配仿真中的实际错误。start_sig 一般是 `io_redirect_target` (PC mismatch)。
