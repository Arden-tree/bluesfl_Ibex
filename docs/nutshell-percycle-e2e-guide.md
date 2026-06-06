# BluesFL on NutShell — Per-Cycle Coverage 端到端技术报告

> 日期: 2026-06-07
> 分支: `feat/per-cycle-coverage`（BluesFL）, NutShell difftest 子模块
> 论文对齐: **Algorithm 1 完全对齐**
> Bug 测试: U1_addiw_signext (ALU sign-extension bug)
> 结果: **Top-1 = ALU, score=1.0**

---

## 0. 架构概览

```
                         ┌──────────────┐
                         │  Bug Patch   │  nutshell-sbfl/patch/U1_*.patch
                         │  (SignExt→   │  注入 bug 到 NutShell Chisel 源码
                         │   ZeroExt)   │
                         └──────┬───────┘
                                │ git apply
                                ▼
┌───────────────────────────────────────────────────────────┐
│                     NutShell Processor                     │
│                                                           │
│  make verilog → build/rtl/*.sv (分文件 RTL, 119 files)    │
│  make emu      → build/emu (Verilator emulator + coverage)│
│                                                           │
│  ./build/emu -i test.elf --diff ref.so \                  │
│      --dump-coverage --dump-wave \                        │
│      --cov-start 530 --cov-end 535 --cov-dir ./cov        │
│                                                           │
│  输出:                                                    │
│    cov/coverage_1060_seq.dat  ← per-cycle coverage dump  │
│    cov/coverage_1062_seq.dat     (每个 posedge 一个文件)  │
│    ...                                                    │
│    build/*.fst             ← FST 波形 (信号值)           │
│    emu_output.log          ← DiffTest 日志               │
└──────────────────────────┬────────────────────────────────┘
                           │
                           ▼
┌───────────────────────────────────────────────────────────┐
│                   BluesFL sv_analysis                      │
│                                                           │
│  1. Code Blockization (sv-parser → DataFlowBlock)         │
│     4 types: ModInput / ModOutput / Assign / Always       │
│                                                           │
│  2. Blues BFS (Algorithm 1)                               │
│     起点: (sig=io_out_bits, t=1070) from test report      │
│     BFS 反向追踪, IntraBlockAnalysis:                     │
│       COMB → coverage at t, propagate at t                │
│       SEQ  → coverage at t-1, propagate at t-1            │
│              if not covered → register holds ({sig}, t-1) │
│     → Instruction Execution Path G (12-13 blocks)         │
│                                                           │
│  3. LLM Reasoning (tool-call mode, deepseek-v4-pro)       │
│     Tools: read_values / check_signals / append_block /   │
│            exit                                            │
│     LLM 在 G 上逐块分析, 读波形信号值, 推理 bug 根因      │
│                                                           │
│  4. Ranking → LocalizationResult                          │
│     Top-1: ALU, score=1.0                                 │
└───────────────────────────────────────────────────────────┘
```

---

## 1. 前置条件

### 1.1 仓库和工具

| 依赖 | 路径 | 版本/说明 |
|------|------|----------|
| BluesFL | `/home/yuan/bluesfl` | 分支 `feat/per-cycle-coverage` |
| NutShell | `/home/yuan/NutShell` | 含 DiffTest 子模块（含 per-cycle coverage 改动）|
| nutshell-sbfl | `/home/yuan/nutshell-sbfl` | Bug 数据集 (patch + test case) |
| Verilator | `verilator --version` | >= 5.028，需 coverage 支持 |
| RISC-V GCC | `riscv64-unknown-elf-gcc` | 编译汇编测试用例 |
| Rust/Cargo | `rustc --version` | stable，编译 sv_analysis |

### 1.2 LLM 配置

文件位置: `/home/yuan/bluesfl/.env`

