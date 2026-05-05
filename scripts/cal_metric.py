import json
import os
import re
import subprocess
import logging
from argparse import ArgumentParser
from ftplib import print_line
from pathlib import Path
from datetime import datetime

# res_prefix = ""
res_prefix = "llm_"


def setup_logging():
    """Setup logging configuration with timestamped log file"""
    # Create logs directory if it doesn't exist
    logs_dir = Path("./logs")
    logs_dir.mkdir(exist_ok=True)

    # Generate timestamp for log filename
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    log_filename = logs_dir / f"script_cal_metric_{timestamp}.log"

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


def collect_results(cur_dir: Path, latest: bool = False):
    results = {}
    pat = re.compile(rf"{res_prefix}_(\d+)")

    for res_dir in cur_dir.glob(f"{res_prefix}*"):
        if not res_dir.is_dir():
            continue
        dir_name = res_dir.name
        match = pat.match(dir_name)
        if match:
            res_id = int(match.group(1))
            results[res_id] = get_result_in_res_folder(res_dir)
    if latest and len(results.keys()) != 0:
        max_key = max(results.keys())
        results = {max_key: results[max_key]}
    return results


def main(cfg):
    logger = setup_logging()

    root_path = Path(cfg.path)

    if not root_path.exists():
        logger.error(f"Path {root_path} does not exist")
        return

    folders = [p for p in root_path.glob("*") if "tmp" not in p.name]
    logger.info(f"Found {len(folders)} folders to process (excluding tmp folders)")

    # Recursively walk through all directories
    total_cases = 0
    block_hit_count = 0
    module_hit_count = 0
    trace_size = 0
    loc_mismatch = []
    loc_match = []

    top_k_count = {}

    hit_count = {}

    for cur_dir in folders:
        if not cur_dir.is_dir():
            continue

        # Skip "tmp" folder and its subdirectories
        if "tmp" in cur_dir.name:
            continue

        test_info = cur_dir / "test_info.json"

        if test_info.exists():
            oracle_file = cur_dir / "oracle_info.json"
            bid = cur_dir.name

            total_cases += 1
            results = collect_results(cur_dir, args.latest)

            oracle_data = read_json(oracle_file)
            block_not_hit = True
            for res_id, v in results.items():
                trace, suspicious_blocks, suspicious_modules = v
                if trace:
                    covered_bids = [item["bid"] for item in trace]
                    if oracle_data['bid'] in covered_bids:
                        block_hit_count += 1
                        block_not_hit = False
                        break
                else:
                    logger.warning(f"res_id={res_id} missing trace.json file at {cur_dir}")
            if block_not_hit:
                hit_count[bid] = 0
                # logger.info(f"block not hit: {cur_dir}")
            else:
                hit_count[bid] = 1
                # logger.info(f"block hit: {cur_dir}")

            for res_id, v in results.items():
                trace, suspicious_blocks, suspicious_modules = v
                if suspicious_modules:
                    chosen_modules = [item[1] for item in suspicious_modules]
                    if oracle_data['module_name'] in chosen_modules:
                        module_hit_count += 1
                        break
                else:
                    logger.warning(f"res_id={res_id} missing suspicious_modules.json file at {cur_dir}")

            matched = False
            for res_id, v in results.items():
                trace, suspicious_blocks, suspicious_modules = v
                if trace:
                    last = trace[-1]
                    if last["bid"] == oracle_data["bid"]:
                        matched = True
                        break

            if not matched:
                loc_mismatch.append(cur_dir)
            else:
                loc_match.append(cur_dir)

            pos_match = {}
            for i in range(10 + 1):
                pos_match[i] = 0
            avg_trace_size = 0
            for res_id, v in results.items():
                trace, suspicious_blocks, suspicious_modules = v
                if trace:
                    avg_trace_size += len(trace)
                    for i in range(len(trace)):
                        if i + 1 not in pos_match:
                            continue
                        pred = trace[len(trace) - i - 1]
                        if pred["bid"] == oracle_data["bid"]:
                            pos_match[i + 1] = 1
            if len(results) != 0:
                avg_trace_size /= len(results)

            trace_size += avg_trace_size

            if not block_not_hit and avg_trace_size <= 10:
                print(f"trace_len={avg_trace_size}, {cur_dir}")

            for i in range(1, 10 + 1):
                pos_match[i] += pos_match[i - 1]

            for top_id, v in pos_match.items():
                if top_id in top_k_count:
                    top_k_count[top_id] += v
                else:
                    top_k_count[top_id] = v

    # Summary
    logger.info(
        f"Processing complete. \n"
        f"Total cases={total_cases}, \n"
        f"Block hit cases={block_hit_count}, \n"
        f"Module hit cases={module_hit_count}, \n"
        f"Top-1 ={top_k_count[1]}, \n"
        f"Top-5 ={top_k_count[5]}, \n"
        f"Top-10 ={top_k_count[10]}, \n"
        f"Average Trace size ={trace_size / total_cases}, \n"
        f"Block hit rate={block_hit_count * 1. / total_cases * 100:}%, \n"
        f"Module hit rate={module_hit_count * 1. / total_cases * 100:}%, \n"
    )

    logger.info(f"Hit Count = \n{hit_count}")

    # print(f"Not accurate localized {len(loc_mismatch)} cases:")
    # for path in loc_mismatch:
    #     print(path)
    #
    # print(f"Accurate localized {len(loc_match)} cases:")
    # for path in loc_match:
    #     print(path)


if __name__ == '__main__':
    # -p /tmp/mutate_result
    parser = ArgumentParser()
    parser.add_argument("--path", "-p", help="root path of mutate result", required=True)
    parser.add_argument("--latest", action="store_true", help="root path of mutate result")
    parser.add_argument("--prefix", default="llm", help="Result file prefix, {prefix}_loc_results_{bug_id}.json")

    args = parser.parse_args()
    res_prefix = args.prefix
    print(f"Args: {args}")
    main(args)
