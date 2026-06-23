#!/usr/bin/env python3
"""
Ibex bug 数据集 — 两阶段批量测试

阶段 1 (compile): 逐个 bug → 替换 .sv → 编译 → cosim → 保存 coverage + mismatch
阶段 2 (analyze): 逐个 bug → test_analysis → sv_analysis → 评测

用法:
    # 阶段 1：预编译（不需要 LLM API）
    python3 scripts/run_dataset.py compile \
        --dataset "/home/yuan/dataset(1)/dataset" \
        --ibex-dir /home/yuan/ibex \
        --output /home/yuan/bluesfl/dataset_results

    # 阶段 2：批量测试（需要 LLM API）
    python3 scripts/run_dataset.py analyze \
        --dataset "/home/yuan/dataset(1)/dataset" \
        --ibex-dir /home/yuan/ibex \
        --bluesfl-dir /home/yuan/bluesfl \
        --output /home/yuan/bluesfl/dataset_results \
        --model deepseek-v4-pro --env .env
"""

import json
import logging
import os
import shutil
import subprocess
import sys
from pathlib import Path

logger = logging.getLogger(__name__)

SIMDIR_NAME = "build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator"
BUILDSRC_NAME = "build/lowrisc_ibex_ibex_simple_system_cosim_0/src/lowrisc_ibex_ibex_core_0.1/rtl"
VC_FILE = "lowrisc_ibex_ibex_simple_system_cosim_0.vc"


def find_bugs(dataset_path):
    bugs = []
    for group in sorted(Path(dataset_path).iterdir()):
        if not group.is_dir() or not group.name.startswith("dataset_"):
            continue
        for bug_dir in sorted(group.iterdir()):
            if not bug_dir.is_dir():
                continue
            oracle = bug_dir / "oracle_info.json"
            sv_files = list(bug_dir.glob("*.sv"))
            if oracle.exists() and sv_files:
                bugs.append((group.name, bug_dir.name, bug_dir, sv_files[0],
                             json.loads(oracle.read_text())))
    return bugs


def run(cmd, timeout=600, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, **kw)


def run_nobuf(cmd, timeout=600, **kw):
    """Run without capture_output to avoid pipe deadlock. Returns (rc, stdout_file, stderr_file)."""
    kw.pop('capture_output', None)
    kw.pop('text', None)
    stdout_path = kw.pop('_stdout_file', None)
    stderr_path = kw.pop('_stderr_file', None)
    with open(stdout_path or os.devnull, 'w') as out, open(stderr_path or os.devnull, 'w') as err:
        r = subprocess.run(cmd, stdout=out, stderr=err, timeout=timeout, **kw)
    return r.returncode


def quick_build(ibex_dir, sv_file):
    """增量编译：Verilator + make"""
    simdir = ibex_dir / SIMDIR_NAME
    buildsrc = ibex_dir / BUILDSRC_NAME
    sv_name = sv_file.name

    shutil.copy2(sv_file, ibex_dir / "rtl" / sv_name)
    shutil.copy2(sv_file, buildsrc / sv_name)
    # touch 确保 Verilator 检测到变化
    os.utime(buildsrc / sv_name, None)

    env = os.environ.copy()
    pkg = str(Path.home() / "ibex-spike-cosim/install/lib/pkgconfig")
    env["PKG_CONFIG_PATH"] = f"{pkg}:{env.get('PKG_CONFIG_PATH', '')}"

    cflags = run(["pkg-config", "--cflags", "riscv-riscv", "riscv-disasm", "riscv-fdt"], env=env, timeout=10).stdout.strip()
    ldflags = run(["pkg-config", "--libs", "riscv-riscv", "riscv-disasm", "riscv-fdt"], env=env, timeout=10).stdout.strip()

    cmd = [
        "verilator", "--Mdir", ".", "--cc", "-f", VC_FILE,
        "--top-module", "ibex_simple_system",
        "--trace", "--trace-fst", "--trace-structs", "--trace-params", "--trace-max-array", "1024",
        "--coverage",
        "-CFLAGS", f"-std=c++17 -Wall -DVL_USER_STOP -DVM_TRACE_FMT_FST -DVM_COVERAGE=1 -DTOPLEVEL_NAME=ibex_simple_system -g {cflags}",
        "-LDFLAGS", f"-pthread -lutil -lelf {ldflags}",
        "-Wall", "-Wno-fatal",
        "--unroll-count", "72", "--build", "-j", str(os.cpu_count() or 4),
    ]
    log_file = simdir / "verilator_build.log"
    rc = run_nobuf(cmd, cwd=simdir, env=env, timeout=600,
                   _stdout_file=str(log_file), _stderr_file=str(log_file))
    return rc == 0


