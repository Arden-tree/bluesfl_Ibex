#!/usr/bin/env python3
"""
Ibex BluesFL batch runner — runs sv_analysis on all bugs in the dataset.

Paper testing flow (Section 4.1 + Section 2.1):
    1. Bug injected by mutator (119 bugs total)
    2. Co-simulation (Ibex RTL + Spike ISS) runs CoreMark
    3. Mismatch detected → test report (I, sig=rvfi_pc_wdata, t, E)
    4. Per-cycle coverage generated during simulation
    5. BluesFL localizes the bug

This script automates steps 2-5 for each bug in the dataset.

Dataset structure (produced by mutator):
    ibex_dataset/
    ├── 0/
    │   ├── 0/                    # ibex working dir (mutated RTL + build)
    │   │   ├── build/.../sim-verilator/
    │   │   └── ...
    │   ├── diff
    │   └── test_info.json        # may not exist; auto-generated if missing
    ├── 1/
    │   ...

Usage:
    python3 scripts/ibex_fl_run_all.py \
        --path ibex_dataset \
        --localizer target/debug/sv_analysis \
        --test-analysis target/debug/test_analysis \
        --env .env \
        --model deepseek-v4-pro \
        --vote-total 1 \
        --prefix llm
"""

import json
import logging
import os
import re
import shutil
import subprocess
import sys
from argparse import ArgumentParser
from datetime import datetime
from pathlib import Path

logger = logging.getLogger(__name__)


def setup_logging():
    logs_dir = Path("./logs")
    logs_dir.mkdir(exist_ok=True)
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    log_filename = logs_dir / f"ibex_fl_run_all_{timestamp}.log"
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s - %(levelname)s - %(message)s',
        handlers=[logging.FileHandler(log_filename), logging.StreamHandler()]
    )
    logger.info(f"Log file: {log_filename}")


