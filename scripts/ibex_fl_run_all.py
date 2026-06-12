#!/usr/bin/env python3
"""
Ibex BluesFL batch runner — runs sv_analysis on all bugs in the dataset.

Dataset structure (produced by mutator):
    ibex_dataset/
    ├── 0/
    │   ├── 0/                    # ibex working dir (mutated RTL + build)
    │   │   ├── mismatch_log.txt
    │   │   ├── build/.../sim-verilator/
    │   │   └── ...
    │   ├── diff
    │   ├── test_info.json
    │   └── oracle_info.json
    ├── 1/
    │   ...

Usage:
    python3 scripts/ibex_fl_run_all.py \
        --path ibex_dataset \
        --localizer target/debug/sv_analysis \
        --env .env \
        --model deepseek-v4-pro \
        --vote-total 1 \
        --prefix llm
"""

import json
import logging
import os
import re
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
            if test_info_file.exists() and cur_dir.name == cur_dir.parent.name:
                cur_wkdir = cur_dir
                logger.info(f"Processing directory: {cur_wkdir}")

                try:
                    with open(test_info_file, 'r') as f:
                        test_data = json.load(f)
                except Exception as e:
                    logger.error(f"Error reading test info file {test_info_file}: {e}")
                    error_folders.append(cur_wkdir)
                    continue

                # Rerun simulation with custom coverage range
                try:
                    if not cfg.no_sim:
                        rerun_simulation(cur_wkdir, test_data)
                except Exception as e:
                    logger.error(f"Error when rerun simulation at {cur_wkdir}: {e}")
                    error_folders.append(cur_wkdir)

                # Run localizer
                try:
                    run_localizer(cfg, cur_wkdir, test_data, cfg.prefix)
                    success_count += 1
                except Exception as e:
                    logger.error(f"Error when executing localizer at {cur_wkdir}: {e}")
                    error_folders.append(cur_wkdir)

    logger.info(f"Done. Success: {success_count}, Errors: {len(error_folders)}")
    for path in error_folders:
        print(f"  ERROR: {path}")


def rerun_simulation(cur_wkdir: Path, test_data):
    """Rerun simulation with custom coverage time range."""
    cover_start = test_data['time_bound']
    cover_end = test_data['start_time']
    cmd = [
        "./Vibex_simple_system",
        "--meminit=ram,../../../examples/sw/benchmarks/coremark/coremark.elf",
        "-t",
        "--cover-start", str(cover_start),
        "--cover-end", str(cover_end)
    ]

    exe_path = cur_wkdir / "build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator"
    if not exe_path.exists():
        logger.warning(f"  Build dir not found: {exe_path}, skipping rerun")
        return

    for f in exe_path.glob("coverage*.dat"):
        os.remove(f)
    try:
        subprocess.run(cmd, capture_output=True, text=True, check=True, cwd=exe_path)
    except subprocess.CalledProcessError:
        pass

    if len(list(exe_path.glob("coverage*.dat"))) > 0:
        logger.info(f"  Coverage generated: cover-start={cover_start}, cover-end={cover_end}")


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

    cmd = [
        cfg.localizer,
        f"--bug-id={bug_id}",
        f"--agent-type={cfg.agent_type}",
        f"--model={cfg.model}",
        f"--project-path={cur_wkdir}/rtl",
        f"--include-paths={cur_wkdir}/vendor/lowrisc_ip/ip/prim/rtl/,{cur_wkdir}/vendor/lowrisc_ip/dv/sv/dv_utils",
        f"--rm-params-path={cur_wkdir}/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/rm_params.tree.json",
        f"--coverage-path={cur_wkdir}/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator",
        f"--wave-path={cur_wkdir}/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/sim.fst",
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

    # Save boot script for reproducibility
    with open(cur_wkdir / "boot_sv_analysis.sh", 'w') as f:
        boot_cmd = ' '.join(cmd) + f' --test-info "{test_data["test_info"]}"'
        f.write(boot_cmd)

    cmd += ['--test-info', test_data['test_info']]

    logger.info(f"  Running sv_analysis for bug {bug_id}...")
    result = subprocess.run(cmd, capture_output=True, text=True, check=True, cwd=cur_wkdir)

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
    parser.add_argument("--model", "-m", default="deepseek-v4-pro", help="LLM model")
    parser.add_argument("--prefix", default="llm", help="result directory prefix")
    parser.add_argument("--start", default=None, help="start index", type=int)
    parser.add_argument("--end", default=None, help="end index", type=int)
    parser.add_argument("--no-sim", help="skip simulation rerun", action="store_true")
    parser.add_argument("--vote-total", default=1, type=int, help="vote total number")
    parser.add_argument("--vote-top-k", default=1, type=int, help="pick top-k choices")
    parser.add_argument("--agent-type", default="open-ai",
                        choices=["open-ai", "claude", "ollama"], help="agent type")

    args = parser.parse_args()
    main(args)
