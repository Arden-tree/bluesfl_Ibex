# NutShell + BluesFL Bug 测试全流程

本文档记录如何在 NutShell 处理器上复现 BluesFL 论文的故障定位流程，以 U6 bug (JALR bit0 未清除) 为例。

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

---

## 1. 环境依赖

| 工具 | 用途 | 安装检查 |
|------|------|---------|
| mill | Chisel/Scala 编译 | `which mill` |
| firtool | CIRCT FIRRTL 编译器 | `which firtool` |
| java (17+) | mill 运行时 | `java -version` |
| riscv64-unknown-elf-gcc | 测试用例交叉编译 | `which riscv64-unknown-elf-gcc` |
| Verilator (5.048+) | C++ 仿真生成 | `verilator --version` |
| rustc + cargo | BluesFL 编译 | `rustc --version` |

## 2. 仓库结构

```
/home/yuan/
├── NutShell/                  # NutShell 处理器源码
│   ├── src/main/scala/        # Chisel 源码
│   ├── src/test/scala/        # 仿真顶层 (SimTop)
│   ├── difftest/              # DiffTest 子模块
│   └── build/                 # 编译产物
│       ├── rtl/*.sv           # 生成的 Verilog (分文件)
│       ├── generated-src/     # Chisel 中间产物
│       ├── verilator-compile/ # Verilator 编译产物
│       │   └── emu            # 仿真器二进制
│       └── U6_wave.fst        # 波形文件
├── bluesfl/                   # BluesFL 故障定位工具
│   ├── src/                   # Rust 源码
│   ├── prompts/               # LLM prompt 模板
│   ├── signal_name_map.json   # 信号名映射表 (91 模块)
│   ├── .env                   # API 配置
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

**这是最关键的一步。** start_time 决定了 BluesFL 从哪个时间点开始反向追踪。

### 7.1 论文方法

论文的做法是：
1. 从 DiffTest 的失败日志中提取 `failure_time`（PC 不一致的时刻）
2. 计算 `start_time = failure_time - 1 - time_step`

### 7.2 实际操作：先粗跑一次获取时间

由于 NutShell 的 DiffTest 不直接输出精确的 redirect 信号错误时间，我们可以：
1. 先用 `start_time=0` 或一个猜测值跑一次 BluesFL
2. 从日志中找到 `io_redirect_target` 实际出现的时间

```bash
# 准备空 rm_params 文件（NutShell 不需要）
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

NutShell 分文件 RTL 的 scope 比单文件多一层 `cpu.soc.nutcore`：

```
TOP.SimTop.cpu.soc.nutcore                          ← top_scope
TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend   ← start_scope
TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend.exu
TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend.exu.alu
```

## 8. Step 6: 运行 BluesFL {#step-6}

### 8.1 API 配置

确认 `/home/yuan/bluesfl/.env` 中的 API 配置：

```env
API_KEY=your-deepseek-api-key
API_BASE=https://api.deepseek.com
```

**注意**: `--model` 参数建议使用 `deepseek-chat`（非推理模型），推理模型 (如 `deepseek-v4-flash`) 返回 `reasoning_content` 而非 `content`，BluesFL 可能无法正确解析。

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

| 参数 | 说明 | NutShell 值 |
|------|------|------------|
| `--project-path` | Verilog RTL 文件目录 | `/home/yuan/NutShell/build/rtl` |
| `--include-paths` | Chisel 生成文件目录 | `/home/yuan/NutShell/build/generated-src` |
| `--rm-params-path` | 参数覆盖率文件 | `rm_params.tree.json` (可空) |
| `--wave-path` | FST 波形文件 | `U6_wave.fst` |
| `--coverage-path` | Verilator 编译目录 | `verilator-compile/` |
| `--top-module` | Verilog 顶层模块 | `SimTop` |
| `--top-scope` | 顶层 FST scope | `TOP.SimTop.cpu.soc.nutcore` |
| `--start-scope` | 开始追踪的 scope | `...backend` |
| `--start-sig` | 开始追踪的信号 | `io_redirect_target` |
| `--start-time` | 开始时间（FST 时间单位） | 515 |
| `--time-bound` | 回溯时间范围 | 15 |
| `--time-step` | 时间步长（2 = 1 clock cycle） | 2 |
| `--agent-type` | LLM API 类型 | `open-ai` |
| `--model` | LLM 模型 | `deepseek-chat` |

## 9. 结果解读

### 9.1 输出文件

运行完成后，`output-path` 目录下生成：

```
e2e_results/U6_final/
├── llm_loc_results_U6_backend.json  # 最终定位结果
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
├── WBU  → ModuleOutput(io_redirect_target) → LLM: not_dive
├── CSR  → ModuleOutput(io_redirect_target) → LLM: not_dive
├── MOU  → ModuleOutput(io_redirect_target) → LLM: not_dive
└── ALU  → ModuleOutput(io_redirect_target) → LLM: dive → suspicious!
```

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
1. 确认 `signal_name_map.json` 存在于 `$SV_ANALYSIS_HOME/` 下
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