# ============================================================
# 阶段 1：预编译
# ============================================================
def phase_compile(bugs, cfg):
    ibex = Path(cfg.ibex_dir)
    simdir = ibex / SIMDIR_NAME
    buildsrc = ibex / BUILDSRC_NAME

    for i, (group, bug_id, bug_dir, sv_file, oracle) in enumerate(bugs):
        name = f"{group}/{bug_id}"
        out_dir = Path(cfg.output) / name.replace("/", "_")
        out_dir.mkdir(parents=True, exist_ok=True)
        done_file = out_dir / "compile_done"

        if done_file.exists():
            logger.info(f"[{i+1}/{len(bugs)}] {name} 已预编译，跳过")
            continue

        logger.info(f"[{i+1}/{len(bugs)}] {name} (oracle: {oracle['module_name']})")

        sv_name = sv_file.name
        rtl_target = ibex / "rtl" / sv_name
        buildsrc_target = buildsrc / sv_name
        rtl_backup = rtl_target.with_suffix(".sv.orig")
        buildsrc_backup = buildsrc_target.with_suffix(".sv.orig")

        if not rtl_backup.exists():
            shutil.copy2(rtl_target, rtl_backup)
        if buildsrc_target.exists() and not buildsrc_backup.exists():
            shutil.copy2(buildsrc_target, buildsrc_backup)

        try:
            # 编译
            logger.info("  编译...")
            if not quick_build(ibex, sv_file):
                logger.error("  编译失败")
                continue

            # 跑 cosim（超时则标记为不能触发 mismatch）
            logger.info("  cosim...")
            for f in simdir.glob("coverage*.dat"):
                os.remove(f)
            try:
                r = run(["./Vibex_simple_system",
                         "--meminit=ram,../../../examples/sw/benchmarks/coremark/coremark.elf",
                         "-t", "-c", "3000",
                         "--cov-start", "1", "--cov-end", "999999", "--cov-dir", "."],
                        cwd=simdir, timeout=600)
            except subprocess.TimeoutExpired:
                logger.warning("  cosim 超时（>10分钟），标记为不能触发 mismatch")
                (out_dir / "no_mismatch").touch()
                done_file.touch()
                continue
            (out_dir / "mismatch_log.txt").write_text(r.stdout + (r.stderr or ""))

            if "mismatch" not in r.stdout.lower():
                logger.warning("  无 mismatch（stall bug？）")
                (out_dir / "no_mismatch").touch()
                done_file.touch()
                continue

            # 复制输出文件（不复制 trace——可能 24GB，太大了）
            shutil.copy2(simdir / "mismatch_log.txt", out_dir / "mismatch_log.txt")
            shutil.copy2(simdir / "sim.fst", out_dir / "sim.fst")
            if not (out_dir / "rm_params.tree.json").exists():
                alt = ibex / "build/lowrisc_ibex_ibex_simple_system_0/sim-verilator/rm_params.tree.json"
                src = alt if alt.exists() else simdir / "rm_params.tree.json"
                if src.exists():
                    shutil.copy2(src, out_dir / "rm_params.tree.json")

            # 立即运行 test_analysis（trace 文件还在 simdir，可以读取）
            bluesfl_dir = Path(getattr(cfg, 'bluesfl_dir', '')) or Path.home() / "bluesfl"
            ta_bin = str(Path(bluesfl_dir) / "target/debug/test_analysis")
            if Path(ta_bin).exists():
                logger.info("  生成 test_info.json...")
                r = run([ta_bin,
                         f"--info-file={simdir}/mismatch_log.txt",
                         f"--inst-trace={simdir}/trace_core_00000000.log",
                         f"--output-file={out_dir}/test_info.json",
                         "--time-step=2"], timeout=120)
                if (out_dir / "test_info.json").exists():
                    logger.info("  test_info.json ✅")
                else:
                    logger.warning("  test_info.json 生成失败")

            # 复制 coverage（全部，阶段 2 会清理）
            cov_dir = out_dir / "coverage"
            cov_dir.mkdir(exist_ok=True)
            for f in simdir.glob("coverage_*_seq.dat"):
                shutil.copy2(f, cov_dir / f.name)

            logger.info(f"  ✅ 预编译完成 ({len(list(cov_dir.glob('*.dat')))} coverage files)")
            done_file.touch()

        except Exception as e:
            logger.error(f"  异常: {e}")
        finally:
            if rtl_backup.exists():
                shutil.copy2(rtl_backup, rtl_target)
            if buildsrc_backup.exists():
                shutil.copy2(buildsrc_backup, buildsrc_target)

    logger.info("阶段 1 完成")


