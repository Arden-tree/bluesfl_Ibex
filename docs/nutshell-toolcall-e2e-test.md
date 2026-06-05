# NutShell BluesFL Tool-Call 端到端测试报告

> 日期: 2026-06-05
> 分支: `fix/test-analysis-alignment` (与 `feat/nutshell-pipeline` 指向同一 commit `e544412`)
> Bug: U6_jalr_bit0_not_cleared (bid=5841, ALU)
> 模型: deepseek-v4-pro
> 模式: tool-call (论文 Section 3.4 描述的方式)

---

## 1. 背景：Voting vs Tool-Call

BluesFL 支持两种 Block Checker LLM 交互模式：

| | Voting (默认) | Tool-Call (论文 Section 3.4) |
|---|---|---|
| 交互方式 | 单轮 prompt + JSON 解析 | 多轮 tool-call (最多 20 轮) |
| 信号值 | 全部预填 prompt | `read_values` 按需读取 |
| 工具 | 无 | `read_values` / `check_signals` / `append_block` / `exit` |
| 决策 | LLM 返回 JSON: `{suspicious, check_signals, terminate}` | LLM 通过 tool 调用交互式决策 |
| 参数 | `--agent-mode voting` (默认) | `--agent-mode tool-call` |
| 上游代码仓 | 有 (`block_checker.rs`) | **无** (本地新增) |