```bash
# === LLM Configuration ===
AGENT_TYPE=open-ai
MODEL=deepseek-v4-pro
API_KEY=<your-api-key>
API_BASE=https://api.deepseek.com

# === Project Paths ===
PROJECT_PATH=/home/yuan/NutShell/build/rtl
INCLUDE_PATHS=/home/yuan/NutShell/build/rtl,/home/yuan/NutShell/build/generated-src
TOP_MODULE=SimTop
TOP_SCOPE=TOP.SimTop.cpu.soc.nutcore
```

**说明**:
- `AGENT_TYPE=open-ai`: 使用 OpenAI 兼容 API（DeepSeek 走 OpenAI 协议）
- `MODEL=deepseek-v4-pro`: 论文用 GPT-4o，这里用 DeepSeek，都支持 tool-call
- `TOP_SCOPE`: Verilator 层级路径，NutShell 的 `cpu.soc.nutcore` 是 RTL 顶层
- `INCLUDE_PATHS`: 必须包含 `build/generated-src`（DifftestMacros.svh 在这里）

### 1.3 NutShell Scope 命名规则

NutShell 分文件 RTL 的 Verilator scope 格式:
```
TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.<module>
```
注意 `cpu.soc.nutcore` 会重复一次（Verilator 层级 vs Chisel 模块名）。BluesFL 的 `coverage/vlc.rs` 有 scope fallback 处理这种重复。

---

## 2. 论文对齐状态

### 2.1 Blues 算法 (Algorithm 1) 逐行对齐

| Algorithm 1 行 | 论文语义 | 实现 | 文件 |
|---------------|---------|------|------|
| 1-5 | BFS 队列初始化 + t<0 检查 | ✅ | `src/tracer/llm.rs` |
| 6 | `FindDrivenBlock(s)` | ✅ | `src/block/mgr.rs` |
| 7 | `IntraBlockAnalysis(s, b, t)` | ✅ | `src/tracer/slice/dynamic_slice.rs` |
| 8-11 | InterBlockAnalysis: 添加节点/边到 G | ✅ | `src/tracer/llm.rs` |
| 13 | `DataflowAnalysis(s, b, t)` → I_s | ✅ | `dynamic_slice.rs` get_driven_signals |
| 14-15 | COMB: return (I_s, t) | ✅ | `next_time = time` |
| 17-18 | SEQ covered at t-1: return (I_s, t-1) | ✅ | **coverage check at `next_time`** |
| 19-20 | SEQ not covered: return ({s}, t-1) | ✅ | `vec![sig.clone()]` |

### 2.2 与论文的实验环境差异（非算法差异）

| 论文 | 当前 | 说明 |
|------|------|------|
| Ibex (19KLoC) | NutShell | 不同处理器 |
| GPT-4o | deepseek-v4-pro | 不同 LLM |
| 119 mutated bugs | 24 nutshell-sbfl bugs | 不同数据集 |
| CoreMark | 汇编测试用例 | 不同测试程序 |
| 自动 test report | 手动构造 | 不同验证框架 |

---

## 3. 完整 E2E 流程 (以 U1 Bug 为例)

### U1 Bug 描述

| 项目 | 值 |
|------|------|
| Bug ID | U1_addiw_signext |
| 位置 | `NutShell/src/main/scala/nutcore/backend/ALU.scala` |
| 变异 | `SignExt` → `ZeroExt`（符号扩展变成零扩展）|
| 触发指令 | `addiw sp, ra, 0` |
| 现象 | `sp = 0x0000000080000000`（应为 `0xffffffff80000000`）|
| 失败周期 | cycleCnt=535, start_time=1070 |

---

### Step 0: 编译 BluesFL sv_analysis

```bash
cd /home/yuan/bluesfl
cargo build --bin sv_analysis
```

输出: `target/debug/sv_analysis`（Rust 编译的 BluesFL 核心二进制）

**说明**: sv_analysis 是 BluesFL 的入口程序，包含:
- sv-parser 解析 SystemVerilog AST
- DataFlowBlockParser 构建 block 图
- Blues BFS 反向追踪
- LLM tool-call 推理

