# NutShell + BluesFL Bug 测试全流程

本文档记录如何在 NutShell 处理器上复现 BluesFL 论文的故障定位流程，以 U6 bug (JALR bit0 未清除) 为例。

## 与论文流程的对应关系

BluesFL 论文 (DAC 2026) 以 Ibex 处理器为实验平台，整体流程为：

```
论文流程 (Ibex)                          本文档 (NutShell)
─────────────────────                    ─────────────────────
1. Verilator 编译 (带 rm-params patch)   → Step 1-2: Chisel → Verilog + emu 编译
2. Co-simulation (Ibex vs Spike)        → Step 3-4: emu + DiffTest (NutShell vs NEMU)
3. test_analysis 解析 mismatch_log.txt  → Step 5: 确定 start_time
   自动生成 test_info.json                  (当前手动，缺少 NutShell 版 test_analysis)
4. fl_run_all.py 批量调用 sv_analysis    → Step 6: 手动调用 sv_analysis
5. collect + cal_metric 计算指标         → 结果解读
```

**主要差异**：

| 环节 | 论文 (Ibex) | NutShell 适配 |
|------|------------|--------------|
| RTL 来源 | Verilog (直接) | Chisel → firtool → 分文件 Verilog |
| 仿真器 | Verilator 原生 | NutShell DiffTest 框架 |
| 参考模型 | Spike | NEMU (riscv64-nemu-interpreter-so) |
| 错误检测 | co-sim 输出 mismatch_log.txt | DiffTest ABORT 输出 |
| start_time 确定 | test_analysis 自动解析 | 手动（粗跑一次从日志获取） |
| 覆盖率 | 自定义 Verilator patch 生成 rm_params | 暂不使用（空 JSON 占位） |
| scope 格式 | `TOP.ibex_simple_system.u_top...` | `TOP.SimTop.cpu.soc.nutcore...` (多一层) |
| 信号名映射 | 不需要（Verilog 原生名） | 需要 signal_name_map.json (Chisel 重命名) |

## 目录

