# BluesFL Ibex 测试框架使用指南

端到端复现 BluesFL 论文的完整测试流程。

论文：*Debug Like a Human: Scaling LLM-based Fault Localization to Processor Design via Block-Level Instruction-Oriented Slicing* (DAC '26)

---

## 目录

1. [环境准备](#1-环境准备)
2. [测试流程总览](#2-测试流程总览)
3. [逐步操作指南](#3-逐步操作指南)
4. [批量测试脚本](#4-批量测试脚本)
5. [架构说明](#5-架构说明)
6. [常见问题](#6-常见问题)

---

## 1. 环境准备

### 1.1 代码仓库

```bash
# BluesFL 代码仓
git clone https://github.com/pointerliu/bluesfl.git
cd bluesfl
git checkout feat/ibex-testing-framework

# Ibex RISC-V 处理器
git clone https://github.com/lowRISC/ibex.git ~/ibex

# Spike ISS（ibex_cosim fork，用作参考模型）
git clone https://github.com/lowRISC/ibex-spike-cosim.git ~/ibex-spike-cosim
cd ~/ibex-spike-cosim
mkdir build && cd build
cmake .. -DCMAKE_INSTALL_PREFIX=../install
make -j$(nproc) && make install
```

### 1.2 编译 BluesFL

```bash
cd bluesfl
cargo build --bin sv_analysis --bin test_analysis
```

产出两个二进制：
- `target/debug/sv_analysis` — 定位器（Phase 1 BFS + Phase 2 LLM 导航）
- `target/debug/test_analysis` — 测试报告生成器（从 cosim 输出自动生成）

### 1.3 编译 Ibex Co-simulation

需要带 `--coverage` 标志编译 Verilator 仿真：

```bash
cd ~/ibex

# 确认 fusesoc 配置文件已启用 coverage：
# dv/verilator/simple_system_cosim/ibex_simple_system_cosim.core 中需要有：
#   - '--coverage'
#   - '-DVM_COVERAGE=1'

fusesoc --cores-root=. run --target=sim \
    --tool=verilator lowrisc:ibex:ibex_simple_system_cosim
```

仿真二进制路径：
```
~/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/Vibex_simple_system
```

### 1.4 CoreMark 测试程序

```bash
# CoreMark ELF 应位于：
# ~/ibex/examples/sw/benchmarks/coremark/coremark.elf
# 若不存在，按 Ibex 文档编译。
```

### 1.5 环境变量

```bash
# API 配置（bluesfl 根目录下）
cat > bluesfl/.env << 'EOF'
API_KEY=你的API密钥
API_BASE=https://api.deepseek.com
MODEL=deepseek-v4-pro
EOF

# Block manager 需要此变量
export SV_ANALYSIS_HOME=~/bluesfl
```

---

## 2. 测试流程总览

```
┌──────────┐    ┌───────────┐    ┌────────────┐
│ 注入 Bug  │───▶│ 编译 Cosim │───▶│ 运行 Cosim  │
│ (改 RTL)  │    │ (fusesoc)  │    │ (CoreMark)  │
└──────────┘    └───────────┘    └──────┬──────┘
                                       │
                   ┌───────────────────┘
                   │
         ┌─────────┼──────────┬──────────────┐
         ▼         ▼          ▼              ▼
     sim.fst   coverage   mismatch_log  trace_core.log
                 *.dat        │              │
                              ▼              │
                    ┌────────────────┐       │
                    │ test_analysis  │◀──────┘
                    └───────┬────────┘
                            │
                       test_info.json
                   (sig=rvfi_pc_wdata, t=19)
                            │
         ┌──────────────────┼──────────────────┐
         ▼                  ▼                  ▼
     sim.fst            coverage            RTL 源码
                            │
                   ┌────────▼────────┐
                   │   sv_analysis   │
                   │                 │
                   │ Phase 1: BFS    │  → 396 trace blocks
                   │ Phase 2: LLM    │  → ibex_alu
                   └────────┬────────┘
                            │
                   llm_loc_results.json
                            │
                   ┌────────▼────────┐
                   │     评测        │  Top-1 = ibex_alu ✅
                   └─────────────────┘
```

### 与论文的对齐关系

| 要素 | 论文 | 本框架 |
|------|------|--------|
| 起始信号 | `rvfi_pc_wdata`（cosim 比对信号） | `rvfi_pc_wdata` |
| 测试报告 | cosim 自动生成 | `test_analysis` 自动生成 |
| Coverage | Verilator per-cycle | `--cov-start 1 --cov-end 30` |
| Blues BFS | Algorithm 1，coverage 检查在 time t | 一致 |
| LLM 导航 | tool calls（Section 3.4） | `--agent-mode=tool-call` |
| 架构 | 两阶段（先构建路径 G，再导航 G） | Phase 1 + Phase 2 |

---

## 3. 逐步操作指南

以论文 Figure 6 的 ALU 加法器 bug 为例。

### Step 1：注入 Bug

修改 Ibex RTL，引入一个 bug：

```diff
--- a/rtl/ibex_alu.sv
+++ b/rtl/ibex_alu.sv
@@ -102,7 +102,7 @@

   // actual adder
-  assign adder_result_ext_o = $unsigned(adder_in_a) + $unsigned(adder_in_b);
+  assign adder_result_ext_o = $unsigned(adder_in_a) - $unsigned(adder_in_b);
```

保存 diff 作为评测用的 ground truth：

```bash
cd ~/ibex
diff -u rtl/ibex_alu.sv.golden rtl/ibex_alu.sv > ~/bluesfl/ibex_dataset/0/diff
```

> **论文做法**：按 mutation rules [20,27] 系统注入 119 个 bug。本框架支持手动注入；可通过 mutator 工具自动化。

### Step 2：编译 Co-simulation（仅 RTL 改动后需要）

```bash
cd ~/ibex
fusesoc --cores-root=. run --target=sim \
    --tool=verilator lowrisc:ibex:ibex_simple_system_cosim
```

### Step 3：运行 Co-simulation

运行联合仿真，生成波形、per-cycle coverage 和 mismatch 日志：

```bash
SIMDIR=~/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator
cd "$SIMDIR"

# 清除旧 coverage 文件
rm -f coverage*.dat

# 运行 cosim
# --cov-start 1：从第 1 个周期开始 dump coverage
# --cov-end 30：上界（仿真在 mismatch 时自动停止）
# --cov-dir .：coverage 文件输出到当前目录
# stdout 重定向捕获 mismatch 信息
./Vibex_simple_system \
    --meminit=ram,../../../examples/sw/benchmarks/coremark/coremark.elf \
    -t \
    --cov-start 1 --cov-end 30 \
    --cov-dir . > mismatch_log.txt 2>&1
```

**Cosim 做了什么：**

1. Ibex RTL（Verilator 编译）和 Spike ISS 同时执行 CoreMark
2. 每条指令 commit 时，checker 模块比对 PC 和寄存器值
3. 发现不一致时停止仿真，输出 mismatch 信息到 stdout
4. 每个 posedge 周期生成一个 coverage 文件

**输出文件：**

| 文件 | 用途 |
|------|------|
| `sim.fst` | 波形文件（LLM 通过 `read_values` 工具读取信号值） |
| `coverage_3,5,...,23_seq.dat` | per-cycle coverage（Blues BFS 的 coverage gate） |
| `rm_params.tree.json` | Verilator 符号表（scope/信号映射） |
| `mismatch_log.txt` | cosim stdout（含 mismatch 信息，test_analysis 的输入） |
| `trace_core_00000000.log` | 指令 trace（test_analysis 的输入） |

**预期 mismatch 输出：**
```
FAILURE: Co-simulation mismatch at time                   22
PC mismatch, DUT retired : f6148 , but the ISS retired: 109fb8
```

### Step 4：生成测试报告

从 cosim 输出自动生成测试报告 `(I, sig, t, E)`：

```bash
cd ~/bluesfl

SIMDIR=~/ibex/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator

target/debug/test_analysis \
    --info-file "$SIMDIR/mismatch_log.txt" \
    --inst-trace "$SIMDIR/trace_core_00000000.log" \
    --output-file ibex_dataset/0/test_info.json \
    --time-step 2
```

**`test_analysis` 的处理逻辑：**

1. 从 `mismatch_log.txt` 解析 mismatch 类型和失败时间
2. 从 `trace_core_00000000.log` 解析指令上下文
3. 计算测试参数：

```
failure_time = 22                          （从 "mismatch at time 22" 解析）
start_time   = failure_time - 1 - time_step = 22 - 1 - 2 = 19
ibex_cycle   = 2 × time_step = 4
time_bound   = start_time - 2 × ibex_cycle = 19 - 8 = 11
```

4. 根据 mismatch 类型选择起始信号：

| Mismatch 类型 | start_sig | 说明 |
|---------------|-----------|------|
| PC 不匹配 | `rvfi_pc_wdata` | 论文 Figure 6 使用的信号 |
| 写使能不匹配 | `rvfi_rd_addr_d` | — |
| 写数据不匹配 | `rvfi_rd_wdata_d` | — |

**输出（`test_info.json`）：**

```json
{
    "start_scope": "TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core",
    "start_sig": "rvfi_pc_wdata",
    "start_time": 19,
    "test_info": "Now this core design is buggy when executing the instruction:\n jal...",
    "time_bound": 11
}
```

### Step 5：运行 BluesFL 定位

执行两阶段 BluesFL 定位：

```bash
cd ~/ibex  # cwd 必须是 Ibex 工作目录
export SV_ANALYSIS_HOME=~/bluesfl

~/bluesfl/target/debug/sv_analysis \
    --bug-id=0 \
    --agent-type=open-ai \
    --agent-mode=tool-call \
    --model=deepseek-v4-pro \
    --project-path=~/ibex/rtl \
    --include-paths=~/ibex/vendor/lowrisc_ip/ip/prim/rtl/,~/ibex/vendor/lowrisc_ip/dv/sv/dv_utils \
    --rm-params-path=$SIMDIR/rm_params.tree.json \
    --coverage-path=$SIMDIR \
    --wave-path=$SIMDIR/sim.fst \
    --top-module=ibex_core \
    --top-scope=TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core \
    --start-scope=TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core \
    --start-sig=rvfi_pc_wdata \
    --start-time=19 \
    --time-bound=11 \
    --time-step=2 \
    --output-path=~/bluesfl/ibex_dataset/0/llm_rvfi \
    --vote-top-k=1 \
    --vote-total=1 \
    --dot-env=~/bluesfl/.env \
    --test-info="Co-simulation PC mismatch: DUT=0x000f6148, expected=0x00109fb8"
```

**关键参数说明：**

| 参数 | 值 | 说明 |
|------|-----|------|
| `--agent-mode=tool-call` | tool-call 模式 | LLM 通过 tool calls 自主导航（论文 Section 3.4） |
| `--start-sig=rvfi_pc_wdata` | cosim 比对信号 | 发生分歧的信号（论文 Figure 6） |
| `--start-time=19` | posedge 周期 | 分歧首次可观测的时间 |
| `--time-bound=11` | BFS 下界 | 反向追踪的时间下限 |
| `--time-step=2` | 时钟周期 | 每个 posedge 对应 2 个仿真时间单位 |
| `--project-path` | RTL 目录 | SystemVerilog 源码 |
| `--coverage-path` | coverage 目录 | 含 `coverage_*.dat` 文件的目录 |
| `--wave-path` | FST 波形 | LLM 读取信号值用 |
| `--rm-params-path` | 符号表 | Verilator 的 scope/信号映射 |

**Phase 1：Blues BFS（Algorithm 1，无 LLM）**

纯数据流反向追踪：

```
输入:  (rvfi_pc_wdata, 19)，在 ibex_core scope
过程:  BFS 反向追踪数据流图
       - FindDrivenBlock(sig): 找到 V_o 包含 sig 的 block
       - IntraBlockAnalysis(sig, block, t):
         · COMB: coverage 检查在 t，传播驱动信号在 t
         · SEQ:  coverage 检查在 t，传播驱动信号在 t-1
                 若未覆盖: 自环（寄存器保持原值）
输出:  396 个 trace block（指令执行路径 G）
```

**Phase 2：LLM 导航（Section 3.4，用 LLM）**

LLM 在单次连续会话中通过 tool calls 导航路径 G：

```
LLM 可用工具:
  read_values(signal, time)    → 读取波形中指定时间的信号值
  check_signals(signals)       → 跳转到驱动所选信号的 block
  append_block()               → 标记当前 block 为可疑
  exit()                       → 结束分析

导航示例:
  起点: ibex_core (rvfi_pc_wdata @19)
  → check_signals(pc_if)      → 跳转到 if_stage 模块
  → read_values(pc_if, 17)    → 读到 0x000F5FC0（错误地址）
  → （继续追踪流水线）
  → check_signals(adder_result_o) → 跳转到 alu 模块
  → append_block()             → 标记 ALU 为可疑
  → exit()
```

**结果：**

```
trace_blocks: 396    （论文: 357）
Top-1:        ibex_alu (bid=1357)
```

### Step 6：评测

将定位结果与注入的 bug 对比：

```bash
# 定位结果
cat ~/bluesfl/ibex_dataset/0/llm_rvfi/llm_loc_results_0.json | python3 -m json.tool

# Ground truth
cat ~/bluesfl/ibex_dataset/0/diff
```

**预期结果：**

```json
{
    "bug_id": "0",
    "choices": [
        {
            "module_name": "ibex_alu",
            "block_id": 1357,
            "score": 0.8
        }
    ]
}
```

Ground truth：`rtl/ibex_alu.sv`（ALU 加法器运算符被修改）。

**Top-1 = ibex_alu ✅** — 正确定位到出错模块。

---

## 4. 批量测试脚本

对数据集中所有 bug 自动执行 Step 3-5：

```bash
cd ~/bluesfl

python3 scripts/ibex_fl_run_all.py \
    --path ibex_dataset \
    --localizer target/debug/sv_analysis \
    --test-analysis target/debug/test_analysis \
    --env .env \
    --model deepseek-v4-pro \
    --vote-total 1 \
    --prefix llm
```

**每个 bug 的自动流程：**

```
ibex_dataset/
├── 0/              ← bug 0
│   ├── 0/          ← Ibex 工作目录（RTL + build）
│   ├── diff        ← ground truth
│   └── llm_1/      ← 定位结果（自动递增编号）
├── 1/              ← bug 1
└── ...

对每个 bug 自动执行:
  1. 重跑 cosim (--cov-start 1) → mismatch_log.txt + coverage 文件
  2. 生成 test_info.json（若不存在）via test_analysis
  3. 运行 sv_analysis → 保存结果
```

**批量脚本选项：**

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `--no-sim` | 关 | 跳过 cosim 重跑（使用已有 coverage） |
| `--start` / `--end` | 全部 | 处理 bug 范围（如 `--start 0 --end 10`） |
| `--prefix` | `llm` | 结果目录前缀 |
| `--vote-total` | 1 | 投票轮数 |
| `--vote-top-k` | 1 | 每轮选取 Top-K |

---

## 5. 架构说明

### 两阶段设计（对齐论文 Section 3.1）

```
┌───────────────────────────────────────────────────────────┐
│                       sv_analysis                          │
│                                                            │
│  ┌──────────────────────────────────────────────────┐     │
│  │  Phase 1: Blues BFS (Algorithm 1)                 │     │
│  │                                                   │     │
│  │  输入:  测试报告 (sig, t)                         │     │
│  │  过程:  纯数据流反向追踪                           │     │
│  │         - sv-parser → 1,329 个代码 block           │     │
│  │         - BFS + IntraBlockAnalysis                 │     │
│  │         - per-cycle coverage gate                  │     │
│  │  输出:  396 个 trace block（指令执行路径 G）       │     │
│  └──────────────────────┬───────────────────────────┘     │
│                         │                                  │
│  ┌──────────────────────▼───────────────────────────┐     │
│  │  Phase 2: LLM 导航 (Section 3.4)                  │     │
│  │                                                   │     │
│  │  输入:  trace blocks (G)                          │     │
│  │  过程:  LLM 通过 tool calls 导航 G                 │     │
│  │         - read_values: 读波形                     │     │
│  │         - check_signals: 跳转到驱动 block          │     │
│  │         - append_block: 标记可疑                   │     │
│  │         - exit: 结束分析                           │     │
│  │  输出:  排序后的可疑 block 列表                    │     │
│  └───────────────────────────────────────────────────┘     │
│                                                            │
└───────────────────────────────────────────────────────────┘
```

### 关键源码文件

| 文件 | 作用 |
|------|------|
| `src/bootstrap.rs` | 入口；编排 Phase 1 + Phase 2 |
| `src/tracer/mod.rs` | Blues BFS 主循环（Algorithm 1）；信号查找、scope 转换 |
| `src/tracer/slice/dynamic_slice.rs` | IntraBlockAnalysis；coverage gate |
| `src/tracer/navigate.rs` | Phase 2 导航工具（NavReadValues 等） |
| `src/tracer/llm.rs` | `run_phase1_bfs()`、`run_phase2_navigate()` |
| `src/block/dfb.rs` | 代码分块（sv-parser → DataFlowBlock） |
| `src/block/utils.rs` | AST 信号提取（`get_right_values_in_expression`） |
| `src/coverage/vlc.rs` | Verilator coverage 解析；`check_line_covered` |
| `src/wave/mgr.rs` | 波形管理器（通过 wellen 读取 FST） |
| `src/bin/test_analysis.rs` | 从 cosim 输出生成测试报告 |
| `src/bin/sv_analysis.rs` | sv_analysis CLI 入口 |

### Coverage 模型

Verilator per-cycle coverage 作为 Algorithm 1 的 coverage gate：

```
Algorithm 1, IntraBlockAnalysis(s, b, t):
  I_s = DataFlowAnalysis(s, b, t)      // 获取所有驱动信号
  if b 是 COMB:
    return (I_s, t)                    // 总是传播
  else:  // SEQ
    if coverage 在 t 显示赋值已执行:
      return (I_s, t - time_step)      // 传播驱动信号
    else:
      return ({s}, t - time_step)      // 自环（寄存器保持原值）
```

Coverage 在当前时间 `t` 检查。每个 `coverage_<time>_seq.dat` 文件记录单个 posedge 周期的行/toggle 覆盖。

### 起始信号：rvfi_pc_wdata

起始信号是 `rvfi_pc_wdata` — RISC-V Formal Interface 的 PC 写数据。这是 co-simulation checker 在 RTL 和 Spike ISS 之间比对的信号。当 PC 不匹配发生时，`rvfi_pc_wdata` 是第一个暴露分歧的信号。

对齐论文 Figure 6：
```
Suspicious Signal: rvfi_pc_wdata
Failed Time: 19
```

---

## 6. 常见问题

### Coverage 文件未生成

**现象：** cosim 运行后没有 `coverage*.dat` 文件。

**原因：** cosim 二进制未带 coverage 支持。

**解决：** 确认 `ibex_simple_system_cosim.core` 含 `--coverage` 和 `-DVM_COVERAGE=1`，且 CLI 用 `--cov-start`（不是 `--cover-start`）。

### BFS 只找到 2 个 trace block

**现象：** `trace_blocks len: 2`，远少于 396。

**原因：** Coverage 检查时间错误（用了 t-1 而非 t），或 RHS 提取遗漏驱动信号。

**解决：**
1. Coverage 检查必须在 `time`（当前 t）：`src/tracer/slice/dynamic_slice.rs` 中 `check_line_covered` 的 time 参数用 `time`
2. RHS 提取直接访问赋值节点：`src/block/utils.rs` 中 `get_right_values_in_expression` 用 `stmt.nodes.3`

### wellen FST 解析 panic

**现象：** `thread 'main' panicked` in `wellen::hierarchy`。

**原因：** Ibex FST 的 scope 结构触发了上游 wellen 的 panic。

**解决：** 使用 `patches/wellen-0.14.6/` 中的补丁版本（已在 `Cargo.toml` 中配置为 path 依赖）。

### LLM 未找到 bug

**现象：** LLM 标记了错误的模块。

**原因：** LLM 行为非确定性，不同运行可能导航路径不同。

**解决：** 多次运行（`--vote-total 2`）或换模型（`--model gpt-4o`）。

### SV_ANALYSIS_HOME 未设置

**现象：** `bootstrap.rs` panic: "SV_ANALYSIS_HOME not set"。

**解决：**
```bash
export SV_ANALYSIS_HOME=~/bluesfl
```