---

### Step 1: 应用 Bug Patch

```bash
cd /home/yuan/NutShell

# 清理之前的修改
git checkout -- .

# 应用 U1 patch（把 ALU.scala 中的 SignExt 改成 ZeroExt）
git apply /home/yuan/nutshell-sbfl/patch/U1_addiw_signext.patch

# 删除有编译问题的测试文件
rm -f src/test/scala/FirrtlExportMain.scala
```

**说明**:
- `git apply` 把 Chisel 源码中的 `SignExt` 替换为 `ZeroExt`，模拟一个真实的 bug
- `FirrtlExportMain.scala` 引用了不存在的类，删除以避免编译错误

---

### Step 2: 生成分文件 RTL

```bash
cd /home/yuan/NutShell
NOOP_HOME=/home/yuan/NutShell make verilog

# 验证: NutCore.sv 应约 1665 行（不是 34409 行的扁平化版本）
wc -l build/rtl/NutCore.sv
# 预期输出: 1665 build/rtl/NutCore.sv
```

**说明**:
- `make verilog` 调用 Chisel 编译器，生成**分文件**的 SystemVerilog（每模块一个 .sv）
- 共生成约 119 个 .sv 文件在 `build/rtl/` 目录
- `NOOP_HOME` 环境变量必须设置，Chisel 编译脚本需要它来定位 `build/generated-src`
- 如果看到 `NutCore.sv` 有几万行，说明用了扁平化的预构建 RTL，必须重新 `make verilog`

**耗时**: ~60s

---

### Step 3: 编译带 Coverage 的 Verilator Emulator

```bash
cd /home/yuan/NutShell
NOOP_HOME=/home/yuan/NutShell make emu EMU_COVERAGE=1 EMU_TRACE=fst RTL_SUFFIX=sv -j$(nproc)

# 验证
ls -lh build/emu
# 应指向 build/verilator-compile/emu
```

**参数说明**:
- `EMU_COVERAGE=1`: 编译 Verilator 时加 `--coverage-line --coverage-toggle`，启用行覆盖率和信号翻转覆盖率
- `EMU_TRACE=fst`: 启用 FST 波形输出（比 VCD 更紧凑，BluesFL 用 FST 读信号值）
- `RTL_SUFFIX=sv`: 使用 `.sv` 后缀的 RTL 文件（而不是 `.v`）
- `-j$(nproc)`: 并行编译加速

**注意**: DiffTest 子模块中的 `vtransform.py` 已修改支持 `.sv` 文件（line 9）

**耗时**: ~5 min

---

### Step 4: 编译测试用例

```bash
cd /home/yuan/NutShell

# 创建链接脚本（把代码段加载到 NutShell 的 ResetVector 地址）
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

# 编译汇编测试用例
riscv64-unknown-elf-gcc -nostdlib -nostartfiles \
    -T build/u1_link.ld \
    -o build/U1_test.elf \
    /home/yuan/nutshell-sbfl/case/U1_addiw_signext.S
```

**关键说明**:
- `-T build/u1_link.ld`: **必须指定链接脚本**，把 `.text` 段放在 `0x80000000`
- NutShell 的 ResetVector 是 `0x80000000`，如果不指定，GCC 默认加载到 `0x10000`
- 地址不对时，处理器会读到 ELF header 的 magic bytes (`0x464c457f`) 当指令执行，触发 illegal instruction
- `-nostdlib -nostartfiles`: 不链接标准库（裸机汇编，没有 libc）

---

### Step 5: 运行仿真 — Per-Cycle Coverage + FST