def main(cfg):
    root_path = Path(cfg.path)

    if not root_path.exists():
        logger.error(f"Path {root_path} does not exist")
        return

    # Resolve localizer/test_analysis to absolute paths up front — subprocess
    # runs with cwd=cur_wkdir, so relative paths would break.
    cfg.localizer = str(Path(cfg.localizer).resolve())
    if cfg.test_analysis:
        cfg.test_analysis = str(Path(cfg.test_analysis).resolve())
    if cfg.env:
        cfg.env = str(Path(cfg.env).resolve())

    if not Path(cfg.localizer).exists():
        logger.error(f"Localizer executable {cfg.localizer} does not exist")
        return

    folders = sorted(
        [p for p in root_path.glob("*") if "tmp" not in p.name and p.is_dir()],
        key=lambda p: int(p.name) if p.name.isdigit() else 0
    )

    if cfg.start is not None and cfg.end is not None:
        folders = folders[cfg.start:cfg.end]
    elif cfg.start is not None and cfg.end is None:
        folders = folders[cfg.start:]
    elif cfg.start is None and cfg.end is not None:
        folders = folders[:cfg.end]

    if cfg.only:
        wanted = set(cfg.only)
        folders = [f for f in folders if f.name in wanted]

    logger.info(f"Found {len(folders)} folders to process")

    error_folders = []
    success_count = 0

    for root_path in folders:
        logger.info(f"Scanning folder: {root_path}")

        for cur_dir in root_path.rglob("*"):
            if not cur_dir.is_dir():
                continue
            if "tmp" in cur_dir.name:
                continue

            test_info_file = cur_dir.parent / "test_info.json"
            # Bug workdir has same name as parent (e.g., ibex_dataset/0/0/)
            if cur_dir.name == cur_dir.parent.name:
                cur_wkdir = cur_dir
                logger.info(f"Processing directory: {cur_wkdir}")

                exe_path = cur_wkdir / "build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator"
                if not exe_path.exists():
                    logger.warning(f"  Build dir not found: {exe_path}, skipping")
                    continue

                # Step 1: cosim #1 (无 coverage) — 检测 mismatch → mismatch_log.txt
                # 对齐论文: mutator 阶段的 cosim 只判 mismatch, 不收 coverage
                sim_status = "skipped"
                try:
                    if not cfg.no_sim:
                        sim_status = rerun_simulation_no_cov(exe_path, cfg.cosim_timeout)
                except Exception as e:
                    logger.error(f"Error in cosim #1 at {cur_wkdir}: {e}")
                    error_folders.append(cur_wkdir)
                    sim_status = "error"

                # 论文: 数据集只保留能触发 mismatch 的 bug。
                # 超时/无 mismatch 的 case 直接跳过, 不做后续步骤。
                if sim_status not in ("hit", "skipped"):
                    logger.warning(f"  cosim#1 status={sim_status}, skip rest")
                    error_folders.append(cur_wkdir)
                    continue

                # Step 2: Generate test_info.json if missing (or force-regen)
                # 对齐论文 gen_test_info.py: 从 mismatch_log 解析 (sig, t, time_bound)
                # 当 test_analysis 二进制更新过 (例如修复了 start_sig 选择),
                # 用 --regen-test-info 强制重新生成, 让 start_sig 跟着 test_analysis
                # 的新逻辑走, 而不是用旧的缓存值。
                need_regen = (cfg.regen_test_info
                              or not test_info_file.exists()) and cfg.test_analysis
                if need_regen:
                    try:
                        if test_info_file.exists() and cfg.regen_test_info:
                            old_sig = json.loads(test_info_file.read_text()).get(
                                "start_sig", "?")
                            backup = test_info_file.with_suffix(".json.bak")
                            shutil.copy2(test_info_file, backup)
                            logger.info(f"  regen: backup old test_info.json → {backup}")
                        else:
                            old_sig = None
                        generate_test_info(cur_wkdir, exe_path, cfg.test_analysis,
                                           cur_dir.parent, cfg.time_step)
                        if test_info_file.exists():
                            new_sig = json.loads(test_info_file.read_text()).get(
                                "start_sig", "?")
                            if old_sig is not None and new_sig != old_sig:
                                logger.info(f"  regen: start_sig {old_sig} → {new_sig}")
                    except Exception as e:
                        logger.error(f"Error generating test_info: {e}")
                        error_folders.append(cur_wkdir)
                        continue

                # Step 3: Read test_info.json
                try:
                    with open(test_info_file, 'r') as f:
                        test_data = json.load(f)
                except Exception as e:
                    logger.error(f"Error reading test info file {test_info_file}: {e}")
                    error_folders.append(cur_wkdir)
                    continue

                # Step 4: cosim #2 (带精准 coverage 窗口) — dump per-cycle coverage
                # 对齐论文 fl_run_all.py: cov_start=time_bound, cov_end=start_time
                if not cfg.no_sim:
                    try:
                        cov_status = rerun_simulation_with_cov(
                            exe_path, test_data, cfg.cosim_timeout)
                        if cov_status not in ("ok", "skipped"):
                            logger.warning(f"  cosim#2 status={cov_status}")
                    except Exception as e:
                        logger.error(f"Error in cosim #2 at {cur_wkdir}: {e}")
                        error_folders.append(cur_wkdir)
                        continue

                # Step 5: Run localizer
                try:
                    run_localizer(cfg, cur_wkdir, test_data, cfg.prefix)
                    success_count += 1
                except Exception as e:
                    logger.error(f"Error when executing localizer at {cur_wkdir}: {e}")
                    error_folders.append(cur_wkdir)

    logger.info(f"Done. Success: {success_count}, Errors: {len(error_folders)}")
    for path in error_folders:
        print(f"  ERROR: {path}")


def _decode_stream(b) -> str:
    if b is None:
        return ""
    if isinstance(b, bytes):
        return b.decode(errors="ignore")
    return b


