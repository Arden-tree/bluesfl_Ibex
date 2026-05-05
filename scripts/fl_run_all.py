import json
import logging
import os
import re
import shutil
import subprocess
from argparse import ArgumentParser
from datetime import datetime
from pathlib import Path

from tqdm import tqdm


def setup_logging():
    """Setup logging configuration with timestamped log file"""
    # Create logs directory if it doesn't exist
    logs_dir = Path("./logs")
    logs_dir.mkdir(exist_ok=True)

    # Generate timestamp for log filename
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    log_filename = logs_dir / f"fl_run_all_{timestamp}.log"

    # Configure logging
    logging.basicConfig(
        level=logging.ERROR,
        format='%(asctime)s - %(levelname)s - %(message)s',
        handlers=[
            logging.FileHandler(log_filename),
            logging.StreamHandler()  # Also output to console
        ]
    )

    logger = logging.getLogger(__name__)
    logger.info(f"Logging initialized. Log file: {log_filename}")
    return logger


def main(cfg):
    root_path = Path(cfg.path)

    if not root_path.exists():
        logger.error(f"Path {root_path} does not exist")
        return

    if not Path(cfg.localizer).exists():
        logger.error(f"Localizer executable {cfg.localizer} does not exist")
        return

    folders = [p for p in root_path.glob("*") if "tmp" not in p.name]
    # folders = [root_path / "init"]

    if cfg.start is not None and cfg.end is not None:
        folders = folders[cfg.start:cfg.end]
    elif cfg.start is not None and cfg.end is None:
        folders = folders[cfg.start:]
    elif cfg.start is None and cfg.end is not None:
        folders = folders[:cfg.end]

    logger.info(f"Found {len(folders)} folders to process (excluding tmp folders)")

    total_valid_folders = 0
    processed_count = 0
    error_folders = []

    # unfinished
    ignore_folders = []

    for root_path in tqdm(folders):
        logger.info(f"Scanning folder: {root_path}")

        for cur_dir in root_path.rglob("*"):
            if not cur_dir.is_dir():
                continue

            # Skip "tmp" folder and its subdirectories
            if "tmp" in cur_dir.name:
                continue

            if cur_dir.name in ignore_folders:
                continue

            test_info_file = cur_dir.parent / "test_info.json"
            if test_info_file.exists() and cur_dir.name == cur_dir.parent.name:
                cur_wkdir = cur_dir
                total_valid_folders += 1

                logger.info(f"Processing directory: {cur_wkdir}")

                try:
                    with open(test_info_file, 'r') as f:
                        test_data = json.load(f)
                except Exception as e:
                    logger.error(f"Error reading test info file {test_info_file}: {e}")
                    error_folders.append(cur_wkdir)
                    continue

                # get custom cover time range: coverage.dat
                try:
                    if not cfg.no_sim:
                        rerun_simulation(cur_wkdir, test_data)
                except Exception as e:
                    logger.error(f"Unexpected error when rerun simulation at {cur_wkdir}: {e}")
                    error_folders.append(cur_wkdir)

                try:
                    run_localizer(cfg, cur_wkdir, test_data, cfg.prefix)
                except Exception as e:
                    logger.error(f"Unexpected error when executing localizer at {cur_wkdir}: {e}")
                    error_folders.append(cur_wkdir)

    # Summary
    logger.info(
        f"Processing complete. Total: {total_valid_folders}, Successfully processed: {processed_count}, Errors: {len(error_folders)}")

    for path in error_folders:
        print(path)


def rerun_simulation(cur_wkdir: Path, test_data):
    cover_start = test_data['time_bound']
    cover_end = test_data['start_time']
    cmd = [
        "./Vibex_simple_system",
        "--meminit=ram,../../../examples/sw/benchmarks/coremark/coremark.elf",
        "-t",
        "--cover-start",
        str(cover_start),
        "--cover-end",
        str(cover_end)
    ]

    exe_path = cur_wkdir / "build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator"
    for f in exe_path.glob("coverage*.dat"):
        os.remove(f)
    try:
        subprocess.run(cmd, capture_output=True, text=True, check=True,
                       cwd=exe_path)
    except subprocess.CalledProcessError:
        pass

    if len(list(exe_path.glob("coverage*.dat"))) > 0:
        logger.info(
            f"Successfully rerun in {cur_wkdir}, generate coverage: cover-start={cover_start}, cover-end={cover_end}")


def run_localizer(cfg, cur_wkdir, test_data, prefix):
    bug_id = cur_wkdir.name
    # res_0/, res_1/, res_2/
    # We will let llm to response for multi times at each bug, so we want to move these results to different folders.
    res_save_folder = cur_wkdir.parent
    cur_max_cnt = 0
    pat = re.compile(rf"{prefix}_(\d+)")
    for d in res_save_folder.glob(f"{prefix}_*"):
        if not d.is_dir():
            continue
        dir_name = d.name
        match = pat.match(dir_name)
        if match:
            cnt = int(match.group(1))
            cur_max_cnt = max(cur_max_cnt, cnt)

    res_save_folder = res_save_folder / f"{prefix}_{cur_max_cnt + 1}"
    assert not res_save_folder.exists()
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

    if len(cfg.env) > 0:
        cmd.append(f"--dot-env={cfg.env}")

    with open(cur_wkdir / "boot_sv_analysis.sh", 'w') as f:
        tmp_cmd = cmd + [f'--test-info "{test_data['test_info']}"', ]
        f.write(' '.join(tmp_cmd))

    cmd += [f'--test-info', f'{test_data['test_info']}', ]
    logger.info(f"Running command: {' '.join(cmd)}")
    result = subprocess.run(cmd, capture_output=True, text=True, check=True, cwd=cur_wkdir)

    # Save both stdout and stderr to file
    with open(cur_wkdir / 'sv_analysis_output.log', 'w') as f:
        f.write("STDOUT:\n")
        f.write(result.stdout)
        f.write("\nSTDERR:\n")
        f.write(result.stderr)


if __name__ == '__main__':
    parser = ArgumentParser()
    # We expect the dataset in structure like:
    # root
    # ├── 0
    # │   ├── 0: ibex wkdir
    # │   │   ├── mismatch_log.txt
    # │   │   └── ...
    # │   ├── diff
    # │   ...
    # │
    # ├── 1
    # │   ├── 1: ibex wkdir
    # │   │   ├── mismatch_log.txt
    # │   │   └── ...
    # │   ├── diff

    # --path=/tmp/mutate_result
    # --localizer=/home/lzz/RustProjects/sv-analysis/target/debug/sv_analysis
    # --agent-type=open-ai
    # --env=/home/lzz/RustProjects/sv-analysis/.env

    logger = setup_logging()
    parser.add_argument("--path", "-p", help="root path of dataset", required=True)
    parser.add_argument("--env", "-e", help="root path of dot env")
    parser.add_argument("--localizer", "-l", help="path of sv_analysis", required=True)
    parser.add_argument("--model", "-m", default="gpt-4o-mini", help="root path of dataset")
    parser.add_argument("--prefix", default="llm_res", help="exp name")
    parser.add_argument("--start", default=None, help="start index", type=int)
    parser.add_argument("--end", default=None, help="end index", type=int)
    parser.add_argument("--no-sim", help="end index", action="store_true")
    parser.add_argument("--vote-total", default=2, help="vote total number")
    parser.add_argument("--vote-top-k", default=1, help="pick top-k choices")
    parser.add_argument("--agent-type", default="open-ai", choices=["open-ai", "claude", "ollama"], help="agent type")

    args = parser.parse_args()
    main(args)
