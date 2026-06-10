# BluesFL on NutShell — 完整测试框架使用指南

> 本文档介绍如何使用 BluesFL 对 NutShell 处理器进行 bug 定位测试。
> 包含环境搭建、bug 注入、仿真、BluesFL 分析、结果查看的完整流程。
> 基于实际测试经验编写，覆盖 7 个 bug 的测试案例。

---

## 目录

1. [架构概览](#1-架构概览)
2. [环境搭建](#2-环境搭建)
3. [测试流程总览](#3-测试流程总览)
4. [Step 0: 编译 BluesFL](#step-0-编译-bluesfl)
5. [Step 1: 注入 Bug](#step-1-注入-bug)
6. [Step 2: 生成分文件 RTL](#step-2-生成分文件-rtl)
7. [Step 3: 编译带 Coverage 的 Emulator](#step-3-编译带-coverage-的-emulator)
8. [Step 4: 编译测试用例](#step-4-编译测试用例)
9. [Step 5: 运行仿真](#step-5-运行仿真)
10. [Step 6: 准备 BluesFL 输入](#step-6-准备-bluesfl-输入)
11. [Step 7: 运行 BluesFL](#step-7-运行-bluesfl)
12. [Step 8: 查看结果](#step-8-查看结果)
13. [Step 9: 清理](#step-9-清理)
14. [完整测试案例](#完整测试案例)
15. [测试结果汇总](#测试结果汇总)
16. [原理分析](#原理分析)
17. [常见问题](#常见问题)

---

## 1. 架构概览

```
                         ┌──────────────┐
                         │  Bug Patch   │  手动修改或 git apply 注入 bug
                         └──────┬───────┘
                                │
                                ▼
┌───────────────────────────────────────────────────────────┐
│                     NutShell Processor                     │
│                                                           │
│  make verilog → build/rtl/*.sv (分文件 RTL, ~119 files)   │
│  make emu      → build/emu (Verilator + coverage + FST)   │
│                                                           │
│  ./build/emu -i test.elf \                                │
│      --diff ref.so \                                      │
│      --dump-coverage --dump-wave \                        │
│      --cov-start 530 --cov-end 535 --cov-dir ./cov        │
│                                                           │
│  输出:                                                    │
│    cov/coverage_1060_seq.dat  (per-cycle coverage)        │
│    build/*.fst                (FST 波形)                  │
│    emu_output.log            (DiffTest 日志)              │
└──────────────────────────┬────────────────────────────────┘
                           │
                           ▼
┌───────────────────────────────────────────────────────────┐
│                   BluesFL sv_analysis                      │
│                                                           │
│  1. Code Blockization (sv-parser → DataFlowBlock)         │
│  2. Blues BFS (Algorithm 1, per-cycle coverage)           │
│  3. LLM Reasoning (tool-call mode, deepseek-v4-pro)       │
│  4. Ranking → LocalizationResult                          │
└───────────────────────────────────────────────────────────┘
```

### 三阶段工作原理

| 阶段 | 输入 | 方法 | 输出 |
|------|------|------|------|
| 1. 代码切片 | RTL 源码 | sv-parser 静态分析 | 5328 个 block |
| 2. Blues BFS | Coverage 数据 | 数据流反向追踪 | 10-15 个相关 block |
| 3. LLM 推理 | FST 波形 + block 代码 | tool-call 多轮交互 | Top-1 定位结果 |

---

## 2. 环境搭建

### 2.1 仓库结构

```bash
/home/yuan/
├── bluesfl/          # BluesFL 工具 (分支: feat/per-cycle-coverage)
├── NutShell/         # NutShell 处理器 (含 DiffTest 子模块)
└── nutshell-sbfl/    # Bug 数据集 (21 个 patch + 测试用例)
```

### 2.2 依赖工具

| 依赖 | 版本要求 | 说明 |
|------|---------|------|
| Verilator | >= 5.028 | 需 coverage 支持 (`--coverage-line --coverage-toggle`) |
| RISC-V GCC | `riscv64-unknown-elf-gcc` | 编译汇编测试用例 |
| Rust/Cargo | stable | 编译 sv_analysis |
| DiffTest | NutShell 子模块 | 需含 per-cycle coverage 改动 |

### 2.3 LLM 配置

文件: `/home/yuan/bluesfl/.env`

```bash
AGENT_TYPE=open-ai
MODEL=deepseek-v4-pro
API_KEY=<your-api-key>
API_BASE=https://api.deepseek.com
```

### 2.4 NutShell Scope 命名

NutShell 分文件 RTL 的 Verilator scope 格式:
```
TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.<module>
```
注意 `cpu.soc.nutcore` 会重复一次。

### 2.5 验证 DiffTest 子模块含 coverage 支持

```bash
cd /home/yuan/NutShell/difftest
git log --oneline -1
# 应该看到: feat: per-cycle coverage dump for BluesFL IntraBlockAnalysis

grep "cov_start" src/test/csrc/common/args.cpp
# 应该有输出，说明 --cov-start/--cov-end/--cov-dir 参数可用
```

---

## 3. 测试流程总览

```
Step 0: cargo build              编译 BluesFL
Step 1: 修改 Scala 源码          注入 bug
Step 2: make verilog             生成 RTL
Step 3: make emu EMU_COVERAGE=1  编译带 coverage 的仿真器
Step 4: gcc                      编译测试用例
Step 5: ./build/emu              运行仿真 (coverage + FST + diff)
Step 6: cp fst, echo rm_params   准备 BluesFL 输入
Step 7: ./sv_analysis            运行 BluesFL 分析
Step 8: cat results              查看定位结果
Step 9: git checkout             清理恢复
```

---

## Step 0: 编译 BluesFL

```bash
cd /home/yuan/bluesfl
cargo build --bin sv_analysis
```

输出: `target/debug/sv_analysis`

---

## Step 1: 注入 Bug

### 方式 A: 使用 nutshell-sbfl 的 patch

```bash
cd /home/yuan/NutShell
git checkout -- .
rm -f src/test/scala/FirrtlExportMain.scala

# 应用 patch (以 U1 为例)
git apply /home/yuan/nutshell-sbfl/patch/U1_addiw_signext.patch
```

### 方式 B: 手动修改源码

直接编辑 Scala 文件。以下是已测试过的 bug 示例:

**Bug A — ALU taken 取反 (控制通路, ALU 内部)**
```scala
// 文件: src/main/scala/nutcore/backend/fu/ALU.scala
// 原始:
val taken = LookupTree(ALUOpType.getBranchType(func), branchOpTable) ^ ALUOpType.isBranchInvert(func)
// 改为:
val taken = !(LookupTree(ALUOpType.getBranchType(func), branchOpTable) ^ ALUOpType.isBranchInvert(func))
```

**Bug B — EXU LSU valid bypass (控制通路, 跨模块)**
```scala
// 文件: src/main/scala/nutcore/backend/seq/EXU.scala
// 原始:
FuType.lsu -> lsu.io.out.valid,
// 改为:
FuType.lsu -> true.B,
```

**D1-like — Decoder XOR→AND 映射 (控制通路, 跨模块)**
```scala
// 文件: src/main/scala/nutcore/isa/RVI.scala
// 原始:
XOR            -> List(InstrR, FuType.alu, ALUOpType.xor),
// 改为:
XOR            -> List(InstrR, FuType.alu, ALUOpType.and),
```

---

## Step 2: 生成分文件 RTL

```bash
cd /home/yuan/NutShell
NOOP_HOME=/home/yuan/NutShell make verilog

# 验证
wc -l build/rtl/NutCore.sv
# 预期: ~1665 行（不是几万行的扁平化版本）
```

**注意**: 如果看到 NutCore.sv 有几万行，说明用了预构建的扁平化 RTL，必须重新 `make verilog`。

---

## Step 3: 编译带 Coverage 的 Emulator

```bash
cd /home/yuan/NutShell
NOOP_HOME=/home/yuan/NutShell \
make emu EMU_COVERAGE=1 EMU_TRACE=fst RTL_SUFFIX=sv \
     WITH_CHISELDB=0 WITH_CONSTANTIN=0 -j$(nproc)
```

**参数说明**:
- `EMU_COVERAGE=1`: Verilator 加 `--coverage-line --coverage-toggle`
- `EMU_TRACE=fst`: 启用 FST 波形输出
- `WITH_CHISELDB=0 WITH_CONSTANTIN=0`: 避免不必要的依赖

**验证**:
```bash
grep "VM_COVERAGE" build/verilator-compile/VSimTop_classes.mk
# 必须显示: VM_COVERAGE = 1
# 如果显示 VM_COVERAGE = 0，说明 coverage 没编译进去，需要:
#   rm -rf build/verilator-compile
#   重新 make emu
```

---

## Step 4: 编译测试用例

### 创建链接脚本

```bash
cat > build/u1_link.ld << 'EOF'
OUTPUT_ARCH(riscv)
ENTRY(_start)
SECTIONS {
    . = 0x80000000;
    .text : { *(.text) }
    .data : { *(.data) }
    .rodata : { *(.rodata) }
}
EOF
```

**关键**: `. = 0x80000000` 必须跟 NutShell 的 ResetVector 一致。

### 使用 nutshell-sbfl 的测试用例

```bash
riscv64-unknown-elf-gcc -nostdlib -nostartfiles \
    -T build/u1_link.ld \
    -o build/U1_test.elf \
    /home/yuan/nutshell-sbfl/case/U1_addiw_signext.S
```

### 手写测试用例示例

```asm
# Bug A 测试用例 (ALU taken 取反)
    .globl _start
    .text
_start:
    li t0, 0x80000000       # 目标地址
    li t1, 1
    beq t1, t1, target      # 应该跳但不跳 (taken 被取反)
    li t2, 0                 # 如果没跳，t2=0 (错)
    j .end
target:
    li t2, 1                 # 如果跳了，t2=1 (对)
.end:
    nop
1:  j 1b
```

编译:
```bash
riscv64-unknown-elf-gcc -nostdlib -nostartfiles \
    -T build/u1_link.ld -o build/test.elf test.S
```

---

## Step 5: 运行仿真

```bash
cd /home/yuan/NutShell

BUG_ID=<你的 bug 标识>
WORK_DIR=/home/yuan/bluesfl/e2e_results/${BUG_ID}_percycle
mkdir -p ${WORK_DIR}/coverage

./build/emu \
    -i ./build/<test>.elf \
    --diff ./ready-to-run/riscv64-nemu-interpreter-so \
    --dump-wave \
    --dump-commit-trace \
    --dump-coverage \
    --cov-start <起始周期> --cov-end <结束周期> \
    --cov-dir ${WORK_DIR}/coverage \
    2>&1 | tee ${WORK_DIR}/emu_output.log
```

### 参数详解

| 参数 | 说明 |
|------|------|
| `-i test.elf` | 测试程序 |
| `--diff ref.so` | DiffTest 参考模型 (NEMU) |
| `--dump-wave` | 生成 FST 波形 |
| `--dump-coverage` | 启用覆盖率收集 |
| `--cov-start N` | 从第 N 个周期开始 per-cycle dump |
| `--cov-end M` | 到第 M 个周期结束 |
| `--cov-dir DIR` | coverage 文件输出目录 |

### 确定覆盖窗口

仿真正常会在某条指令处 ABORT，日志里会显示:
```
t2 different at pc = 0x0080000006, right = ..., wrong = ...
DiffTest mismatch: posedge_time=1070 (host_cycle=535)
Core 0: ABORT at pc = 0x8000000a
Core-0 instrCnt = 3, cycleCnt = 535
```

从中提取:
- `cycleCnt = 535` → 失败周期
- `instrCnt = 3` → 指令数
- `posedge_time = 1070` → 失败时刻 (= cycleCnt × 2)
- `cov-start = cycleCnt - instrCnt - 2` (多留 2 个周期余量)
- `cov-end = cycleCnt + 2` (多留 2 个周期)

### Per-cycle Coverage 工作原理

1. 每个 posedge **前**: `VerilatedCov::zero()` 清零计数器
2. posedge step 执行: Verilator 累积本周期的覆盖率
3. posedge step **后**: `coverage->write()` 写入文件
4. 文件命名: `coverage_<cycles*2>_seq.dat`
5. 每个文件只包含**这一个周期**的覆盖率数据

### 预期输出

```
${WORK_DIR}/coverage/
├── coverage_1060_seq.dat  (cycle 530, ~36MB)
├── coverage_1062_seq.dat  (cycle 531)
├── coverage_1064_seq.dat  (cycle 532)
├── coverage_1066_seq.dat  (cycle 533)
├── coverage_1068_seq.dat  (cycle 534)
└── coverage_1070_seq.dat  (cycle 535, 失败周期)
```

---

## Step 6: 准备 BluesFL 输入

```bash
BUG_ID=<你的 bug 标识>
WORK_DIR=/home/yuan/bluesfl/e2e_results/${BUG_ID}_percycle

# FST 波形
FST_FILE=$(ls -t /home/yuan/NutShell/build/*.fst | head -1)
cp ${FST_FILE} ${WORK_DIR}/sim.fst

# rm_params (NutShell 无参数化模块，空 JSON)
echo '{}' > ${WORK_DIR}/rm_params.tree.json
```

---

## Step 7: 运行 BluesFL

```bash
cd /home/yuan/bluesfl

BUG_ID=<你的 bug 标识>
WORK_DIR=/home/yuan/bluesfl/e2e_results/${BUG_ID}_percycle

SV_ANALYSIS_HOME=/home/yuan/bluesfl \
./target/debug/sv_analysis \
    --bug-id=${BUG_ID} \
    --agent-type=open-ai \
    --agent-mode tool-call \
    --model=deepseek-v4-pro \
    --project-path=/home/yuan/NutShell/build/rtl \
    --include-paths=/home/yuan/NutShell/build/rtl \
    --include-paths=/home/yuan/NutShell/build/generated-src \
    --rm-params-path=${WORK_DIR}/rm_params.tree.json \
    --coverage-path=${WORK_DIR}/coverage \
    --wave-path=${WORK_DIR}/sim.fst \
    --top-module=SimTop \
    --top-scope=TOP.SimTop.cpu.soc.nutcore \
    --start-scope=TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend \
    --start-sig=io_out_bits \
    --start-time=<posedge_time> \
    --time-bound=<posedge_time - instrCnt*2> \
    --time-step=2 \
    --output-path=${WORK_DIR}/llm_1 \
    --vote-top-k=1 \
    --vote-total=1 \
    --test-info="<描述 bug 现象>" \
    --dot-env=./.env
```

### 关键参数详解

| 参数 | 说明 | 如何确定 |
|------|------|---------|
| `--agent-mode tool-call` | 使用 tool-call 模式（多轮交互） | 固定 |
| `--start-scope ...backend` | BFS 起始 scope | 固定，从 backend 开始让算法自然追踪 |
| `--start-sig io_out_bits` | BFS 起始信号 | 数据类 bug 用 `io_out_bits`，分支类 bug 用 `io_redirect_target` |
| `--start-time` | BFS 起始时刻 | = 仿真日志的 `posedge_time` |
| `--time-bound` | BFS 下界 | = `start-time - instrCnt × 2` |
| `--time-step` | 时间步长 | 固定为 2（Verilator: 1 cycle = 2 time units） |
| `--vote-total 1` | 单次运行 | 固定为 1（多次投票可能导致空结果） |
| `--test-info` | bug 现象描述 | 从仿真日志提取 |

### 时间系统

```
Verilator: 1 clock cycle = 2 time units (posedge + negedge)
time_step = 2
start_time = cycleCnt × 2
time_bound = start_time - instrCnt × 2 (回溯到第一条指令执行时)
coverage 文件名: coverage_<cycle × 2>_seq.dat
```

### 起始信号选择指南

| bug 类型 | --start-sig | --start-scope |
|---------|-------------|---------------|
| 寄存器值错误 | `io_out_bits` | `...backend` |
| PC/分支错误 | `io_redirect_target` | `...backend` |
| Load 数据错误 | `io_out_bits` | `...backend` |

### 预期运行时间

| 阶段 | 耗时 |
|------|------|
| parse_sv (116 files) | ~6 min |
| BlockManager (dataflow) | ~1 min |
| Blues BFS + LLM | ~15-20 min |
| **总计** | **~25 min** |

---

## Step 8: 查看结果

```bash
BUG_ID=<你的 bug 标识>
WORK_DIR=/home/yuan/bluesfl/e2e_results/${BUG_ID}_percycle

# 定位结果
cat ${WORK_DIR}/llm_1/llm_loc_results_${BUG_ID}.json

# 追踪路径
python3 -c "
import json
with open('${WORK_DIR}/llm_1/trace.json') as f:
    data = json.load(f)
for i, item in enumerate(data):
    scope = item['scope'].split('.')[-1]
    btype = item['type']
    t = item['time']
    print(f'  [{i:2d}] {btype:12s} scope=...{scope:20s} t={t}')
"
```

### 结果解读

成功案例:
```json
{
  "choices": [
    {"module_name": "ALU", "block_id": 5806, "score": 1.0}
  ]
}
```

失败案例:
```json
{
  "choices": []
}
```

---

## Step 9: 清理

```bash
cd /home/yuan/NutShell
git checkout -- .
```

---

## 完整测试案例

以下是我们实际测试过的 7 个 bug，每个都按上述流程执行。

### 案例 1: U1 — ALU SignExt→ZeroExt (数据路径)

```bash
# Step 1
git apply /home/yuan/nutshell-sbfl/patch/U1_addiw_signext.patch

# Step 5
--cov-start 530 --cov-end 535

# Step 7
--start-time 1070 --time-bound 1064
--test-info="Instruction addiw x2, x1, 0 at pc=0x80000006 incorrectly produces x2=0x0000000080000000 (zero-extended), should be 0xffffffff80000000 (sign-extended)"
```

**结果: Top-1 = ALU, score=1.0** ✅ 定位准确

---

### 案例 2: U6 — JALR bit[0] 未清零 (控制通路, ALU 内部)

```bash
# Step 1
git apply /home/yuan/nutshell-sbfl/patch/U6_jalr_bit0_not_cleared.patch

# Step 7
--start-sig io_redirect_target
--test-info="JALR instruction target has bit[0]=1, should be cleared to 0"
```

**结果: Top-1 = ALU, score=1.0** ✅ 定位准确

---

### 案例 3: Bug A — ALU taken 取反 (控制通路, ALU 内部)

```bash
# Step 1: 手动修改 ALU.scala line 118
# val taken = !(LookupTree(...))

# Step 5
--cov-start 530 --cov-end 535

# Step 7
--start-sig io_redirect_target --start-time 1066 --time-bound 1064
--test-info="BEQ at pc=0x80000004 should be taken but is not taken"
```

**结果: Top-1 = ALU, score=1.0** ✅ 定位准确

---

### 案例 4: Bug B — EXU LSU valid bypass (控制通路, 跨模块)

```bash
# Step 1: 手动修改 EXU.scala line 107
# FuType.lsu -> true.B

# Step 5
--cov-start 530 --cov-end 600

# Step 7
--start-time 1080 --time-bound 1064
--test-info="Load word at pc=0x80000018 produces t2=0x0f000f00 (stale data due to LSU reporting valid before data ready), should be 0xdeadbeef"
```

**结果: Top-1 = LSExecUnit, score=1.0** ⚠️ 定位到症状模块，非根因 (bug 在 EXU)

---

### 案例 5: D1 — Decoder SUB→ADD (控制通路, 跨模块)

```bash
# Step 1
git apply /home/yuan/nutshell-sbfl/patch/D1_sub_sra_decode.patch

# Step 7
--test-info="SUB instruction produces ADD result"
```

**结果: Top-1 = ALU** ⚠️ 定位到症状模块 (bug 在 Decoder)

---

### 案例 6: D1-like — Decoder XOR→AND (控制通路, 跨模块)

```bash
# Step 1: 手动修改 RVI.scala line 65
# XOR -> ALUOpType.and

# Step 5
--cov-start 530 --cov-end 600

# Step 7
--start-time 1074 --time-bound 1060
--test-info="XOR instruction at pc=0x80000014 produces t2=0x0f000f00 (AND result), should be 0xf00ff00f (XOR result)"
```

**结果: choices=[]** ❌ 完全失败

---

### 案例 7: E2 — rfWen 门控 (控制通路, 跨模块)

**结果: choices=[]** ❌ 完全失败

---

## 测试结果汇总

| Bug | 类型 | 注入位置 | Top-1 | 准确？ |
|-----|------|---------|-------|--------|
| U1 | 数据路径 | ALU | ALU | ✅ 准确 |
| U6 | 控制(同模块) | ALU | ALU | ✅ 准确 |
| Bug A | 控制(同模块) | ALU | ALU | ✅ 准确 |
| Bug B | 控制(跨模块) | EXU | LSExecUnit | ⚠️ 症状模块 |
| D1 | 控制(跨模块) | Decoder | ALU | ⚠️ 症状模块 |
| D1-like | 控制(跨模块) | Decoder | 空 | ❌ 失败 |
| E2 | 控制(跨模块) | EXU | 空 | ❌ 失败 |

### 定位规律

```
成功 ← bug 在被追踪信号的 assign 表达式链路上
  ↓
  U1: SignExt→ZeroExt          (ALU assign 表达式里的函数)
  Bug A: taken 取反             (ALU assign 表达式里的运算)
  U6: JALR 目标没清零 bit[0]   (ALU assign 表达式里的位操作)

症状模块 ← bug 改了控制信号，但控制信号不在数据流链路上
  ↓
  Bug B: EXU valid 门控        (valid 不在 io_out_bits 的 assign 里)
  D1: Decoder 操作码映射        (fuOpType 不在 ALU 的 assign 里)

失败 ← 控制信号跨模块传递，LLM 无法判断控制值的正确性
  ↓
  D1-like: LLM 直接 terminate   (无法判断 fuOpType 应该是 xor 还是 and)
  E2: 信号展开过大，追踪能力不足
```

---

## 原理分析

### BluesFL 能检查什么

| 类型 | 示例 | 能否定位 |
|------|------|---------|
| assign 表达式数据计算错误 | SignExt→ZeroExt, 常量错误 | ✅ |
| 信号取反/恒常 | taken 取反, 信号接地 | ✅ |
| 位宽/位选错误 | JALR 目标没清零 bit[0] | ✅ |
| MuxLookup/Case 的选择器错误 | Decoder 映射错, if 条件错 | ❌ |
| 跨模块 valid/enable 门控 | EXU valid bypass, rfWen 短路 | ❌ |

### 为什么跨模块控制通路定位不到

BluesFL 的 Blues BFS 是纯数据流追踪，回答的问题是：
> "这个输出值是由哪些 assign 语句计算出来的？"

它不回答：
> "为什么选择了这条计算路径而不是那条？"

后者是控制流问题。当 bug 改变的是 MuxLookup/Case 的选择器（如 Decoder 的操作码映射）或 if 的条件（如 EXU 的 valid 门控），这些控制信号不在输出值的 assign 表达式链路上，BFS 追踪不到。

### 解决方向

在 IntraBlockAnalysis 中加入控制依赖追踪：
- 分析 MuxLookup/Mux/Case 表达式时，把 selector 也作为依赖信号
- 分析 always block 的 if 条件时，把 condition 也加入追踪队列

---

## 常见问题

### Q: Binary 加载地址不对 (0x10000 而非 0x80000000)
A: 必须用 `-T link.ld` 链接脚本，设置 `. = 0x80000000`。

### Q: VM_COVERAGE = 0，coverage 不工作
A: 需要删除 `build/verilator-compile` 目录后重新编译:
```bash
rm -rf build/verilator-compile
NOOP_HOME=/home/yuan/NutShell make emu EMU_COVERAGE=1 EMU_TRACE=fst RTL_SUFFIX=sv WITH_CHISELDB=0 WITH_CONSTANTIN=0 -j$(nproc)
```

### Q: NutCore.sv 有几万行
A: 用了预构建的扁平化 RTL。必须 `make verilog` 重新生成分文件 RTL。

### Q: vote-total=2 结果为空
A: 两次独立 LLM 运行结果不一致时投票会清空。用 `--vote-total=1`。

### Q: `--diff` 找不到参考模型
A: 检查 `./ready-to-run/riscv64-nemu-interpreter-so` 是否存在。

### Q: `FirrtlExportMain.scala` 编译错误
A: 删除该文件: `rm -f src/test/scala/FirrtlExportMain.scala`

### Q: parse_sv 时间太长
A: Divider.sv 单文件解析需要 ~350s，这是正常的。总 parse 时间约 6 min。

### Q: Token 消耗
A: 一次 tool-call 运行约 200K-650K token，取决于 bug 复杂度。跨模块 bug 消耗更多 token。