1. [环境依赖](#1-环境依赖)
2. [仓库结构](#2-仓库结构)
3. [Step 1: 应用 Bug Patch 并生成 RTL](#step-1)
4. [Step 2: 编译仿真器 (emu) 并启用 FST 波形](#step-2)
5. [Step 3: 编译测试用例](#step-3)
6. [Step 4: 运行仿真生成 FST 波形](#step-4)
7. [Step 5: 确定 start_time](#step-5)
8. [Step 6: 运行 BluesFL](#step-6)
9. [结果解读](#结果解读)
10. [常见问题排查](#常见问题排查)
11. [测试其他 Bug](#测试其他-bug)
12. [完整一键脚本参考](#完整一键脚本参考)

---

## 1. 环境依赖

| 工具 | 用途 | 论文对应 | 安装检查 |
|------|------|---------|---------|
| mill | Chisel/Scala 编译 | 无 (Ibex 用 Verilog) | `which mill` |
| firtool | CIRCT FIRRTL 编译器 | 无 | `which firtool` |
| java (17+) | mill 运行时 | 无 | `java -version` |
| riscv64-unknown-elf-gcc | 测试用例交叉编译 | 同 | `which riscv64-unknown-elf-gcc` |
| Verilator (5.048+) | C++ 仿真生成 | 5.037 + 自定义 patch | `verilator --version` |
| rustc + cargo | BluesFL 编译 | 同 | `rustc --version` |

论文还依赖自定义 Verilator patch (`0001-verilator-dump-rm-params.patch`) 来生成 `rm_params.tree.json`（参数消除后的覆盖率数据）。NutShell 适配中暂不需要此 patch。

## 2. 仓库结构

```
/home/yuan/
├── NutShell/                  # NutShell 处理器源码
│   ├── src/main/scala/        # Chisel 源码
│   ├── src/test/scala/        # 仿真顶层 (SimTop)
│   ├── difftest/              # DiffTest 子模块 (论文: 无，Ibex 用 Verilator 原生)
│   └── build/                 # 编译产物
│       ├── rtl/*.sv           # 生成的 Verilog (分文件)
│       ├── generated-src/     # Chisel 中间产物
│       ├── verilator-compile/ # Verilator 编译产物 + 覆盖率数据
│       │   └── emu            # 仿真器二进制
│       └── U6_wave.fst        # FST 波形文件
├── bluesfl/                   # BluesFL 故障定位工具
│   ├── src/                   # Rust 源码
│   │   └── bin/
│   │       ├── sv_analysis.rs       # 主入口 (论文核心)
│   │       └── test_analysis.rs     # test_info 自动生成 (仅 Ibex)
│   ├── prompts/               # LLM prompt 模板
│   │   ├── block_checker_2.md       # BlockChecker prompt
│   │   ├── block_reranker.md        # BlockReranker prompt
│   │   ├── mod_checker.md           # ModuleChecker prompt (论文关键创新)
│   │   └── mod_checker_system.md
│   ├── signal_name_map.json   # 信号名映射表 (91 模块, NutShell 专属)
│   ├── .env                   # API 配置
│   ├── exps/                  # 论文实验脚本
│   │   └── run_biosfl.sh            # 批量实验入口
│   ├── scripts/
│   │   ├── fl_run_all.py            # 批量调用 sv_analysis
│   │   ├── collect_loc_results.py   # 结果收集
│   │   ├── cal_metric.py            # 指标计算
│   │   └── gen_signal_map.py        # 信号映射表生成
│   └── target/debug/sv_analysis  # 编译好的二进制
├── nutshell-sbfl/             # NutShell SBFL 数据集
│   ├── case/                  # 测试用例汇编源码
│   ├── patch/                 # Bug patch 文件
│   │   └── U6_jalr_bit0_not_cleared.patch
│   ├── build/
│   │   └── riscv64-nemu-interpreter-so  # NEMU 参考模型
│   └── ready-to-run/          # 预编译二进制
```

## 3. Step 1: 应用 Bug Patch 并生成 RTL {#step-1}

> **论文对应**: 论文通过 `src/bin/mutator` 自动注入 bug 到 Ibex Verilog 中。
> NutShell 用 Chisel，需要在 Scala 源码层面应用 patch。

### 3.1 应用 patch

```bash
cd /home/yuan/NutShell

# 确保从干净状态开始
git checkout -- src/main/scala/nutcore/backend/fu/ALU.scala

# 应用 U6 bug patch
git apply /home/yuan/nutshell-sbfl/patch/U6_jalr_bit0_not_cleared.patch

# 验证 patch 已应用
git diff src/main/scala/nutcore/backend/fu/ALU.scala
```

U6 patch 的改动：将 `Cat(adderRes(63,1), false.B)` 改为 `adderRes`，即 JALR 目标地址不再清除 bit0。

### 3.2 生成 RTL

> **论文对应**: Ibex 直接使用 Verilog 源码，无需此步。

```bash
cd /home/yuan/NutShell

# 编译 Chisel → Verilog (分文件输出)
make sim-verilog NOOP_HOME=/home/yuan/NutShell FIRTOOL=$HOME/.local/bin/firtool
```

验证生成的 RTL 包含 bug：

```bash
# 在 ALU.sv 中，_target_T_2 应该使用 _adderRes_T_3[38:0]
# 而不是清除 bit0 的版本
grep "_target_T_2" build/rtl/ALU.sv
```

预期输出（包含 bug）：
```verilog
wire [38:0]  _target_T_2 =
    io_in_bits_func[3] ? _adderRes_T_3[38:0] : io_cfIn_pc + io_offset[38:0];
```

## 4. Step 2: 编译仿真器 (emu) 并启用 FST 波形 {#step-2}

> **论文对应**: `verilator --trace-fst --cc ibex_core.sv` 直接编译。
> NutShell 使用 DiffTest 框架，需要额外链接 NEMU 参考模型。

**关键**: 必须使用 `EMU_TRACE=fst` 编译，否则无法生成 FST 波形。

### 4.1 注意事项

- **必须直接调用 difftest 的 make**，NutShell 的顶层 Makefile 不传递 `EMU_TRACE` 和 `REF`
- `REF` 必须传 `.so` 文件的完整路径（不是目录），这样 difftest 使用 `LinkedProxy` 模式
- `make clean` 会删除整个 `build/` 目录，需要重新生成 RTL

### 4.2 编译命令

```bash
# 清理（如果需要）
rm -rf /home/yuan/NutShell/build

# Step 1: 重新生成 RTL (clean 后必须)
cd /home/yuan/NutShell
make sim-verilog NOOP_HOME=/home/yuan/NutShell FIRTOOL=$HOME/.local/bin/firtool

# Step 2: 编译 emu (直接通过 difftest)
make -C /home/yuan/NutShell/difftest emu \
  EMU_TRACE=fst \
  NOOP_HOME=/home/yuan/NutShell \
  REF=/home/yuan/nutshell-sbfl/build/riscv64-nemu-interpreter-so \
  FIRTOOL=$HOME/.local/bin/firtool \
  WITH_CHISELDB=0 WITH_CONSTANTIN=0 RTL_SUFFIX=sv
```

验证编译成功：

```bash
file /home/yuan/NutShell/build/verilator-compile/emu
# 应输出: ELF 64-bit LSB pie executable, x86-64
```

## 5. Step 3: 编译测试用例 {#step-3}

> **论文对应**: 论文使用 coremark 等 benchmark 的 ELF。
> NutShell 使用针对每个 bug 设计的汇编测试用例。

测试用例位于 `/home/yuan/nutshell-sbfl/case/`。需要用 RISC-V 交叉编译器编译。

```bash
# 创建简单的链接脚本 (NutShell 从 0x80000000 开始执行)
cat > /tmp/link.ld << 'EOF'
ENTRY(_start)
SECTIONS {
  . = 0x80000000;
  .text : { *(.text) }
  .data : { *(.data) }
  .rodata : { *(.rodata) }
  .bss : { *(.bss) }
}
EOF

# 编译 U6 测试用例
riscv64-unknown-elf-gcc -nostdlib -nostartfiles -T /tmp/link.ld \
  -o /tmp/U6_test.elf \
  /home/yuan/nutshell-sbfl/case/U6_jalr_bit0_not_cleared.S

# 转换为二进制 (emu 需要 .bin 格式)
riscv64-unknown-elf-objcopy -O binary /tmp/U6_test.elf /tmp/U6_test.bin
```

## 6. Step 4: 运行仿真生成 FST 波形 {#step-4}

> **论文对应**: `./Vibex_simple_system --meminit=ram,coremark.elf -t`
> 生成 `sim.fst` + `mismatch_log.txt` + `ibex_simple_system.log`。
> NutShell 的 DiffTest 直接输出到 stderr，无独立 mismatch_log 文件。

```bash
cd /home/yuan/NutShell

./build/emu \
  -i /tmp/U6_test.bin \
  --diff /home/yuan/nutshell-sbfl/build/riscv64-nemu-interpreter-so \
  --dump-wave \
  --wave-path /home/yuan/NutShell/build/U6_wave.fst \
  -C 2000
```

参数说明：
- `-i`: 测试用例二进制
- `--diff`: NEMU 参考模型 (.so)
- `--dump-wave`: 启用波形输出
- `--wave-path`: FST 输出路径
- `-C`: 最大仿真周期

预期输出：
```
Core 0: ABORT at pc = 0x80000012
Core-0 instrCnt = 4, cycleCnt = 1,536
```

ABORT 表示 DiffTest 检测到 DUT 与参考模型不一致（即 bug 触发）。

验证 FST 文件生成：
```bash
ls -lh /home/yuan/NutShell/build/U6_wave.fst
```

## 7. Step 5: 确定 start_time {#step-5}

> **论文对应**: `test_analysis` 工具自动解析 `mismatch_log.txt`，生成 `test_info.json`：
> ```bash
> test_analysis --info-file=mismatch_log.txt \
>   --inst-trace=ibex_simple_system.log \
>   --output-file=test_info.json
> ```
> 自动计算 `start_time = failure_time - 1 - time_step`。
>
> **NutShell 缺失**: NutShell 的 DiffTest 不输出 `mismatch_log.txt` 格式，
> 需要手动确定 start_time。

**这是最关键的一步。** start_time 决定了 BluesFL 从哪个时间点开始反向追踪。

### 7.1 论文方法 (test_analysis)

论文的 `test_analysis.rs` 从 `mismatch_log.txt` 中提取：
1. `failure_time` — co-simulation 不一致的时间点（格式: `FAILURE: Co-simulation mismatch at time <N>`）
2. `start_time = failure_time - 1 - time_step`
3. `time_bound = start_time - 2 * ibex_cycle`（回溯 2 个 CPU 周期）
4. `start_sig` — 根据不一致类型自动选择（PC/WE/WADDR/WDATA）
5. `test_info` — 自动生成描述文本

### 7.2 NutShell 适配：先粗跑一次获取时间

由于 NutShell 的 DiffTest 不输出标准 mismatch_log，我们用两步法：

```bash
# 准备空 rm_params 文件（NutShell 不需要自定义 Verilator patch）
echo '{}' > /home/yuan/NutShell/build/rm_params.tree.json

# 先跑一次，观察日志中的时间
export SV_ANALYSIS_HOME=/home/yuan/bluesfl
RUST_LOG=info /home/yuan/bluesfl/target/debug/sv_analysis \
  --bug-id=U6_backend --agent-type=open-ai --model=deepseek-chat \
  --project-path=/home/yuan/NutShell/build/rtl \
  --include-paths=/home/yuan/NutShell/build/generated-src \
  --rm-params-path=/home/yuan/NutShell/build/rm_params.tree.json \
  --wave-path=/home/yuan/NutShell/build/U6_wave.fst \
  --coverage-path=/home/yuan/NutShell/build/verilator-compile \
  --top-module=SimTop \
  --top-scope=TOP.SimTop.cpu.soc.nutcore \
  --start-scope=TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend \
  --start-sig=io_redirect_target --start-time=0 \
  --test-info="Backend redirect_target is wrong" \
  --time-bound=15 --time-step=2 \
  --output-path=./e2e_results/U6_probe 2>&1 | tee /tmp/bluesfl_probe.log

# 从日志中找 redirect_target 的时间
grep "redirect_target" /home/yuan/bluesfl/logs/sv-analysis_rCURRENT.log | head -5
```

日志中会显示类似：
```
DEBUG [sv_analysis::tracer] line: 82, name: io_redirect_target, time: Some(515)
```

这个 `515` 就是正确的 start_time。

### 7.3 Scope 格式说明

> **论文 (Ibex)**: `TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core`

NutShell 分文件 RTL 的 scope 比单文件多一层 `cpu.soc.nutcore`（因为 NutShellSim → NutShell → NutCore 层级）：

```
TOP.SimTop.cpu.soc.nutcore                          ← top_scope
TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend   ← start_scope
TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend.exu
TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend.exu.alu
```

## 8. Step 6: 运行 BluesFL {#step-6}

> **论文对应**: `fl_run_all.py` 批量调用，参数从 `test_info.json` 自动读取。
> NutShell 当前为手动调用。

### 8.1 API 配置

确认 `/home/yuan/bluesfl/.env` 中的 API 配置：

```env
API_KEY=your-deepseek-api-key
API_BASE=https://api.deepseek.com
```

**注意**: `--model` 参数建议使用 `deepseek-chat`（非推理模型），推理模型 (如 `deepseek-v4-flash`) 返回 `reasoning_content` 而非 `content`，BluesFL 可能无法正确解析。

论文默认使用 `gpt-4o`。

### 8.2 执行命令

```bash
cd /home/yuan/bluesfl
mkdir -p e2e_results/U6_final

export SV_ANALYSIS_HOME=/home/yuan/bluesfl
RUST_LOG=warn /home/yuan/bluesfl/target/debug/sv_analysis \
  --bug-id=U6_backend \
  --agent-type=open-ai \
  --model=deepseek-chat \
  --project-path=/home/yuan/NutShell/build/rtl \
  --include-paths=/home/yuan/NutShell/build/generated-src \
  --rm-params-path=/home/yuan/NutShell/build/rm_params.tree.json \
  --wave-path=/home/yuan/NutShell/build/U6_wave.fst \
  --coverage-path=/home/yuan/NutShell/build/verilator-compile \
  --top-module=SimTop \
  --top-scope=TOP.SimTop.cpu.soc.nutcore \
  --start-scope=TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend \
  --start-sig=io_redirect_target \
  --start-time=515 \
  --test-info="Backend redirect_target is wrong" \
  --time-bound=15 \
  --time-step=2 \
  --output-path=./e2e_results/U6_final
```

### 8.3 参数说明

| 参数 | 说明 | 论文 (Ibex) | NutShell |
|------|------|------------|----------|
| `--project-path` | Verilog RTL 目录 | `ibex/rtl` | `NutShell/build/rtl` |
| `--include-paths` | 依赖头文件目录 | `vendor/lowrisc_ip/...` | `NutShell/build/generated-src` |
| `--rm-params-path` | 参数覆盖率文件 | `sim-verilator/rm_params.tree.json` | 空占位 |
| `--wave-path` | FST 波形文件 | `sim-verilator/sim.fst` | `U6_wave.fst` |
| `--coverage-path` | Verilator 编译目录 | `sim-verilator/` | `verilator-compile/` |
| `--top-module` | Verilog 顶层模块 | `ibex_core` | `SimTop` |
| `--top-scope` | 顶层 FST scope | `TOP...u_ibex_core` | `TOP.SimTop.cpu.soc.nutcore` |
| `--start-scope` | 开始追踪的 scope | 自动 (test_analysis) | 手动 `...backend` |
| `--start-sig` | 开始追踪的信号 | 自动 (pc_id/rvfi_*) | 手动 `io_redirect_target` |
| `--start-time` | 开始时间 | 自动 (`failure_time - 1 - time_step`) | 手动 (粗跑获取) |
| `--time-bound` | 回溯时间范围 | 自动 (`start_time - 4`) | 手动 15 |
| `--time-step` | 时间步长 | 2 (固定) | 2 (固定) |
| `--agent-type` | LLM API 类型 | `open-ai` | `open-ai` |
| `--model` | LLM 模型 | `gpt-4o` | `deepseek-chat` |

## 9. 结果解读

> **论文对应**: `collect_loc_results.py` + `cal_metric` 计算指标。
> NutShell 当前直接查看 JSON 输出。

### 9.1 输出文件

运行完成后，`output-path` 目录下生成：

```
e2e_results/U6_final/
├── llm_loc_results_U6_backend.json  # 最终定位结果 (论文 choices)
├── suspicious_blocks.json           # 可疑 block 详情（含代码片段）
├── suspicious_modules.json          # 可疑模块列表
└── trace.json                       # 追踪路径记录
```

### 9.2 U6 测试结果

```json
{
  "bug_id": "U6_backend",
  "token_usage": {
    "input_tokens": 227332,
    "output_tokens": 20226,
    "total_tokens": 247558
  },
  "choices": [
    {
      "module_name": "ALU",
      "score": 1.0
    }
  ]
}
```

**Top-1 = ALU, score = 1.0**，与论文预期一致。

`suspicious_blocks.json` 包含 ALU 模块的完整代码片段，其中 `_target_T_2 = _adderRes_T_3[38:0]` 即为缺少 bit0 清除的 bug 行。

### 9.3 追踪路径

```
Backend (start_sig=io_redirect_target @ t=515)
├── WBU  → ModuleOutput(io_redirect_target) → ModuleChecker: not_dive
├── CSR  → ModuleOutput(io_redirect_target) → ModuleChecker: not_dive
├── MOU  → ModuleOutput(io_redirect_target) → ModuleChecker: not_dive
└── ALU  → ModuleOutput(io_redirect_target) → ModuleChecker: dive → BlockChecker: suspicious!
```

这个路径与论文 Figure 3 描述的追踪过程一致：
1. 从错误信号 `io_redirect_target` 反向追踪
2. 到达 Backend 的 `ModuleOutput`（子模块输出端口）
3. ModuleChecker 判断 WBU/CSR/MOU 的输入也错了（追上游，不 dive）
4. ModuleChecker 判断 ALU 的输入合理但输出错误（dive 进去）
5. BlockChecker 检查 ALU 代码 → suspicious

## 10. 常见问题排查

### Q1: `Error: Io(Custom { kind: NotFound, error: "path not found" })`

**原因**: `output-path` 目录不存在或无法创建。

**解决**: 先手动创建目录，或在 bluesfl 项目根目录下运行：
```bash
cd /home/yuan/bluesfl && mkdir -p e2e_results/your_output_dir
```

### Q2: emu 编译报 `undefined reference to VerilatedFst`

**原因**: `EMU_TRACE=1` 只启用 VCD (--trace)，不启用 FST。FST 需要 `EMU_TRACE=fst`。

**解决**: 必须使用 `EMU_TRACE=fst`，且必须直接调用 difftest 的 make：
```bash
make -C /home/yuan/NutShell/difftest emu EMU_TRACE=fst ...
```

### Q3: emu 编译报 `REF_PROXY` 相关 C++ 错误

**原因**: `REF` 参数传了目录路径而不是 `.so` 文件路径，或者通过 NutShell 顶层 Makefile 传递导致参数丢失。

**解决**: REF 必须传完整的 `.so` 文件路径，且直接调用 difftest：
```bash
REF=/home/yuan/nutshell-sbfl/build/riscv64-nemu-interpreter-so
```

### Q4: LLM 投票全部失败 (`No signal values found`)

**原因**: FST 中信号名与 BluesFL 解析的 Verilog 信号名不匹配。

**解决**:
1. 确认 `signal_name_map.json` 存在于 `$SV_ANALYSIS_HOME/` 下（Chisel 重命名问题，Ibex 不需要）
2. 确认 `start_time` 正确（先粗跑一次从日志中获取）

### Q5: LLM 全部判 `not suspicious`

**可能原因**:
1. `start_time` 不正确，追踪到了错误的信号变化时间
2. `model` 使用了推理模型（如 `deepseek-v4-flash`），返回格式不兼容
3. FST 波形太短（测试用例太简单），信号值不够丰富

**解决**: 使用 `deepseek-chat` 模型，并确认 start_time 正确。

### Q6: `make clean` 后 RTL 丢失

**原因**: `make clean` 删除整个 `build/` 目录。

**解决**: clean 后必须重新执行 Step 1 (生成 RTL) 和 Step 2 (编译 emu)。

## 11. 测试其他 Bug

nutshell-sbfl 数据集中还有其他 bug 可测试。流程相同：

1. 选择 patch: `ls /home/yuan/nutshell-sbfl/patch/`
2. 应用 patch → 生成 RTL → 编译 emu
3. 选择测试用例: `ls /home/yuan/nutshell-sbfl/case/`
4. 编译测试用例 → 生成 FST
5. 确定 start_time → 运行 BluesFL

**注意**: 每个 bug 的 `start_scope`、`start_sig`、`start_time` 需要根据具体 bug 调整。

| Bug | 描述 | 正确模块 | start_sig |
|-----|------|---------|-----------|
| M1 | DIV by zero | MDU | (需确定) |
| U1 | ADDIW signext | ALU | (需确定) |
| U6 | JALR bit0 not cleared | ALU | io_redirect_target |
| D1 | SUB/SRA decode | ALU/IDU | (需确定) |

## 12. 完整一键脚本参考

```bash
#!/bin/bash
set -e

NUTSHELL=/home/yuan/NutShell
BLUESFL=/home/yuan/bluesfl
SBFL=/home/yuan/nutshell-sbfl
BUG=U6

echo "=== Step 1: Apply patch ==="
cd $NUTSHELL
git checkout -- src/main/scala/nutcore/backend/fu/ALU.scala
git apply $SBFL/patch/${BUG}_jalr_bit0_not_cleared.patch

echo "=== Step 2: Generate RTL ==="
rm -rf $NUTSHELL/build
make sim-verilog NOOP_HOME=$NUTSHELL FIRTOOL=$HOME/.local/bin/firtool

echo "=== Step 3: Build emu with FST ==="
make -C $NUTSHELL/difftest emu \
  EMU_TRACE=fst \
  NOOP_HOME=$NUTSHELL \
  REF=$SBFL/build/riscv64-nemu-interpreter-so \
  FIRTOOL=$HOME/.local/bin/firtool \
  WITH_CHISELDB=0 WITH_CONSTANTIN=0 RTL_SUFFIX=sv

echo "=== Step 4: Compile test case ==="
riscv64-unknown-elf-gcc -nostdlib -nostartfiles -T /tmp/link.ld \
  -o /tmp/${BUG}_test.elf $SBFL/case/${BUG}_jalr_bit0_not_cleared.S
riscv64-unknown-elf-objcopy -O binary /tmp/${BUG}_test.elf /tmp/${BUG}_test.bin

echo "=== Step 5: Generate FST ==="
cd $NUTSHELL
./build/emu -i /tmp/${BUG}_test.bin \
  --diff $SBFL/build/riscv64-nemu-interpreter-so \
  --dump-wave --wave-path $NUTSHELL/build/${BUG}_wave.fst -C 2000

echo "=== Step 6: Run BluesFL ==="
echo '{}' > $NUTSHELL/build/rm_params.tree.json
cd $BLUESFL && mkdir -p e2e_results/${BUG}_run

export SV_ANALYSIS_HOME=$BLUESFL
RUST_LOG=warn $BLUESFL/target/debug/sv_analysis \
  --bug-id=${BUG}_backend --agent-type=open-ai --model=deepseek-chat \
  --project-path=$NUTSHELL/build/rtl \
  --include-paths=$NUTSHELL/build/generated-src \
  --rm-params-path=$NUTSHELL/build/rm_params.tree.json \
  --wave-path=$NUTSHELL/build/${BUG}_wave.fst \
  --coverage-path=$NUTSHELL/build/verilator-compile \
  --top-module=SimTop \
  --top-scope=TOP.SimTop.cpu.soc.nutcore \
  --start-scope=TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend \
  --start-sig=io_redirect_target --start-time=515 \
  --test-info="Backend redirect_target is wrong" \
  --time-bound=15 --time-step=2 \
  --output-path=./e2e_results/${BUG}_run

echo "=== Done ==="
cat e2e_results/${BUG}_run/llm_loc_results_${BUG}_backend.json
```
