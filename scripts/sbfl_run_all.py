import json
import logging
import os
import re
import shutil
import subprocess
from argparse import ArgumentParser
from concurrent.futures import ProcessPoolExecutor, as_completed
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
    log_filename = logs_dir / f"sbfl_run_all_{timestamp}.log"

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


def process_root_folder(root_path, cfg):
    """Process a single root folder - this will run in a separate process"""
    # Setup logging for this process
    process_logger = logging.getLogger(f"process_{os.getpid()}")

    processed_count = 0
    error_folders = []
    valid_folders = 0

    # unfinished
    ignore_folders = []

    process_logger.info(f"Processing root folder: {root_path}")

    for cur_dir in root_path.rglob("*"):
        if not cur_dir.is_dir():
            continue

        # Skip "tmp" folder and its subdirectories
        if "tmp" in cur_dir.name:
            continue

        if cur_dir.name in ignore_folders:
            continue

        test_info_file = cur_dir.parent / "test_info.json"
        # parent: 75, proj_root: 75/75
        if test_info_file.exists() and cur_dir.name == cur_dir.parent.name:
            cur_wkdir = cur_dir
            valid_folders += 1

            process_logger.info(f"Processing directory: {cur_wkdir}")

            try:
                with open(test_info_file, 'r') as f:
                    test_data = json.load(f)
            except Exception as e:
                process_logger.error(f"Error reading test info file {test_info_file}: {e}")
                error_folders.append(cur_wkdir)
                continue

            # get custom cover time range: coverage.dat
            try:
                rerun_simulation(cur_wkdir, test_data, process_logger)
            except Exception as e:
                process_logger.error(f"Unexpected error when rerun simulation at {cur_wkdir}: {e}")
                error_folders.append(cur_wkdir)

            try:
                run_localizer(cfg, cur_wkdir, test_data, process_logger)
                processed_count += 1
            except Exception as e:
                process_logger.error(f"Unexpected error when executing localizer at {cur_wkdir}: {e}")
                error_folders.append(cur_wkdir)

    return {
        'root_path': root_path,
        'valid_folders': valid_folders,
        'processed_count': processed_count,
        'error_folders': error_folders
    }


def main(cfg):
    root_path = Path(cfg.path)

    if not root_path.exists():
        logger.error(f"Path {root_path} does not exist")
        return

    if not Path(cfg.localizer).exists():
        logger.error(f"Localizer executable {cfg.localizer} does not exist")
        return

    folders = [p for p in root_path.glob("*") if "tmp" not in p.name]
    logger.info(f"Found {len(folders)} folders to process (excluding tmp folders)")

    total_valid_folders = 0
    total_processed_count = 0
    all_error_folders = []

    # Process folders concurrently
    if cfg.threads == 1:
        # Single-threaded processing (original behavior)
        for root_path in tqdm(folders, desc="Processing folders"):
            result = process_root_folder(root_path, cfg)
            total_valid_folders += result['valid_folders']
            total_processed_count += result['processed_count']
            all_error_folders.extend(result['error_folders'])
    else:
        # Multi-threaded processing
        logger.info(f"Using {cfg.threads} threads for processing")

        with ProcessPoolExecutor(max_workers=cfg.threads) as executor:
            # Submit all tasks
            future_to_folder = {
                executor.submit(process_root_folder, root_path, cfg): root_path
                for root_path in folders
            }

            # Process completed tasks with progress bar
            with tqdm(total=len(folders), desc="Processing folders") as pbar:
                for future in as_completed(future_to_folder):
                    folder = future_to_folder[future]
                    try:
                        result = future.result()
                        total_valid_folders += result['valid_folders']
                        total_processed_count += result['processed_count']
                        all_error_folders.extend(result['error_folders'])
                        logger.info(
                            f"Completed processing {folder}: {result['processed_count']}/{result['valid_folders']} successful")
                    except Exception as e:
                        logger.error(f"Error processing folder {folder}: {e}")
                        all_error_folders.append(folder)
                    finally:
                        pbar.update(1)

    # Summary
    logger.info(
        f"Processing complete. Total: {total_valid_folders}, Successfully processed: {total_processed_count}, Errors: {len(all_error_folders)}")

    for path in all_error_folders:
        print(path)


def rerun_simulation(cur_wkdir: Path, test_data, logger):
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


def run_localizer(cfg, cur_wkdir, test_data, logger):
    # sbfl_res_0/, sbfl_res_1/, sbfl_res_2/
    # We will let llm to response for multi times at each bug, so we want to move these results to different folders.
    res_save_folder = cur_wkdir.parent
    cur_max_cnt = 0
    pat = re.compile(rf"{cfg.prefix}_(\d+)")
    for d in res_save_folder.glob(f"{cfg.prefix}_*"):
        if not d.is_dir():
            continue
        dir_name = d.name
        match = pat.match(dir_name)
        if match:
            if cfg.clean:
                shutil.rmtree(d)
                logger.info(f"Clean before result folders: {d}")
            else:
                cnt = int(match.group(1))
                cur_max_cnt = max(cur_max_cnt, cnt)

    res_save_folder = res_save_folder / f"{cfg.prefix}_{cur_max_cnt + 1}"
    assert not res_save_folder.exists()
    os.mkdir(res_save_folder)

    bug_id = str(cur_wkdir.parent.name)
    metric = cfg.metric
    top_k = cfg.top_k

    cmd = [
        cfg.localizer,
        f"--project-path={cur_wkdir}/rtl",
        f"--include-paths={cur_wkdir}/vendor/lowrisc_ip/ip/prim/rtl/,{cur_wkdir}/vendor/lowrisc_ip/dv/sv/dv_utils",
        f"--rm-params-path={cur_wkdir}/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/rm_params.tree.json",
        f"--coverage-path={cur_wkdir}/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator",
        f"--wave-path={cur_wkdir}/build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/sim.fst",
        "--top-module=ibex_core",
        "--top-scope=TOP.ibex_simple_system.u_top.u_ibex_top.u_ibex_core",
        f"--bug-id={bug_id}",
        f"--interval={2}",
        f"--time-step={2}",
        f"--failed-time={test_data['start_time']}",
        f"--metric={metric}",
        f"--top-k={top_k}",
        f"--output-path={str(res_save_folder)}"
    ]

    with open(cur_wkdir / "boot_sbfl.sh", 'w') as f:
        tmp_cmd = cmd
        f.write(' '.join(tmp_cmd))

    logger.info(f"Running command: {' '.join(cmd)}")
    result = subprocess.run(cmd, capture_output=True, text=True, check=True, cwd=cur_wkdir)

    # Save both stdout and stderr to file
    with open(cur_wkdir / 'sbfl_output.log', 'w') as f:
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

    # --path=/home/lzz/dac26/hdl_fl_data/mutate_result
    # --localizer=/home/lzz/RustProjects/sv-analysis/target/debug/sbfl

    logger = setup_logging()
    parser.add_argument("--path", "-p", help="root path of dataset", required=True)
    parser.add_argument("--prefix", default="sbfl_res", help="Result folder name")
    parser.add_argument("--clean", "-c", action="store_true", help="clean before result folders", default=False)
    parser.add_argument("--localizer", "-l", help="path of sbfl", required=True)
    parser.add_argument("--metric", "-m", help="metric of spectrum", default="tarantula")
    parser.add_argument("--top-k", "-k", help="top K", default=10)
    parser.add_argument("--threads", "-t", type=int, help="number of threads to use", default=1)

    args = parser.parse_args()
    main(args)