```bash
cd /home/yuan/NutShell

BUG_ID=U1_addiw_signext
WORK_DIR=/home/yuan/bluesfl/e2e_results/${BUG_ID}_percycle
mkdir -p ${WORK_DIR}/coverage

# 运行仿真（同时收集 per-cycle coverage + FST 波形 + 日志）
./build/emu \
    -i ./build/U1_test.elf \
    --diff ./ready-to-run/riscv64-nemu-interpreter-so \
    --dump-wave \
    --dump-commit-trace \
    --dump-coverage \
    --cov-start 530 --cov-end 535 \
    --cov-dir ${WORK_DIR}/coverage \
    2>&1 | tee ${WORK_DIR}/emu_output.log
```

**参数详解**:
- `-i test.elf`: 加载编译好的测试程序
- `--diff ref.so`: 指定 DiffTest 参考模型（NEMU 动态解释器），每条指令提交时比较 DUT 和 REF 的状态
- `--dump-wave`: 生成 FST 波形文件（BluesFL 用它读信号值）
- `--dump-commit-trace`: 记录指令提交信息
- `--dump-coverage`: 启用覆盖率收集
- **`--cov-start 530`**: 从第 530 个周期开始 per-cycle coverage dump
- **`--cov-end 535`**: 到第 535 个周期结束（U1 在 cycleCnt=535 失败）
- **`--cov-dir ${WORK_DIR}/coverage`**: per-cycle coverage 文件输出目录

**Per-cycle coverage 工作原理**:
1. 在每个 posedge clock **前**: `VerilatedCov::zero()` 清零计数器
2. posedge step 执行: Verilator 累积本周期的覆盖率
3. posedge step **后**: `coverage->write()` 写入文件
4. 文件命名: `coverage_<cycles*2>_seq.dat`（×2 是 Verilator 的 time_step）
5. 每个文件只包含**这一个周期**的覆盖率数据（干净的 per-cycle 快照）

**窗口选择**: U1 在 cycle 535 失败，选择 530-535（6 个周期）。Blues BFS 从 t=1070 开始反向追踪，最多回溯到 time_bound=1064（对应 cycle 532）。6 个周期的窗口足够覆盖。

**预期输出**:
```
     sp different at pc = 0x0080000006, right = 0xffffffff80000000, wrong = 0x0000000080000000
dump coverage data to /home/yuan/NutShell/build/<timestamp>.coverage.dat...
Core 0: ABORT at pc = 0x8000000a
Core-0 instrCnt = 3, cycleCnt = 535
```

**关键信号解读**:
- `sp different`: 寄存器 sp（x2）值不匹配
- `right = 0xffffffff80000000`: 参考模型的正确值（符号扩展）
- `wrong = 0x0000000080000000`: DUT 的错误值（零扩展 → 这就是 bug）
- `instrCnt = 3`: 执行了 3 条指令就失败了
- `cycleCnt = 535`: 在第 535 个时钟周期失败

**Per-cycle coverage 文件**:
```
${WORK_DIR}/coverage/
├── coverage_1060_seq.dat  ← cycle 530 (530*2=1060)
├── coverage_1062_seq.dat  ← cycle 531
├── coverage_1064_seq.dat  ← cycle 532
├── coverage_1066_seq.dat  ← cycle 533
├── coverage_1068_seq.dat  ← cycle 534
└── coverage_1070_seq.dat  ← cycle 535 (失败周期)
```

每个文件 ~36MB（Verilator LCOV 格式）。

**耗时**: ~2s（U1 短仿真）

---

### Step 6: 准备其他 BluesFL 输入文件

```bash
# 6a. FST 波形（已由仿真生成）
FST_FILE=$(ls -t /home/yuan/NutShell/build/*.fst | head -1)
cp ${FST_FILE} ${WORK_DIR}/sim.fst

# 6b. rm_params（NutShell 无参数化模块，空 JSON 即可）
echo '{}' > ${WORK_DIR}/rm_params.tree.json
```

**说明**:
- `sim.fst`: FST 波形文件，BluesFL 通过 `read_values` tool 从中读取信号值
- `rm_params.tree.json`: 参数化模块配置（Ibex 有参数化设计，NutShell 没有，所以传空 `{}`)