# ============================================================
# 阶段 2：批量测试
# ============================================================
def phase_analyze(bugs, cfg):
    ibex = Path(cfg.ibex_dir)
    bluesfl = Path(cfg.bluesfl_dir)

    results = []
    for i, (group, bug_id, bug_dir, sv_file, oracle) in enumerate(bugs):
        name = f"{group}/{bug_id}"
        out_dir = Path(cfg.output) / name.replace("/", "_")

        if not (out_dir / "compile_done").exists():
            logger.info(f"[{i+1}/{len(bugs)}] {name} 跳过（未预编译）")
            results.append({"bug": name, "status": "not_compiled", "oracle": oracle["module_name"]})
            continue

        if (out_dir / "no_mismatch").exists():
            logger.info(f"[{i+1}/{len(bugs)}] {name} ❌ 未命中（不能触发 mismatch）")
            results.append({"bug": name, "status": "no_mismatch", "oracle": oracle["module_name"],
                          "detail": "不能触发 mismatch，BluesFL 无法检测"})
            continue

        logger.info(f"[{i+1}/{len(bugs)}] {name} (oracle: {oracle['module_name']})")

        try:
            # 使用 phase 1 生成的 test_info.json（不再重新生成）
            if not (out_dir / "test_info.json").exists():
                logger.error("  test_info.json 缺失（phase 1 未生成）")
                results.append({"bug": name, "status": "test_info_missing", "oracle": oracle["module_name"]})
                continue

            test_info = json.loads((out_dir / "test_info.json").read_text())

            # 清理 coverage（只保留需要的）
            tb = test_info['time_bound']
            st = test_info['start_time']
            cov_dir = out_dir / "coverage"
            kept = 0
            for f in cov_dir.glob("coverage_*_seq.dat"):
                try:
                    t = int(f.stem.split("_")[1])
                    if tb - 2 <= t <= st + 2:
                        kept += 1
                    else:
                        os.remove(f)
                except (ValueError, IndexError):
                    pass
            logger.info(f"  Coverage: {kept} files (time {tb-2}~{st+2})")

            # sv_analysis — 不用 capture_output，重定向到文件避免死锁
            llm_dir = out_dir / "llm_rvfi"
            llm_dir.mkdir(exist_ok=True)
            env = os.environ.copy()
            env["SV_ANALYSIS_HOME"] = str(bluesfl)
            cmd = [
                str(bluesfl / "target/debug/sv_analysis"),
                f"--bug-id={bug_id}", "--agent-type=open-ai", "--agent-mode=tool-call",
                f"--model={cfg.model}",
                f"--project-path={ibex}/rtl",
                f"--include-paths={ibex}/vendor/lowrisc_ip/ip/prim/rtl/,{ibex}/vendor/lowrisc_ip/dv/sv/dv_utils",
                f"--rm-params-path={out_dir}/rm_params.tree.json",
                f"--coverage-path={cov_dir}",
                f"--wave-path={out_dir}/sim.fst",
                "--top-module=ibex_core",
                "--top-scope=TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core",
                f"--start-scope={test_info['start_scope']}",
                f"--start-sig={test_info['start_sig']}",
                f"--start-time={test_info['start_time']}",
                f"--time-bound={test_info['time_bound']}", "--time-step=2",
                f"--output-path={llm_dir}", "--vote-top-k=1", "--vote-total=1",
            ]
            if cfg.env:
                env_path = Path(cfg.env)
                if not env_path.is_absolute():
                    env_path = Path(cfg.bluesfl_dir) / cfg.env
                cmd.append(f"--dot-env={env_path}")
            cmd += ["--test-info", test_info.get("test_info", "")]

            logger.info("  sv_analysis...")
            log_path = str(llm_dir / "sv_analysis.log")
            run_nobuf(cmd, cwd=ibex, env=env, timeout=1200,
                      _stdout_file=log_path, _stderr_file=log_path)

            # 评测
            loc_file = llm_dir / f"llm_loc_results_{bug_id}.json"
            if not loc_file.exists():
                logger.error("  无定位结果")
                results.append({"bug": name, "status": "no_result", "oracle": oracle["module_name"]})
                continue

            result = json.loads(loc_file.read_text())
            choices = result.get("choices", [])
            oracle_mod = oracle["module_name"]

            for j, c in enumerate(choices):
                if c.get("module_name") == oracle_mod:
                    logger.info(f"  ✅ Top-{j+1} 命中 {oracle_mod}")
                    results.append({"bug": name, "status": "hit", "rank": j + 1, "oracle": oracle_mod})
                    break
            else:
                top1 = choices[0].get("module_name", "?") if choices else "?"
                logger.info(f"  ❌ Top-1={top1}, oracle={oracle_mod}")
                results.append({"bug": name, "status": "miss", "top1": top1, "oracle": oracle_mod})

        except Exception as e:
            logger.error(f"  异常: {e}")
            results.append({"bug": name, "status": "error", "error": str(e), "oracle": oracle.get("module_name", "?")})

    # 汇总
    total = len(results)
    hits = [r for r in results if r["status"] == "hit"]
    misses = [r for r in results if r["status"] == "miss"]
    no_mismatch = [r for r in results if r["status"] == "no_mismatch"]
    errors = [r for r in results if r["status"] not in ("hit", "miss", "no_mismatch")]
    tested = len(hits) + len(misses)  # 能触发 mismatch 的 bug 数

    top1 = sum(1 for r in hits if r.get("rank", 99) == 1)
    top5 = len(hits)

    logger.info(f"\n{'='*60}")
    logger.info(f"总计: {total} bugs")
    logger.info(f"  能触发 mismatch: {tested} (Top-1: {top1}, Top-5: {top5})")
    logger.info(f"  不能触发 mismatch: {len(no_mismatch)} (BluesFL 无法检测)")
    logger.info(f"  其他错误: {len(errors)}")
    if tested > 0:
        logger.info(f"  Top-1 命中率: {top1}/{tested} = {top1/tested*100:.1f}%")
        logger.info(f"  Top-5 命中率: {top5}/{tested} = {top5/tested*100:.1f}%")
    logger.info(f"{'='*60}")
    logger.info(f"明细:")
    for r in results:
        if r["status"] == "hit":
            logger.info(f"  ✅ {r['bug']}: oracle={r['oracle']}, Top-{r.get('rank',1)}")
        elif r["status"] == "miss":
            logger.info(f"  ❌ {r['bug']}: oracle={r['oracle']}, Top-1={r.get('top1','?')}")
        elif r["status"] == "no_mismatch":
            logger.info(f"  ⚠️ {r['bug']}: oracle={r['oracle']}, 不能触发 mismatch")
        else:
            logger.info(f"  💥 {r['bug']}: oracle={r.get('oracle','?')}, {r['status']}")

    summary = {
        "total": total,
        "tested": tested,
        "top1": top1, "top5": top5,
        "no_mismatch": len(no_mismatch),
        "errors": len(errors),
        "top1_rate": f"{top1}/{tested}" if tested > 0 else "N/A",
        "top5_rate": f"{top5}/{tested}" if tested > 0 else "N/A",
        "results": results,
    }
    (Path(cfg.output) / "summary.json").write_text(json.dumps(summary, indent=2, ensure_ascii=False))