**注意**: 上游代码仓 (https://github.com/pointerliu/bluesfl master) 只开源了 voting 模式。
Tool-call 模式的实现文件 (`block_checker_toolcall.rs`, `toolcall_tools.rs`, prompt 文件) 是本地新增的。

---

## 2. Tool-Call 实现文件

| 文件 | 功能 |
|------|------|
| `src/agent/block/block_checker_toolcall.rs` | BlockCheckerToolAgent — 多轮 tool-call 入口 |
| `src/agent/block/toolcall_tools.rs` | 4 个 tool 实现 + shared state |
| `src/agent/block/mod.rs` | 模块导出 |
| `prompts/block_checker_toolcall_system.md` | 系统提示词 (角色、工具说明、工作流、示例) |
| `prompts/block_checker_toolcall.md` | 用户提示词模板 (scenario、code、sig_value、driven_signals) |
| `src/bootstrap.rs` | `AgentMode::ToolCall` 分支 + `BlockCheckerEnum` 调度 |

### 2.1 四个 Tool

```rust
// 1. read_values — 从波形读取信号值
//    输入: [{name, time}, ...]
//    输出: [{signal_name, time, bit-width, value}, ...]

// 2. check_signals — 选择上游信号继续追踪
//    输入: [{name, time}, ...]  (必须来自 driven_signals 列表)
//    输出: {status, signals_count}

// 3. append_block — 标记当前 block 为可疑
//    输入: {reason: string}
//    输出: {status}

// 4. exit — 终止分析
//    输入: {reason: string}
//    输出: {status}
```

### 2.2 工作流

```
LLM 收到 prompt (code + sig_value + driven_signals)
  → 可多次调用 read_values 查看信号值
  → 决策:
    a) append_block + exit   → 当前 block 可疑
    b) check_signals + exit  → 追踪上游信号
    c) exit                  → 不可疑，无上游值得追踪
  → 最多 20 轮 tool 调用
```

### 2.3 Shared State

```rust
pub struct ToolCallState {
    pub suspicious: bool,          // append_block 设为 true
    pub terminate: bool,           // exit 设为 true
    pub checked_signals: Vec<(String, TimeAnnotation)>,  // check_signals 收集
}
```

---

## 3. 端到端复现步骤 (U6)

### 3.0 前置条件

```bash
# 已有文件 (由 nutshell_fl_run_all.py 或手动准备)
e2e_results/nutshell/U6_jalr_bit0_not_cleared/
├── sim.fst                  # FST 波形 (117KB)
├── test_info.json           # 包含 start_time=515
├── coverage/                # Verilator 覆盖率
├── rm_params.tree.json      # 参数
└── emu_output.log           # 仿真日志

# NutShell 分文件 RTL (必须用 make verilog 生成)
/home/yuan/NutShell/build/rtl/        # NutCore.sv ~1,665 行
/home/yuan/NutShell/build/generated-src/  # DifftestMacros.svh
```

### 3.1 编译 sv_analysis

```bash
cd /home/yuan/bluesfl
cargo build --bin sv_analysis
# 输出: target/debug/sv_analysis
```

### 3.2 运行 sv_analysis (tool-call 模式)

```bash
cd /home/yuan/bluesfl

API_KEY=sk-xxx \
API_BASE=https://api.deepseek.com \
SV_ANALYSIS_HOME=/home/yuan/bluesfl \
./target/debug/sv_analysis \
  --bug-id=U6_jalr_bit0_not_cleared \
  --agent-type=open-ai \
  --agent-mode tool-call \
  --model=deepseek-v4-pro \
  --project-path=/home/yuan/NutShell/build/rtl \
  --include-paths=/home/yuan/NutShell/build/rtl \
  --include-paths=/home/yuan/NutShell/build/generated-src \
  --rm-params-path=./e2e_results/nutshell/U6_jalr_bit0_not_cleared/rm_params.tree.json \
  --coverage-path=./e2e_results/nutshell/U6_jalr_bit0_not_cleared/coverage \
  --wave-path=./e2e_results/nutshell/U6_jalr_bit0_not_cleared/sim.fst \
  --top-module=SimTop \
  --top-scope=TOP.SimTop.cpu.soc.nutcore \
  --start-scope=TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend \
  --start-sig=io_redirect_target \
  --start-time=515 \
  --time-bound=485 \
  --time-step=2 \
  --output-path=./e2e_results/nutshell/U6_jalr_bit0_not_cleared/llm_4 \
  --vote-top-k=1 \
  --vote-total=2 \
  --test-info="ABORT at pc=0x80000012 after 4 instructions, last commit pc=0x8000000c inst=0x00008067"
```

**关键参数**:
- `--agent-mode tool-call` — 切换到 tool-call 模式 (默认是 voting)
- `--model=deepseek-v4-pro` — tool calling 支持较好的模型
- `--project-path` — **必须用 NutShell make verilog 生成的分文件 RTL** (~1.6K 行 NutCore.sv)
- `--include-paths` — 两个路径: RTL 目录 + generated-src 目录

### 3.3 环境变量

| 变量 | 值 | 说明 |
|------|-----|------|
| `API_KEY` | DeepSeek API key | LLM 调用 |
| `API_BASE` | `https://api.deepseek.com` | OpenAI 兼容 API |
| `SV_ANALYSIS_HOME` | `/home/yuan/bluesfl` | signal_name_map.json 所在目录 |

---

## 4. 运行结果

### 4.1 定位结果

```json
{
  "bug_id": "U6_jalr_bit0_not_cleared",
  "token_usage": {
    "input_tokens": 202935,
    "output_tokens": 29100,
    "total_tokens": 232035
  },
  "choices": [
    {
      "module_name": "ALU",
      "block_id": 5841,
      "score": 1.0
    }
  ]
}
```

**Top-1 命中**: ALU (bid=5841, score=1.0)

### 4.2 运行过程

```
阶段 1: parse_sv (119 files)
  NutCore.sv: 3.16s (对比扁平化 RTL 的 155s)
  总计: ~40s

阶段 2: BlockManager (数据流分析)
  ~50s

阶段 3: ModuleChecker + DIVE 决策
  DIVE → ALU (io_redirect_target@515)
  DIVE → WBU (io_redirect_target@515)
  DIVE → CSR (io_redirect_target@515)
  DIVE → MOU (io_redirect_target@515)

阶段 4: Tool-Call Agent (LLM 交互)
  ALU block:
    → LLM 多轮 read_values 查看信号
    → append_block: "Operator precedence bug in taken wire..."
    → ToolCall result: suspicious=true, terminate=true
  其他模块:
    → suspicious=false, terminate=true

阶段 5: Block Reranker → 输出最终排名

总耗时: ~130 min (CSR dataflow 追踪展开大量 perfCnts 信号)
```

### 4.3 输出文件

```
e2e_results/nutshell/U6_jalr_bit0_not_cleared/llm_4/
├── llm_loc_results_U6_jalr_bit0_not_cleared.json  (302B)  — 定位结果
├── suspicious_blocks.json                          (5.1KB) — 可疑 block 列表
├── suspicious_modules.json                         (1.0KB) — 可疑模块
└── trace.json                                      (118KB) — 完整追踪路径
```

备份位置: `llm_4_toolcall_deepseek_v4_pro/`

---

## 5. 三次运行对比

| # | 模式 | 模型 | RTL 来源 | 结果 | Token | 耗时 |
|---|------|------|----------|------|-------|------|
| 1 | voting | deepseek-chat | NutShell make verilog | **ALU bid=5841** | 247K | ~10min |
| 2 | voting | deepseek-v4-pro | NutShell make verilog | Backend_inorder bid=2675 (偏) | 419K | ~90min |
| 3 | **tool-call** | **deepseek-v4-pro** | **NutShell make verilog** | **ALU bid=5841** | 232K | ~130min |

**关键发现**:
- deepseek-chat voting 成功 (#1) 是因为 dataflow 追踪直接到了 ALU，LLM 投票确认
- deepseek-v4-pro voting 失败 (#2) — 追踪展开到更多模块后投票判偏
- **deepseek-v4-pro tool-call 成功 (#3)** — LLM 通过多轮 read_values 自主分析信号，准确定位 ALU

---

## 6. 已知问题

### 6.1 CSR Dataflow 追踪展开过慢

分文件 RTL 中 CSR 模块有大量 perfCnts/perfCntCond 信号 (~7700 条 SEQ fallback)，
导致 dataflow 追踪耗时 60+ 分钟。这是 NutShell 特有的问题 (Ibex 没有这么多性能计数器)。

可能的优化方向:
- 在 ModuleChecker 阶段跳过 CSR (不 DIVE)
- 在 dynamic_slice 中对 perfCnts 信号剪枝
- 限制单次追踪的信号数量

### 6.2 LLM 分析理由不完全准确

Tool-call agent 定位到了 ALU，但分析理由是 "operator precedence bug in taken wire"。
实际 U6 bug 是 JALR 的 `io_redirect_target` 没有清除 bit0。定位正确但解释有偏差。

### 6.3 Driven Signals 值未预填

当前 tool-call 实现只提供 driven signals 的 name + time，不提供值。
论文 Figure 3 显示应该预填 signal values (name, time, width, value)。
LLM 需要额外调用 read_values 获取值，增加了 token 消耗。

---

## 7. 与论文的对齐情况

| 论文描述 | 本地实现 | 对齐 |
|----------|----------|------|
| 4 个 tool (read_values, check_signals, append_block, exit) | 一致 | ✓ |
| Multi-turn 交互 (论文未指定轮数) | 20 轮上限 | ✓ |
| Driven signals 预填值 + read_values 按需读 | 仅提供 name+time，通过 read_values 读 | △ |
| GPT-4o 模型 | deepseek-v4-pro | △ |
| Ibex 119 bugs | NutShell 24 bugs | △ |
| Code Blockization | DataFlowBlockParser | ✓ |
| Blues 追踪算法 | Intra-Block + Inter-Block | ✓ |
| Module Checker (DIVE/SKIP) | 已恢复 | ✓ |
| Block Reranker | 已实现 | ✓ |

图例: ✓ 完全对齐 | △ 替代方案/有差异

---

## 8. 日志查看

```bash
# 查看 tool-call agent 的决策日志
grep "ToolCall\|append_block\|suspicious=true\|MaxDepth\|read_values" logs/sv-analysis_rCURRENT.log

# 查看 DIVE/SKIP 决策
grep "DIVE\|NOT DIVE" logs/sv-analysis_rCURRENT.log

# 查看完整日志
cat logs/sv-analysis_rCURRENT.log
```