def rerun_simulation_no_cov(exe_path: Path, timeout: int) -> str:
    """Cosim #1: 不带 coverage flag, 只检测 mismatch → 写 mismatch_log.txt.

    对齐论文 mutator 阶段的 cosim (ibex_boot.sh):
      - 无 --cov-start/--cov-end/--cov-dir
      - 让 cosim 自然跑, mismatch 时自动停
      - 捕获 stdout (含 mismatch 信息) 给 test_analysis 解析

    Returns:
        'hit'         — mismatch detected (bug triggered)
        'no_mismatch' — cosim finished but no mismatch in output
        'timeout'     — killed after <timeout>s without mismatch
    """
    mismatch_log = exe_path / "mismatch_log.txt"

    # 清旧 coverage 和 log
    for f in exe_path.glob("coverage*.dat"):
        os.remove(f)
    if mismatch_log.exists():
        os.remove(mismatch_log)

    cmd = [
        "./Vibex_simple_system",
        "--meminit=ram,../../../examples/sw/benchmarks/coremark/coremark.elf",
        "-t",
    ]

    logger.info(f"  [cosim #1] running without coverage (timeout={timeout}s)...")
    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True, cwd=exe_path, timeout=timeout,
        )
    except subprocess.TimeoutExpired as e:
        with open(mismatch_log, 'w') as f:
            f.write(_decode_stream(e.stdout))
            err = _decode_stream(e.stderr)
            if err:
                f.write("\nSTDERR:\n")
                f.write(err)
            f.write(f"\n\n[TIMEOUT] cosim #1 killed after {timeout}s without mismatch\n")
        logger.warning(f"  [cosim #1] TIMEOUT after {timeout}s (bug not triggered)")
        return "timeout"

    with open(mismatch_log, 'w') as f:
        f.write(result.stdout)
        if result.stderr:
            f.write("\nSTDERR:\n")
            f.write(result.stderr)

    if "mismatch" in result.stdout.lower():
        logger.info(f"  [cosim #1] mismatch detected, mismatch_log.txt saved")
        return "hit"
    logger.warning(f"  [cosim #1] no mismatch found in cosim output")
    return "no_mismatch"