---

### Step 7: 运行 BluesFL sv_analysis

```bash
cd /home/yuan/bluesfl

BUG_ID=U1_addiw_signext
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
    --start-time=1070 \
    --time-bound=1064 \
    --time-step=2 \
    --output-path=${WORK_DIR}/llm_1 \
    --vote-top-k=1 \
    --vote-total=1 \
    --test-info="Instruction addiw x2, x1, 0 at pc=0x80000006 incorrectly produces x2=0x0000000080000000 (zero-extended), whereas it should be 0xffffffff80000000 (sign-extended)" \
    --dot-env=./.env
```

**参数逐行详解**:

| 参数 | 值 | 说明 |
|------|------|------|
| `--bug-id` | `U1_addiw_signext` | 标识符，用于输出文件命名 |
| `--agent-type` | `open-ai` | 使用 OpenAI 兼容 API 协议（DeepSeek 走这个协议）|
| `--agent-mode` | `tool-call` | **论文 Section 3.4 的 tool-call 模式**。LLM 通过 4 个 tool 交互: read_values / check_signals / append_block / exit |
| `--model` | `deepseek-v4-pro` | LLM 模型，论文用 GPT-4o |
| `--project-path` | `.../build/rtl` | SystemVerilog 源文件目录（119 个 .sv 文件）|
| `--include-paths` | rtl + generated-src | `#include` 搜索路径。`generated-src` 有 DifftestMacros.svh |
| `--rm-params-path` | `rm_params.tree.json` | 参数化模块配置，NutShell 不需要，传 `{}` |
| `--coverage-path` | `.../coverage` | **per-cycle coverage 文件目录**，含 6 个 .dat 文件 |
| `--wave-path` | `sim.fst` | FST 波形文件，LLM 通过 `read_values` 读信号值 |
| `--top-module` | `SimTop` | Verilator 顶层模块名 |
| `--top-scope` | `TOP.SimTop.cpu.soc.nutcore` | Verilator 层级路径前缀 |
| **`--start-scope`** | `...backend` | **Blues BFS 起始 scope**。用 backend 而不是 ALU，让算法自然追踪到 ALU |
| **`--start-sig`** | `io_out_bits` | **Blues BFS 起始信号**。backend 的输出信号，失败时值不正确 |
| **`--start-time`** | `1070` | **Blues BFS 起始时间** = cycleCnt × time_step = 535 × 2 |
| **`--time-bound`** | `1064` | **BFS 下界** = start_time - instrCnt × time_step = 1070 - 3×2 |
| **`--time-step`** | `2` | **Verilator: 每个 clock cycle = 2 个 time unit**（posedge + negedge）|
| `--output-path` | `${WORK_DIR}/llm_1` | 输出目录 |
| `--vote-top-k` | `1` | 投票取 Top-1 |
| `--vote-total` | `1` | **单次运行**（=1 不投票）。设为 2 会跑两次取共识，但 LLM 随机性可能导致不一致 |
| `--test-info` | `"Instruction addiw..."` | **论文的 test report (I, sig, t, E)** 中的 E（期望行为描述）|
| `--dot-env` | `./.env` | LLM API 配置文件 |

**时间系统详解**:
- Verilator 每个 `step()` 是 1 time unit
- 一个 clock cycle = 2 steps (posedge + negedge) = 2 time units
- `time_step=2` 是这个约定的体现
- `start_time=1070` = cycle 535 × 2 = 第 535 个 posedge 的时间
- `time_bound=1064` = 1070 - 6 = 追踪不会回溯到早于 cycle 532
- Per-cycle coverage 文件名 `coverage_<t>_seq.dat` 中的 `t` 就是 `cycles × 2`

---

### Step 8: 查看结果

```bash
# 8a. 定位结果（最终输出）
cat ${WORK_DIR}/llm_1/llm_loc_results_${BUG_ID}.json
```

