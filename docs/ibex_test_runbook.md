# Ibex BluesFL 测试流程 (服务器部署版)

复现 BluesFL 论文 (DAC '26, arXiv 2605.17290) 在 Ibex 处理器上的 fault localization。

## 0. 依赖

### 系统包
```bash
sudo apt install -y \
    python3 python3-pip \
    git curl \
    libelf-dev libfl-dev libgoogle-perftools-dev \
    device-tree-compiler \
    autoconf automake autotools-dev \
    clang-format jq
```

### Python 包
```bash
pip3 install --user fusesoc 'edalize>=0.3' packaging pyelftools
```

### Rust (BluesFL)
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source $HOME/.cargo/env
```

### RISC-V 工具链 (编译 CoreMark + Spike 依赖)
```bash
# 用 apt 装的低版本即可 (gcc + newlib)
sudo apt install -y gcc-riscv64-unknown-elf
```

### Verilator 5.x (用源码装, 系统 5.048 即可)
```bash
# Ubuntu 22.04 系统仓库有 verilator, 版本可能略旧
sudo apt install -y verilator
verilator --version  # 需要 >= 5.0
```

---

## 1. 仓库准备

四个仓库 (假设都放在 `$HOME`), **必须 pin 到下面指定的 commit**:

```bash
cd $HOME

# (1) BluesFL 主仓 (Arden-tree fork, 不是 pointerliu 的)
git clone https://github.com/Arden-tree/bluesfl_Ibex.git bluesfl
cd bluesfl
git checkout feat/ibex-testing-framework
git reset --hard 192bf5bd9976792c640fab8e5ff2ee6aab9e60ea  # 2026-06-18
cd ..

# (2) Ibex cpufuzz fork (E1PsyCongroo/ibex dev-sbfl 分支, 数据集的源 RTL 基座)
git clone -b dev-sbfl https://github.com/E1PsyCongroo/ibex.git ibex-sbfl
cd ibex-sbfl
git reset --hard 6a5c738872e108920a95da5477654409833f2dd3   # 2026-06-18
cd ..

# (3) Spike ISS (lowRISC/riscv-isa-sim ibex_cosim 分支, 不是 ibex-spike-cosim 仓)
git clone -b ibex_cosim https://github.com/lowRISC/riscv-isa-sim.git ibex-spike-cosim
cd ibex-spike-cosim
git reset --hard aadf648d742de54f0a50eec01ceffd13ed12a1d1   # 2026-05-28
git submodule update --init --recursive
cd ..

# (4) 数据集 (含 mutated RTL + oracle_info.json, 共 12 个 dataset_X)
# 从你们内部数据源拷贝 (非 git 仓库), 目录结构:
#   dataset/
#   ├── dataset_0/{0,1,3,4,...,N}/
#   │   ├── *.sv           # mutated 源
#   │   ├── *.sv.diff      # 原始 diff
#   │   └── oracle_info.json   # {bid, module_name, scope_name}
#   ├── dataset_1/...
#   └── ...
```

---

## 2. 编译 Spike cosim

```bash
cd $HOME/ibex-spike-cosim
mkdir build && cd build
../configure --prefix=$HOME/ibex-spike-cosim/install --enable-commitlog
make -j$(nproc)
make install
# 关键产物 (fusesoc 编译 ibex 时 pkg-config 查找):
#   $HOME/ibex-spike-cosim/install/lib/pkgconfig/riscv-riscv.pc
#   $HOME/ibex-spike-cosim/install/lib/pkgconfig/riscv-disasm.pc
#   $HOME/ibex-spike-cosim/install/lib/pkgconfig/riscv-fdt.pc
#   $HOME/ibex-spike-cosim/install/lib/libriscv.so
```

---

## 3. 给 ibex-sbfl 打 patch

ibex-sbfl 的 `dev-sbfl` 分支已经包含下面 3 个 patch, 如果直接用 dev-sbfl 分支可以跳过这节。
如果从其他分支起, 必须打这 3 个 patch:

### Patch 1: per-cycle coverage (Verilator sim_ctrl)
文件: `vendor/lowrisc_ip/dv/verilator/simutil_verilator/cpp/verilator_sim_ctrl.cc`

加 3 块代码:
- 静态变量 `cov_start_time`, `cov_end_time`, `cov_dir`
- CLI option `--cov-start/--cov-end/--cov-dir` 解析
- Run() 循环里 posedge 后 dump + zero coverage

参考实现见 BluesFL `feat/ibex-testing-framework` 分支的 `scripts/ibex_fl_run_all.py` 注释。

### Patch 2: core file 加 `--coverage` + `--dump-tree-json` + `-Wno-fatal` + `-Wno-MODDUP`
文件: `dv/verilator/simple_system_cosim/ibex_simple_system_cosim.core`

sim target 的 `verilator_options` 里加:
```yaml
- '--coverage'
- '--dump-tree-json'   # 生成 rm_params.tree.json 源文件
- '-Wno-fatal'         # mutated RTL 可能有 UNUSEDSIGNAL/WIDTHEXPAND
- '-Wno-MODDUP'        # cpufuzz fork 同时包含 generic + xilinx prim 变体
- '-CFLAGS "... -DVM_COVERAGE=1 ..."'   # 启用 coverage dump C++ 宏
```

### Patch 3: edalize 兼容修复
文件: `util/check_tool_requirements.py`

```python
# 新版 edalize (>=0.3) 没有 .version 子模块, 改成 importlib.metadata fallback:
class EdalizeToolReq(ToolReq):
    def get_version(self):
        try:
            return importlib.import_module(self.tool + '.version').version
        except (ModuleNotFoundError, AttributeError):
            try:
                return importlib.metadata.version(self.tool)
            except Exception:
                raise RuntimeError(f'Unable to import {self.tool} to check version')
```

---

## 4. 编译 BluesFL 二进制

```bash
cd $HOME/bluesfl
cargo build --release --bin sv_analysis --bin test_analysis
# 产物:
#   target/release/sv_analysis
#   target/release/test_analysis
```

测试 rm_params 解析 (可选, 验证 BluesFL 能读 5.x 后处理格式):
```bash
cargo test --lib coverage::param::tests::test_ibex_sbfl_rm_params_v5_postprocessed -- --nocapture
```

---

## 5. 配置 .env (LLM API)

```bash
cat > $HOME/bluesfl/.env <<EOF
API_KEY=<your-deepseek-api-key>
API_BASE=https://api.deepseek.com
MODEL=deepseek-v4-pro
EOF
```

---

## 6. Phase A: apply_and_build.py (编译阶段)

对每个 mutated case: 拷贝 golden ibex-sbfl → 注入 mutated .sv → fusesoc 编译 → 后处理 rm_params

```bash
cd $HOME/bluesfl
python3 scripts/apply_and_build.py \
    --dataset-root $HOME/dataset/dataset \
    --dataset dataset_0 \
    --ibex-src $HOME/ibex-sbfl \
    --spike-prefix $HOME/ibex-spike-cosim/install \
    --output $HOME/dataset/built \
    --restore-ibex
```

**输出结构**:
```
$HOME/dataset/built/
├── dataset_0_0/
│   ├── build_status.json
│   └── dataset_0_0/                  # wkdir (Phase B 用这个)
│       ├── rtl/                      # mutated RTL
│       ├── examples/.../coremark.elf
│       └── build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/
│           ├── Vibex_simple_system   # cosim 二进制
│           └── rm_params.tree.json   # 自动后处理 (Verilator 5.x → 4.x 兼容)
├── dataset_0_1/...
```

**单 case 重试**: `--only dataset_0_5 dataset_0_7`

**编译耗时**: ~3-5 min/case × N case

**预期失败**:
- Verilator hard error (e.g. `wire x=x` circular) → 跳过 (case 9 这种)

---

## 7. Phase B: ibex_fl_run_all.py (cosim + 定位阶段)

对每个 built case:
1. cosim#1 (无 coverage, 60s timeout) — 检测 mismatch
2. test_analysis 生成 test_info.json (start_sig/start_time/time_bound)
3. cosim#2 (带 coverage 窗口) — dump per-cycle coverage dat
4. sv_analysis 运行 BluesFL 反向追踪 + LLM 排序

```bash
cd $HOME/bluesfl
python3 scripts/ibex_fl_run_all.py \
    --path $HOME/dataset/built \
    --localizer $HOME/bluesfl/target/release/sv_analysis \
    --test-analysis $HOME/bluesfl/target/release/test_analysis \
    --env $HOME/bluesfl/.env \
    --sv-home $HOME/bluesfl \
    --golden-rm-params $HOME/ibex-sbfl/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/rm_params.tree.json \
    --model deepseek-v4-pro \
    --vote-total 1 \
    --prefix llm_v5
```

**关键参数**:
- `--cosim-timeout 60` (默认): bug 未触发即跳过 (论文对齐)
- `--vote-total 1`: 单次 LLM (tool-call mode 自带 robustness)
- `--only dataset_0_3 dataset_0_8`: 单 case 重试

**输出结构**:
```
$HOME/dataset/built/dataset_0_3/llm_v5_1/
├── llm_loc_results_dataset_0_3.json   # 主结果 (Top-K module + score)
├── suspicious_blocks.json             # block 级
├── suspicious_modules.json            # module 级
└── trace.json                         # BFS 反向追踪路径
```

**单个 case 耗时**: 5-15 min (cosim 30s + LLM 数分钟)

---

## 8. 验证结果 vs oracle

```bash
python3 - <<'EOF'
import json
from pathlib import Path

DATASET = Path("/home/yuan/dataset/dataset")
BUILT = Path("/home/yuan/dataset/built")
PREFIX = "llm_v5_1"

cases = sorted([p.name for p in (DATASET/"dataset_0").iterdir() if p.is_dir() and p.name.isdigit()])
print(f"{'case':<10} {'oracle':<22} {'top1':<22} {'top2':<22} {'hit?':<6} {'tokens(K)':<10}")
print("-" * 100)

total, hit = 0, 0
for case_num in cases:
    case_id = f"dataset_0_{case_num}"
    oracle_p = DATASET/"dataset_0"/case_num/"oracle_info.json"
    res_p = BUILT/case_id/PREFIX/f"llm_loc_results_{case_id}.json"
    if not (oracle_p.exists() and res_p.exists()):
        continue
    oracle = json.load(open(oracle_p))["module_name"]
    res = json.load(open(res_p))
    choices = sorted(res["choices"], key=lambda c: -(c.get("score") or 0))
    top1 = choices[0]["module_name"] if choices else "-"
    top2 = choices[1]["module_name"] if len(choices) > 1 else "-"
    is_hit = (top1 == oracle)
    total += 1
    hit += int(is_hit)
    tok = res["token_usage"]["total_tokens"] // 1000
    print(f"{case_id:<10} {oracle:<22} {top1:<22} {top2:<22} {'✅' if is_hit else '❌':<6} {tok:<10}")

print(f"\nHit rate: {hit}/{total} = {hit/max(total,1)*100:.1f}%")
EOF
```

---

## 9. 常见问题

### Q: 某个 case cosim#1 超时 60s, 怎么判断是 deadlock 还是 bug 未触发?
```bash
# 手动跑那个 case, 用 ps 采样 CPU
cd <sim-verilator dir>
./Vibex_simple_system --meminit=ram,../../../examples/sw/benchmarks/coremark/coremark.elf -t &
PID=$!
for i in 1 2 3 4 5 6; do
  sleep 5
  ps -p $PID -o %cpu=,etime=,stat= 2>&1
done
kill -KILL $PID
```
- 85% CPU → 正常跑, 只是没 mismatch (silent bug, 论文范围外)
- ~0% CPU → 真 deadlock (stall_wb=1 之类)

### Q: 数据集里为什么有 cosmetic mutation (如 case 5)?
cpufuzz mutator 偶尔产 0 影响的 mutation。**论文方法论**: 只保留产生 mismatch 的 case, cosmetic 自动被过滤掉。

### Q: Spike cosim 进程残留?
`ps aux | grep Vibex_simple_system | grep -v grep` 看遗留, `kill -KILL` 清掉。

### Q: rm_params.tree.json 怎么来?
Phase A 在 fusesoc build 后自动调 `scripts/fix_rm_params_v5.py`:
- 读 Verilator 5.x 输出的 `_009_param.tree.json`
- 给缺 `dead` 字段的 MODULE 补 `dead: false`
- 写成 BluesFL master 期望的 4.x 兼容格式

### Q: BluesFL 代码改了什么?
本次 pipeline 改动 (clean diff, 可重放):
1. `src/bin/test_analysis.rs` 加 cpufuzz cosim 格式 parser: `"DUT didn't write to register xN, but a write was expected"` → `WeMismatch(true)`
2. `src/coverage/param.rs` 加单元测试 `test_ibex_sbfl_rm_params_v5_postprocessed` (只验证, 不改逻辑)
3. `scripts/apply_and_build.py` 集成 fix_rm_params_v5.py
4. `scripts/ibex_fl_run_all.py` 改 rm_params 优先用 wkdir 内本地生成, 缺失才 fallback 到 golden
5. 新脚本 `scripts/fix_rm_params_v5.py`

### Q: dataset_0 实测分布
9 case 中:
- 1 case Verilator hard error (case 9: circular wire) → build 跳过
- 6 case cosim#1 60s 无 mismatch (silent/cosmetic bug, 论文范围外)
- 2 case 有效 mismatch (case 3, 8) → 跑完 sv_analysis
- Top-1 命中: 1/2 (case 3 ✅, case 8 miss)