def rerun_simulation_with_cov(exe_path: Path, test_data: dict, timeout: int) -> str:
    """Cosim #2: 带精准 coverage 窗口, dump per-cycle coverage.

    对齐论文 fl_run_all.py (master 分支) 的 rerun_simulation:
      cov_start = test_data['time_bound']    # BluesFL BFS 下界
      cov_end   = test_data['start_time']    # 失败时间 (cosim 在此自然停)
      cov_dir   = '.'                        # (master 漏了这行, 我们补上)

    修正 master 的两处 bug:
      - flag 名: --cov-start (不是 --cover-start)
      - 必须传 --cov-dir, 否则 verilator_sim_ctrl.cc 里 cov_dir=nullptr 不写文件

    Returns:
        'ok'          — coverage 文件已生成
        'no_coverage' — cosim 跑完但没生成 coverage 文件
        'timeout'     — 超时
    """
    cov_start = test_data.get('time_bound', 1)
    cov_end = test_data.get('start_time', 30)

    # 清旧 coverage (保留 mismatch_log.txt, test_analysis 已读过)
    for f in exe_path.glob("coverage*.dat"):
        os.remove(f)

    cmd = [
        "./Vibex_simple_system",
        "--meminit=ram,../../../examples/sw/benchmarks/coremark/coremark.elf",
        "-t",
        "--cov-start", str(cov_start),
        "--cov-end", str(cov_end),
        "--cov-dir", ".",
    ]

    logger.info(f"  [cosim #2] running with coverage window "
                f"[{cov_start}, {cov_end}] (timeout={timeout}s)...")
    try:
        subprocess.run(
            cmd, capture_output=True, text=True, cwd=exe_path, timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        logger.warning(f"  [cosim #2] TIMEOUT after {timeout}s")
        return "timeout"

    cov_count = len(list(exe_path.glob("coverage*.dat")))
    if cov_count > 0:
        logger.info(f"  [cosim #2] coverage: {cov_count} files generated")
        return "ok"
    logger.warning(f"  [cosim #2] no coverage files generated")
    return "no_coverage"


def generate_test_info(cur_wkdir: Path, exe_path: Path,
                       test_analysis_bin: str, output_dir: Path, time_step: int):
    """Auto-generate test_info.json from cosim mismatch log.

    Paper Section 2.1: test report (I, sig, t, E) auto-generated from co-simulation.
    test_analysis parses mismatch_log.txt + trace_core_00000000.log → test_info.json
    """
    mismatch_log = exe_path / "mismatch_log.txt"
    trace_log = exe_path / "trace_core_00000000.log"
    output_file = output_dir / "test_info.json"

    if not mismatch_log.exists():
        logger.error(f"  mismatch_log.txt not found at {mismatch_log}")
        return

    cmd = [
        test_analysis_bin,
        f"--info-file={mismatch_log}",
        f"--inst-trace={trace_log}",
        f"--output-file={output_file}",
        f"--time-step={time_step}",
    ]

    logger.info(f"  Generating test_info.json via test_analysis...")
    result = subprocess.run(cmd, capture_output=True, text=True, check=True)

    if output_file.exists():
        logger.info(f"  test_info.json generated: {output_file}")
    else:
        logger.error(f"  test_info.json generation failed")
        if result.stderr:
            logger.error(f"  {result.stderr.strip()}")


def run_localizer(cfg, cur_wkdir, test_data, prefix):
    """Run sv_analysis for a single bug."""
    bug_id = cur_wkdir.name

    # Find next available result directory
    res_save_folder = cur_wkdir.parent
    cur_max_cnt = 0
    pat = re.compile(rf"{prefix}_(\d+)")
    for d in res_save_folder.glob(f"{prefix}_*"):
        if not d.is_dir():
            continue
        match = pat.match(d.name)
        if match:
            cnt = int(match.group(1))
            cur_max_cnt = max(cur_max_cnt, cnt)

    res_save_folder = res_save_folder / f"{prefix}_{cur_max_cnt + 1}"
    os.mkdir(res_save_folder)

    sim_dir = cur_wkdir / "build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator"

    # rm_params.tree.json: 优先用 wkdir 内由 apply_and_build 后处理生成的版本
    # (Verilator 5.x 输出已 patch 成 BluesFL 兼容格式), 缺失时 fallback 到 golden
    rm_params = sim_dir / "rm_params.tree.json"
    if not rm_params.exists():
        # 尝试用 fix_rm_params_v5.py 现场生成
        fix_script = Path(cfg.sv_home) / "scripts/fix_rm_params_v5.py"
        src = sim_dir / "Vibex_simple_system_009_param.tree.json"
        if fix_script.exists() and src.exists():
            logger.info(f"  generating rm_params.tree.json via fix_rm_params_v5.py")
            subprocess.run(
                ["python3", str(fix_script),
                 "--input", str(src), "--output", str(rm_params)],
                check=True, capture_output=True, text=True,
            )
        else:
            # fallback 到 golden
            golden_rm_params = Path(cfg.golden_rm_params)
            if golden_rm_params.exists():
                logger.info(f"  copying rm_params.tree.json from {golden_rm_params}")
                shutil.copy2(golden_rm_params, rm_params)
            else:
                logger.error(f"  rm_params missing and neither fix script nor golden found")

    cmd = [
        cfg.localizer,
        f"--bug-id={bug_id}",
        f"--agent-type={cfg.agent_type}",
        f"--agent-mode=tool-call",
        f"--model={cfg.model}",
        f"--project-path={cur_wkdir}/rtl",
        f"--include-paths={cur_wkdir}/vendor/lowrisc_ip/ip/prim/rtl/,{cur_wkdir}/vendor/lowrisc_ip/dv/sv/dv_utils",
        f"--rm-params-path={rm_params}",
        f"--coverage-path={sim_dir}",
        f"--wave-path={sim_dir}/sim.fst",
        "--top-module=ibex_core",
        "--top-scope=TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core",
        f"--start-scope={test_data['start_scope']}",
        f"--start-sig={test_data['start_sig']}",
        f"--start-time={test_data['start_time']}",
        f"--time-bound={test_data['time_bound']}",
        "--time-step=2",
        f"--output-path={str(res_save_folder)}",
        f"--vote-top-k={cfg.vote_top_k}",
        f"--vote-total={cfg.vote_total}",
    ]

    if cfg.env:
        cmd.append(f"--dot-env={cfg.env}")

    cmd += ['--test-info', test_data['test_info']]

    # Save boot script (JSON form) for reproducibility — avoids shell quoting
    # bugs when test_info contains parens/backticks/newlines.
    import json as _json
    with open(cur_wkdir / "boot_sv_analysis.json", 'w') as f:
        _json.dump({"cwd": str(cur_wkdir), "cmd": cmd}, f, indent=2)

    logger.info(f"  Running sv_analysis for bug {bug_id}...")
    env = os.environ.copy()
    if cfg.sv_home:
        env["SV_ANALYSIS_HOME"] = cfg.sv_home
    result = subprocess.run(cmd, capture_output=True, text=True, check=True,
                            cwd=cur_wkdir, env=env)

    with open(cur_wkdir / 'sv_analysis_output.log', 'w') as f:
        f.write("STDOUT:\n")
        f.write(result.stdout)
        f.write("\nSTDERR:\n")
        f.write(result.stderr)

    logger.info(f"  Results saved to {res_save_folder}")


if __name__ == '__main__':
    setup_logging()
    parser = ArgumentParser(description="Ibex BluesFL batch runner")
    parser.add_argument("--path", "-p", help="root path of dataset", required=True)
    parser.add_argument("--env", "-e", default="", help="path to .env file")
    parser.add_argument("--localizer", "-l", help="path of sv_analysis", required=True)
    parser.add_argument("--test-analysis", default="", help="path of test_analysis binary")
    parser.add_argument("--model", "-m", default="deepseek-v4-pro", help="LLM model")
    parser.add_argument("--prefix", default="llm", help="result directory prefix")
    parser.add_argument("--start", default=None, help="start index", type=int)
    parser.add_argument("--end", default=None, help="end index", type=int)
    parser.add_argument("--only", nargs="+", default=None,
                        help="只处理指定 case 名 (如 --only dataset_0_0 dataset_0_1)")
    parser.add_argument("--no-sim", help="skip simulation rerun", action="store_true")
    parser.add_argument("--regen-test-info", action="store_true",
                        help="强制重新生成 test_info.json (覆盖旧的; 旧文件备份到 .json.bak). "
                             "当 test_analysis 修复了 start_sig 选择后用这个让所有 case 跟着走.")
    parser.add_argument("--vote-total", default=1, type=int, help="vote total number")
    parser.add_argument("--vote-top-k", default=1, type=int, help="pick top-k choices")
    parser.add_argument("--time-step", default=2, type=int, help="time step for test_analysis")
    parser.add_argument("--cosim-timeout", default=60, type=int,
                        help="cosim 超时秒数 (默认 60s, 对齐论文 ibex_boot.sh); 超时表示 bug 未触发, 跳过该 case")
    parser.add_argument("--agent-type", default="open-ai",
                        choices=["open-ai", "claude", "ollama"], help="agent type")
    parser.add_argument("--sv-home", default="/home/yuan/bluesfl",
                        help="SV_ANALYSIS_HOME (bluesfl project root)")
    parser.add_argument("--golden-rm-params", default="/home/yuan/ibex/build_golden/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/rm_params.tree.json",
                        help="golden rm_params.tree.json to copy into freshly built wkdir")

    args = parser.parse_args()
    main(args)