预期输出:
```json
{
  "bug_id": "U1_addiw_signext",
  "choices": [
    {
      "module_name": "ALU",
      "block_id": 5806,
      "score": 1.0
    }
  ]
}
```

**解读**: Top-1 定位到 ALU 模块的 block 5806（AssignBlock），置信度 1.0。这就是 `SignExt → ZeroExt` 的 bug 所在。

```bash
# 8b. 可疑模块列表
cat ${WORK_DIR}/llm_1/suspicious_modules.json

# 8c. Blues BFS 追踪路径（13 个 block）
python3 -c "
import json
with open('${WORK_DIR}/llm_1/trace.json') as f:
    data = json.load(f)
for i, item in enumerate(data):
    scope = item['scope'].split('.')[-1]
    btype = item['type']
    t = item['time']
    print(f'  [{i:2d}] {btype:12s} scope=...{scope:15s} t={t}')
"
```

预期 trace 路径:
```
  [ 0] ModuleOutput scope=...csr            t=1070
  [ 1] ModuleOutput scope=...alu            t=1070    ← 目标
  [ 2] ModuleOutput scope=...lsu            t=1070
  [ 3] ModuleOutput scope=...lsExecUnit     t=1070
  [ 4] ModuleOutput scope=...mdu            t=1070
  [ 5] Assign     scope=...csr            t=1070
  [ 6] Assign     scope=...alu            t=1070    ← 定位到这里
  [ 7] Assign     scope=...lsu            t=1070
  [ 8] Assign     scope=...lsExecUnit     t=1070
  [ 9] Assign     scope=...mdu            t=1070
  [10] ModuleInput scope=...exu            t=1070
  [11] ModuleInput scope=...exu            t=1070
  [12] AlwaysSeq  scope=...backend         t=1070    ← SEQ block, IntraBlockAnalysis at t-1
```

**路径解读**:
1. BFS 从 backend 的 AlwaysSeq block 开始（block 12）
2. IntraBlockAnalysis 展开 driven signals → 追踪到 exu 的 ModuleInput
3. 继续展开到 ALU、CSR、LSU、MDU 的 ModuleOutput 和 Assign blocks
4. LLM 通过 tool-call 逐一分析这些 blocks，读取信号值
5. LLM 判断 ALU 的 Assign block 是 bug 根因，调用 `append_block` 加入可疑队列
6. 最终 ALU 得分 1.0

---

### Step 9: 清理

```bash
cd /home/yuan/NutShell
git checkout -- .
```

恢复 NutShell 到干净状态，移除 bug patch。

---

## 4. Coverage 数据格式说明

### 4.1 Verilator LCOV .dat 文件

每行格式: `C '<binary_key_value_pairs>' <count>`

键值对用 `\x01key\x02value` 编码:
- `f` — 源文件路径
- `l` — 行号
- `t` — 类型 (`line` 或 `toggle`)
- `h` — Verilator 层级 (scope)
- `S` — 覆盖的行范围

### 4.2 BluesFL 文件名约定

正则匹配: `coverage_[a-z|_]*(\d+)[a-z|_]*.dat`

| 文件名 | 含义 |
|--------|------|
| `coverage_0_seq.dat` | 聚合 coverage（整个仿真）|
| `coverage_1070_seq.dat` | **Per-cycle** coverage，time=1070（cycle 535）|
| `coverage_0_comb.dat` | 组合逻辑 coverage |

### 4.3 BluesFL Coverage 解析链

```
文件名 → read_coverage_files() [bootstrap.rs]
       → 正则提取 TimeAnnotation
       → VlcCoverageReport::from_file() [vlc.rs]
       → HashMap<TimeAnnotation, VlcReport>
       → check_line_covered(btype, scope, module, time, lineno) [vlc.rs]
       → IntraBlockAnalysis 使用 per-time 查询 [dynamic_slice.rs]
```

