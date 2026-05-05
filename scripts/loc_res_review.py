import json
import logging
import re
from argparse import ArgumentParser
from datetime import datetime
from pathlib import Path


def setup_logging():
    """Setup logging configuration with timestamped log file"""
    # Create logs directory if it doesn't exist
    logs_dir = Path("./logs")
    logs_dir.mkdir(exist_ok=True)

    # Generate timestamp for log filename
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    log_filename = logs_dir / f"gen_test_info_{timestamp}.log"

    # Configure logging
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s - %(levelname)s - %(message)s',
        handlers=[
            logging.FileHandler(log_filename),
            logging.StreamHandler()  # Also output to console
        ]
    )

    logger = logging.getLogger(__name__)
    logger.info(f"Logging initialized. Log file: {log_filename}")
    return logger


def read_json(trace_file):
    with open(trace_file, "r") as fp:
        trace_data = json.load(fp)
    return trace_data


def get_result_in_res_folder(res_dir: Path):
    trace_data = None
    suspicious_blocks = None
    suspicious_modules = None
    for sub_file in res_dir.glob("*.json"):
        if sub_file.name == "trace.json":
            trace_data = read_json(sub_file)

        if sub_file.name == "suspicious_blocks.json":
            suspicious_blocks = read_json(sub_file)

        if sub_file.name == "suspicious_modules.json":
            suspicious_modules = read_json(sub_file)

    return trace_data, suspicious_blocks, suspicious_modules


def collect_results(cur_dir: Path):
    results = {}
    pat = re.compile(r"res_(\d+)")

    for res_dir in cur_dir.glob("res*"):
        if not res_dir.is_dir():
            continue
        dir_name = res_dir.name
        match = pat.match(dir_name)
        if match:
            res_id = int(match.group(1))
            results[res_id] = get_result_in_res_folder(res_dir)
    return results


def main(cfg):
    root_path = Path(cfg.path)

    if not root_path.exists():
        logger.error(f"Path {root_path} does not exist")
        return

    total_cases = 0

    for cur_dir in root_path.glob("*"):
        if not cur_dir.is_dir():
            continue

        # Skip "tmp" folder and its subdirectories
        if "tmp" in cur_dir.name:
            continue

        oracle_file = cur_dir / "oracle_info.json"
        total_cases += 1
        results = collect_results(cur_dir)

        oracle_data = read_json(oracle_file)
        for k, v in results.items():
            trace, suspicious_blocks, suspicious_modules = v
            print(f"{cur_dir}, trace_len={len(trace)} @ res = {k}")

    logger.info(
        f"Processing complete. Total: {total_cases}")


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

    logger = setup_logging()
    parser.add_argument("--path", "-p", help="root path of dataset", required=True)

    args = parser.parse_args()
    main(args)