# ============================================================
# main
# ============================================================
def main():
    from argparse import ArgumentParser
    p = ArgumentParser(description="Ibex bug 数据集两阶段测试")
    p.add_argument("phase", choices=["compile", "analyze"], help="compile=预编译, analyze=批量测试")
    p.add_argument("--dataset", required=True)
    p.add_argument("--ibex-dir", required=True)
    p.add_argument("--bluesfl-dir", default="")
    p.add_argument("--output", required=True)
    p.add_argument("--model", default="deepseek-v4-pro")
    p.add_argument("--env", default="")
    p.add_argument("--start", type=int, default=None)
    p.add_argument("--end", type=int, default=None)
    cfg = p.parse_args()

    Path(cfg.output).mkdir(parents=True, exist_ok=True)
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s",
                        handlers=[logging.StreamHandler(), logging.FileHandler(Path(cfg.output) / "run.log")])

    bugs = find_bugs(cfg.dataset)
    if cfg.start is not None or cfg.end is not None:
        bugs = bugs[cfg.start:cfg.end]
    logger.info(f"共 {len(bugs)} 个 bug")

    if cfg.phase == "compile":
        phase_compile(bugs, cfg)
    else:
        if not cfg.bluesfl_dir:
            logger.error("analyze 需要 --bluesfl-dir")
            sys.exit(1)
        phase_analyze(bugs, cfg)


if __name__ == "__main__":
    main()