---

## 5. 关键代码文件索引

| 文件 | 功能 | 论文对应 |
|------|------|---------|
| `src/tracer/slice/dynamic_slice.rs` | IntraBlockAnalysis | Algorithm 1 lines 13-20 |
| `src/tracer/llm.rs` | Blues BFS 主循环 + LLM 调度 | Algorithm 1 lines 1-11 |
| `src/block/dfb.rs` | Code Blockization (4 types) | Section 3.2 |
| `src/block/mgr.rs` | BlockManager, FindDrivenBlock | Algorithm 1 line 6 |
| `src/coverage/vlc.rs` | Verilator coverage 解析 + per-time 查询 | Coverage data |
| `src/bootstrap.rs` | 入口, read_coverage_files() | Setup |
| `src/agent/block/block_checker_toolcall.rs` | LLM tool-call agent | Section 3.4 |
| `difftest/.../emu/emu.cpp` | Per-cycle coverage dump | Coverage collection |
| `difftest/.../common/args.cpp` | --cov-start/--cov-end/--cov-dir | Coverage window |

---

## 6. 运行时间参考

| 阶段 | U1 耗时 | 说明 |
|------|---------|------|
| `make verilog` | ~60s | Chisel → SystemVerilog |
| `make emu` (coverage) | ~5 min | Verilator 编译 |
| 仿真 (per-cycle cov) | ~13s | 535 cycles + 6 次 coverage dump |
| `cargo build` sv_analysis | ~25s | Rust 编译（增量） |
| parse_sv + BlockManager | ~90s | 解析 119 个 .sv 文件 |
| Blues BFS + LLM | ~15 min | BFS 追踪 + LLM API 调用 |
| **总计** | ~25 min | |

---

## 7. 常见问题

### Q: Binary 加载地址不对 (0x10000 而非 0x80000000)
A: 必须用 `-T link.ld` 链接脚本，设置 `. = 0x80000000`。否则 NutShell ResetVector 读到 ELF magic bytes 当指令，触发 illegal instruction。

### Q: Per-cycle coverage 文件太大
A: 每个 .dat 文件 ~36MB。缩小 `--cov-start` 到 `--cov-end` 的窗口来减少文件数。窗口大小 = 失败周期 - time_bound/time_step ≈ 6 个周期就够了。

### Q: vote-total=2 结果为空
A: 两次独立 LLM 运行结果不一致时投票会清空结果。用 `--vote-total=1` 单次运行，或增加 `--vote-total` 到 3 取多数。

### Q: NOOP_HOME 未设置
A: `NOOP_HOME=/home/yuan/NutShell`，Chisel 编译和 verilog 生成都需要。

### Q: NutCore.sv 有几万行
A: 用了 nutshell-sbfl 预构建的扁平化 RTL。必须 `make verilog` 重新生成分文件 RTL。

---

## 8. 输出文件结构

```
e2e_results/U1_addiw_signext_percycle/
├── emu_output.log           # DiffTest 仿真日志
├── sim.fst                  # FST 波形 (85K)
├── rm_params.tree.json      # 参数配置 ({})
├── coverage/
│   ├── coverage_1060_seq.dat  # cycle 530 per-cycle coverage (~36MB)
│   ├── coverage_1062_seq.dat  # cycle 531
│   ├── coverage_1064_seq.dat  # cycle 532
│   ├── coverage_1066_seq.dat  # cycle 533
│   ├── coverage_1068_seq.dat  # cycle 534
│   └── coverage_1070_seq.dat  # cycle 535 (失败周期)
└── llm_1/
    ├── llm_loc_results_U1_addiw_signext.json  # 定位结果 (ALU, score=1.0)
    ├── suspicious_blocks.json                  # 可疑 blocks 详情
    ├── suspicious_modules.json                 # 可疑模块列表
    └── trace.json                              # Blues BFS 追踪路径
```
