# BluesFL on NutShell: 适配技术文档

> 对齐论文 "Debug Like a Human" (DAC'26, arXiv:2605.17290) 方法论结构
> 验证 bug: U6_jalr_bit0_not_cleared (ALU, bid=5841, score=1.0)
> 日期: 2026-06-03

---

## 1 背景: NutShell vs Ibex 适配差异

论文在 Ibex (19 KLoC SystemVerilog, 119 bugs) 上验证 BluesFL。
我们将其适配到 NutShell — 另一款开源 RISC-V 处理器，面临以下差异:

| 维度 | Ibex (论文) | NutShell (适配) |
|------|-----------|----------------|
| HDL 来源 | 手写 SystemVerilog | Chisel 生成 Verilog |
| 仿真框架 | Spike co-sim | DiffTest (NEMU) |
| 测试程序 | CoreMark | 每个 bug 一个汇编用例 (.S) |
| RTL 结构 | 单文件 (ibex_core.sv) | 分文件 (ALU.sv, CSR.sv, ...) |
| Bug 数量 | 119 | 24 |
| 信号命名 | 原始名 | Chisel 重命名 (需映射表) |
| Scope 层级 | `TOP.u_top.<mod>` | `TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.<mod>` |
| FST 时间 | posedge 周期数 | Verilator 时间单位 (非 cycle×2) |

这些差异影响 BluesFL 流程的每一步，下面按论文方法论结构逐一对齐。

---

## 2 Test Report: (I, sig, t, E)

论文 Section 2.1 定义 test report 为四元组 `(I, sig, t, E)`:
- **I**: 失败的指令
- **sig**: 检测到错误的信号
- **t**: 失败时间 (posedge 时钟周期)
- **E**: 期望行为的自然语言描述

### 2.1 论文 (Ibex) 的生成方式

```
输入: ibex_simple_system.log + mismatch_log.txt
解析: "FAILURE: Co-simulation mismatch at time X"
      "PC_MISMATCH ORACLE=0x..."
公式: start_time = failure_time - 1 - time_step
      time_bound  = start_time - 2 × ibex_cycle
信号: PC mismatch → "rvfi_pc_wdata"
      WDATA mismatch → "rvfi_rd_wdata_d"
```

### 2.2 NutShell 的生成方式

NutShell 的 DiffTest 不产生 `mismatch_log.txt`，错误信息直接输出到 stderr:

```
Core 0: ABORT at pc = 0x80000012                           ← mismatch 检测
Core-0 instrCnt = 4, cycleCnt = 1,536                      ← 执行统计
[03] commit pc 000000008000000c inst 00008067 wen 0 ...     ← 指令追踪
```

用 `nutshell_test_analysis.py` 解析上述输出，生成 test report:

```
输入: emu stderr + stdout (通过 --dump-commit-trace 获取指令追踪)
解析: 正则匹配 ABORT / commit pc / instrCnt / REPORT_DIFFERENCE
信号: PC mismatch → "io_redirect_target"
      寄存器 mismatch → "io_out_bits"
时间: 无法自动计算! FST 时间 ≠ cycle × 2, 必须手动探测
```

### 2.3 U6 实际 test report

| 字段 | 论文 Ibex 示例 (Figure 6) | U6 NutShell 实际 |
|------|--------------------------|-----------------|
| **I** | `jmp pc + 0xa0c0` | `jalr x1, 0(x1)` (inst=0x00008067, pc=0x8000000c) |
| **sig** | `rvfi_pc_wdata` | `io_redirect_target` |
| **t** | `19` | `515` (手动探测) |
| **E** | "Instruction jmp incorrectly jumps to 0x000F5FC0, should jump to 0x0010A140" | "ABORT at pc=0x80000012 after 4 instructions, last commit pc=0x8000000c inst=0x00008067" |

**关键差异**: 论文的 `t` 由 co-simulation 日志直接给出；NutShell 需要手动从 FST 波形探测。

### 2.4 生成 test_info.json 的命令

```bash
python3 scripts/nutshell_test_analysis.py \
    --emu-log /path/to/emu_output.log \
    --bug-id U6_jalr_bit0_not_cleared \
    --output test_info.json \
    --start-time 515    # 手动覆盖
```

输出结构:
```json
{
  "bug_id": "U6_jalr_bit0_not_cleared",
  "start_scope": "TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend",
  "start_sig": "io_redirect_target",
  "start_time": 515,
  "test_info": "ABORT at pc=0x80000012 after 4 instructions, ...",
  "time_bound": 485,
  "time_step": 2,
  "top_module": "SimTop",
  "top_scope": "TOP.SimTop.cpu.soc.nutcore",
  "commit_instrs": [
    {"idx": 0, "pc": 0x80000000, "inst": 0x97, "wen": 1, "dst": 1, "data": 0x80000000},
    {"idx": 1, "pc": 0x80000004, "inst": 0x1010093, "wen": 1, "dst": 1, "data": 0x80000010},
    {"idx": 2, "pc": 0x80000008, "inst": 0x10e187, "wen": 1, "dst": 1, "data": 0x80000011},
    {"idx": 3, "pc": 0x8000000c, "inst": 0x8067, "wen": 0, "dst": 0, "data": 0x80000011}
  ]
}
```

### 2.5 start_time 探测方法

NutShell FST 时间单位不等于 cycle × 2。探测步骤:

1. 用 `--start-time 0` 运行 sv_analysis
2. 在 sv_analysis 日志中观察信号值首次出现错误的时间
3. 或用 GTKWave 打开 FST，定位 `io_redirect_target` 首次错误值的时间
4. 用确定的时间重新运行

U6 探测结果: `start_time = 515`

---

## 3 Code Blockization: 数据流代码块化

论文 Section 3.2 将 HDL 源码划分为 4 种互不相交的 code block:

| Block 类型 | Vi (输入) | Vo (输出) | 说明 |
|-----------|---------|---------|------|
| ModInputBlock | 父模块连接信号 | 此端口信号 | 跨模块入 |
| ModOutputBlock | 此端口信号 | 父模块连接信号 | 跨模块出 |
| AssignBlock | RHS 信号 | LHS 信号 | 连续赋值，可合并 |
| AlwaysBlock | RHS + 条件信号 | LHS 信号 | 时序/组合逻辑 |

### 3.1 论文 (Ibex) 的输入

- 单文件 `ibex_core.sv` (19 KLoC)
- sv-parser 直接解析
- 产出: 1,329 个 blocks

### 3.2 NutShell 的输入

- **分文件 RTL**: `build/rtl/` 下 119 个 .sv 文件 (ALU.sv, CSR.sv, ...)
- 由 `make verilog` 从 Chisel 编译生成
- **不能用** nutshell-sbfl 预构建的扁平化 RTL (NutCore.sv 34,409 行)，否则:
  - parse_sv: 155s (vs 正常 2.8s)
  - BlockManager: 30+ min 超时 (vs 正常 43s)

### 3.3 性能对比

| RTL 来源 | NutCore.sv 行数 | parse_sv | BlockManager | 产出 blocks |
|----------|----------------|----------|-------------|------------|
| `make verilog` (分文件) | 1,665 | 337s (全部 119 文件) | 43s | ~6,000+ |
| 预构建 (扁平化) | 34,409 | 155s (单文件) | >30min (超时) | — |

### 3.4 生成带 bug RTL 的命令

```bash
cd /home/yuan/nutshell-sbfl
git checkout -- .
git apply patch/U6_jalr_bit0_not_cleared.patch
make verilog       # Chisel → 分文件 SystemVerilog
```

验证:
```bash
wc -l build/rtl/NutCore.sv
# 必须是 ~1,665 行，不能是 34,000+
```

### 3.5 额外依赖: signal_name_map.json

Chisel 生成的 Verilog 信号名与源码不同 (例如 `io_redirect_target` 可能在 Verilog 中变为 `_T_123`)。
需要 `signal_name_map.json` (7375 行) 提供 Chisel→Verilog 信号名映射。

此文件放置于 `$SV_ANALYSIS_HOME` 目录 (即 BluesFL 项目根目录)，sv_analysis 自动加载。

> 论文 (Ibex) 直接使用手写 SystemVerilog，不需要此映射。

---

## 4 Blues: Block-Level 指令导向切片

论文 Algorithm 1 从 `(sig, t)` 出发，反向追踪构建指令执行路径 G:

```
1. 初始化队列 S = {(sig, t)}
2. while S 非空:
3.   (s, t_cur) ← Pop(S)
4.   b ← FindDrivenBlock(s)                  // Inter-Block
5.   (driven_signals, t') ← IntraBlock(s, b, t_cur)
6.   for each s_i in driven_signals:
7.     b' ← FindDrivenBlock(s_i)              // Inter-Block
8.     G 中添加节点 (b', t') 和边 (b',t')→(b,t_cur)
9.     Push(s_i, t') 到 S
```

### 4.1 Intra-Block Analysis 时间标注

| Block 类型 | 规则 | 说明 |
|-----------|------|------|
| 组合 (Assign/ModInput/ModOutput) | `t' = t` | 同一时刻传播 |
| 时序 (AlwaysBlock @posedge clk) | 赋值在 t-1 覆盖 → `t' = t-1` | 上一拍驱动 |
| 时序 (AlwaysBlock @posedge clk) | 未覆盖 → `t' = t-1`, 信号驱动自身 | 寄存器保持 |

NutShell 的 `time_step = 2` (每个时钟周期 = 2 个 posedge 时间单位)。

### 4.2 Inter-Block Analysis: 跨模块追踪

论文通过 ModInputBlock/ModOutputBlock 的 Vi/Vo 映射实现跨模块追踪。

NutShell 分文件 RTL 的 scope 比论文 (Ibex) 多一层:

```
Ibex:    TOP.ibex_simple_system.u_top.if_stage
NutShell: TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend.exu.alu
                                  ^^^^^^^^^^^^^^^^^ 额外层级
```

因此需要 ModuleChecker 组件，在 Blues 追踪到模块边界时:
1. 找到当前 scope 的所有子模块 (WBU, MOU, CSR, ALU, ...)
2. 利用覆盖率信息选择哪些子模块可能被覆盖
3. 进入选中子模块继续追踪

### 4.3 U6 实际 trace

```
(backend, t=515, sig=io_redirect_target)
  → ModuleOutput × 4: {WBU, MOU, CSR, ALU}
    → ModuleChecker 选中 ALU (coverage=1)
      → ALU AssignBlock (bid=5841) ← bug 在这里
```

论文 Figure 6 的 Ibex trace:
```
(Top, t=19) → if_stage → ... → ex_stage → alu (bug)
```

两者结构一致: 都是从顶层逐步追踪到 bug 所在的 ALU AssignBlock。

---

## 5 LLM Reasoning and Ranking

论文 Section 3.4 + Figure 3 定义 LLM 在每个状态 `(b, t)` 进行推理:

### 5.1 Prompt 结构

论文 Figure 3 的 prompt 模板:

```
System: "You are a debugging assistant for a RISCV microprocessor design team..."

User:
  # Simulation fault information
  {test_report}              ← 论文的 E (test_info)

  # Code Snippet
  {code_block_context}       ← 当前 block 的 Verilog 代码

  # Driven signals           ← IntraBlockAnalysis 产生的输入信号
  [{"name":"res", "time":15, "width":16, "value":"0x5fc0"}, ...]
```

我们的实现对应文件:

| 论文模板字段 | 实现文件 | 替换变量 |
|------------|---------|---------|
| System prompt | `prompts/block_checker_system.md` | — (含 2 个完整 few-shot 示例) |
| `{test_report}` | `prompts/block_checker_2.md` | `{scenario}` (由 test_info + module_name 拼接) |
| `{code_block_context}` | 同上 | `{block_code}` (block.get_ctx()) |
| `{driven_signals}` | 同上 | `{sig_value}` + `{input_wave}` (拆分为输出信号 + 输入信号) |

信号表示格式与论文一致:
```json
{"signal_name": "adder_result_o", "time": 15, "bit-width": 32, "value": "0x000f5fc0"}
```

### 5.2 Tool Calls

论文定义 4 个 LLM tool call:

| 论文 Tool | 功能 | 我们实现为 |
|----------|------|-----------|
| `exit` | 终止调试循环 | JSON 响应 `terminate: true` |
| `append_block` | 标记 block 可疑 | JSON 响应 `suspicious: true` → 自动加入队列 |
| `check_signals` | 选择进一步检查的上游信号 | JSON 响应 `check_signals: [{name, time}]` |
| `read_values` | 从波形读取信号值 | 自动嵌入 prompt (在调用 LLM 前从 FST 读取) |

**关键差异**: 论文的 tool call 是 LLM 主动多轮调用；我们的实现采用**投票制** —
对每个 block 发送 `vote_total` 次并发 LLM 请求，每次返回完整 JSON 决策，
然后多数投票决定是否 suspicious、选择哪些 check_signals。

### 5.3 投票参数

```bash
--vote-total 2     # 每个 block 的 LLM 投票轮数
--vote-top-k 1     # Top-K 选择
```

U6 实际 LLM 交互 (从日志):
```
LLM vote task 0 starting for sig=io_redirect_target
LLM vote task 1 starting for sig=io_redirect_target
  → task 0: suspicious=false, terminate=false, check_signals=[io_instrValid]
  → task 1: suspicious=false, terminate=false, check_signals=[mepc, io_in_valid, io_instrValid]
Voting result: not_dive=2, suspicious=0 → 继续追踪

... (多轮投票) ...

LLM vote task 0 starting for sig=io_redirect_target  (在 ALU block)
  → task 0: suspicious=true, terminate=false, check_signals=[io_in_bits_func, ...]
  → task 1: (无有效 JSON)
Voting result: not_dive=1, suspicious=1 → 标记为可疑

... (最终 rerank) ...

Ranking: ALU bid=5841 score=1.0
```

### 5.4 LLM 配置

```bash
# .env 文件
AGENT_TYPE=open-ai
MODEL=deepseek-v4-flash        # 1M context, 推荐用于 NutShell
API_KEY=sk-xxx
API_BASE=https://api.deepseek.com
```

| 模型 | Context | 论文对应 | NutShell 实测 |
|------|---------|---------|-------------|
| deepseek-v4-flash | 1M | — | 推荐，低成本 |
| deepseek-chat | ~256K | — | U6 成功 (247K tokens), 偶发超限 |
| gpt-4o-mini | 128K | Top-1=17 | 论文基准模型 |

> NutShell 的 prompt 比 Ibex 大得多 (227K vs ~30K tokens)，因为:
> - Chisel 生成的信号名更长
> - 分文件 RTL 导致 block 的 context 更大
> - `input_wave` 中的信号数量更多

---

## 6 端到端流程: U6 完整命令

### 6.1 Step 1-2: Build

```bash
cd /home/yuan/nutshell-sbfl
git checkout -- .
git apply patch/U6_jalr_bit0_not_cleared.patch
make verilog
EMU_TRACE=fst make emu RTL_SUFFIX=sv
```

### 6.2 Step 3: Simulate

```bash
riscv64-linux-gnu-gcc -nostdlib -nostartfiles -T case/link.ld \
    -o build/U6_test.bin case/U6_jalr_bit0_not_cleared.S

./build/emu \
    -i build/U6_test.bin \
    --diff build/riscv64-nemu-interpreter-so \
    --dump-commit-trace \
    --dump-wave \
    --wave-path=build/U6_wave.fst \
    2>&1 | tee build/emu_output.log
```

### 6.3 Step 4: Test Report

```bash
cd /home/yuan/bluesfl

python3 scripts/nutshell_test_analysis.py \
    --emu-log /home/yuan/nutshell-sbfl/build/emu_output.log \
    --bug-id U6_jalr_bit0_not_cleared \
    --output test_info.json \
    --start-time 515
```

### 6.4 Step 5: sv_analysis

```bash
cd /home/yuan/bluesfl
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
    --test-info "ABORT at pc=0x80000012 after 4 instructions, last commit pc=0x8000000c inst=0x00008067"
```

**参数与论文概念的映射**:

| 参数 | 论文概念 |
|------|---------|
| `--project-path` | ❶ Code Blockization 的输入 (RTL 文件目录) |
| `--start-sig` / `--start-time` | Blues 的初始 `(sig, t)` |
| `--time-bound` | Blues 回溯上限 (避免回溯到 reset) |
| `--wave-path` | read_values 的 FST 波形数据源 |
| `--test-info` | Test Report 的 `E` (嵌入 LLM prompt 的 `{test_report}`) |
| `--vote-total` / `--vote-top-k` | LLM 投票策略 (非论文原始设计) |

### 6.5 结果

```
e2e_results/U6_t515/
├── llm_loc_results_U6_backend.json    # ❹ 最终排名
├── suspicious_blocks.json             # ❸ 可疑 block 队列
├── suspicious_modules.json            # ModuleChecker 输出
└── trace.json                         # ❷ Blues 执行路径 G
```

```json
{
  "bug_id": "U6_backend",
  "token_usage": { "input_tokens": 227332, "output_tokens": 20226, "total_tokens": 247558 },
  "choices": [
    { "module_name": "ALU", "block_id": 5841, "score": 1.0 }
  ]
}
```

### 6.6 各阶段耗时

| 阶段 | 耗时 | 论文对应 |
|------|------|---------|
| parse_sv (119 files) | 337s | ❶ Code Blockization |
| BlockManager (数据流图) | 43s | ❶ |
| Blues (BFS 追踪) | <1s | ❷ |
| LLM 投票 | ~120s | ❸ |
| Ranking | <1s | ❹ |
| **总计** | ~8-10 min | — |

---

## 7 Oracle 与 Metric

论文 Table 1 使用 Top-N metric: 在排序结果的 Top-N 中是否包含 bug block。

### 7.1 Oracle 格式

每个 bug 一个 JSON 文件 + 汇总的 `oracle_info.json`:

```json
{
  "bid": 5841,
  "module_name": "ALU",
  "scope_name": "TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend.exu.alu",
  "description": "JALR target bit0 not cleared",
  "bug_file": "src/main/scala/nutcore/backend/fu/ALU.scala"
}
```

`bid` 是论文的 Block ID — 通过首次成功运行 sv_analysis 后，从 `suspicious_blocks.json` 中获取。

### 7.2 收集 + 计算命令

```bash
cd /home/yuan/bluesfl

# 收集所有 llm_loc_results_*.json
python3 scripts/collect_loc_results.py \
    --root ./e2e_results \
    --output ./e2e_results/merged_results.json \
    --prefix llm

# 计算 Top-1/Top-5/Top-10
cargo run --bin cal_metric -- \
    --predictions ./e2e_results/merged_results.json \
    --oracle scripts/nutshell_oracle \
    --model-name deepseek-v4-flash
```

---

## 8 自动化脚本

| 脚本 | 功能 | 命令 |
|------|------|------|
| `scripts/nutshell_test_analysis.py` | 生成 Test Report | `python3` |
| `scripts/nutshell_fl_run_all.py` | 批量 build→sim→analyze | `python3` |
| `exps/run_nutshell.sh` | 实验入口 (pipeline + metric) | `bash` |

### 批量运行示例

```bash
# 单个 bug (已有 build 和 FST, 只跑分析)
START_TIME=515 ./exps/run_nutshell.sh --skip-build --skip-sim U6

# 全量 (需改 run_nutshell.sh 中 MODELS 数组)
./exps/run_nutshell.sh
```

---

## 9 24 个 Bug 的 start_time 待测表

| Bug ID | Oracle Module | start_time | Bug 描述 |
|--------|--------------|------------|---------|
| U6_jalr_bit0_not_cleared | ALU | **515** | JALR 目标 bit0 未清零 |
| U1_addiw_signext | ALU | ? | ADDIW 符号扩展错误 |
| U7_lb_lbu_ext_swap | UnpipelinedLSU | ? | LB/LBU 扩展互换 |
| M1_div_by_zero | MDU | ? | DIV 除零语义错误 |
| D1_sub_sra_decode | RVI | ? | SUB/SRA 解码混淆 |
| C1_alu_forwarding_disabled | ISU | ? | ALU 前递被禁用 |
| C3_branch_no_redirect | ALU | ? | 分支跳转未重定向 |
| RE1_branch_misaligned | BRU | ? | 分支对齐检查缺失 |
| RE2_store_misaligned_flush | UnpipelinedLSU | ? | Store 对齐异常未抑制写 |
| P4_mepc_save_error | CSR | ? | mepc 保存错误 |
| P6_mret_privilege_mode | CSR | ? | MRET 特权级恢复错误 |
| NE1_mtval_high_bits | CSR | ? | mtval 高位不可写 |
| NE3_xret_privilege_check | CSR | ? | U-mode SRET 未 trap |
| X1_fence_fencei_field_check | RVZifencei | ? | FENCE 保留字段检查缺失 |
| X2_load_store_funct3_illegal | RVI | ? | 非法 funct3 未 trap |
| X3_csr_privilege_access | CSR | ? | 特权 CSR 访问检查缺失 |
| X4_exception_type_priority | EXU | ? | 异常优先级错误 |
| X5_lr_sc_reservation | UnpipelinedLSU | ? | LR/SC 预约错误 |
| X6_minstret_count_error | CSR | ? | minstret 计数错误 |
| E1_precise_exception_writeback | EXU | ? | 异常后 younger 指令仍提交 |
| E2_load_exception_writeback | EXU | ? | Load fault 仍写寄存器 |
| PT4_pte_rw_legality_check | EmbeddedTLB | ? | PTE R/W 检查缺失 |
| PT9_superpage_mask_error | EmbeddedTLB | ? | 超页掩码错误 |
| PT10_sfence_vma_flush_disabled | EmbeddedTLB | ? | SFENCE.VMA 未刷新 TLB |

---

## 10 常见问题

**Q: NutCore.sv 有 34,000 行**
A: 用了预构建扁平化 RTL。必须 `make verilog` 重新生成。扁平化导致 parse 155s + BlockManager 超时。

**Q: DifftestMacros.svh 找不到**
A: `--include-paths` 必须包含 `build/generated-src`。

**Q: LLM context window limit**
A: 换 `deepseek-v4-flash` (1M context)。NutShell prompt ~227K tokens，远超 Ibex 的 ~30K。

**Q: token_usage=0, choices 为空**
A: Blues 未走到 Assign/Always block。检查 start_time 是否正确。

**Q: trace 太短**
A: 检查 start_scope 和 start_sig 是否匹配仿真错误。start_sig 一般是 `io_redirect_target` (PC mismatch)。

**Q: SV_ANALYSIS_HOME 未设置**
A: `export SV_ANALYSIS_HOME=/home/yuan/bluesfl`
