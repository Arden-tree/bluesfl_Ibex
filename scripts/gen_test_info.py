import os
import subprocess
import logging
from argparse import ArgumentParser
from pathlib import Path
from datetime import datetime


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


def main(cfg):
    logger = setup_logging()

    root_path = Path(cfg.path)

    if not root_path.exists():
        logger.error(f"Path {root_path} does not exist")
        return

    if not Path(cfg.generator).exists():
        logger.error(f"Generator executable {cfg.generator} does not exist")
        return

    folders = [p for p in root_path.glob("*") if "tmp" not in p.name]
    logger.info(f"Found {len(folders)} folders to process (excluding tmp folders)")

    # Recursively walk through all directories
    processed_count = 0
    error_folders = []

    for root_path in folders:
        logger.info(f"Scanning folder: {root_path}")

        for cur_dir in root_path.rglob("*"):
            if not cur_dir.is_dir():
                continue

            # Skip "tmp" folder and its subdirectories
            if "tmp" in cur_dir.name:
                continue

            # Check if both required files exist
            mismatch_log = cur_dir / "mismatch_log.txt"
            cosim_log = cur_dir / "build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/simple_system_cosim.log"

            if mismatch_log.exists() and cosim_log.exists():
                cur_wkdir = cur_dir
                logger.info(f"Processing directory: {cur_wkdir}")

                # Prepare arguments for the executable
                info_file_arg = f"--info-file={mismatch_log}"
                inst_trace_arg = f"--inst-trace={cosim_log}"
                output_file_arg = f"--output-file={cur_wkdir.parent}/test_info.json"

                # Run the executable
                try:
                    cmd = [
                        cfg.generator,
                        info_file_arg,
                        inst_trace_arg,
                        output_file_arg
                    ]

                    logger.info(f"Running command: {' '.join(cmd)}")
                    result = subprocess.run(cmd, capture_output=True, text=True, check=True, cwd=cur_wkdir)

                    logger.info(f"Successfully processed {cur_wkdir}")
                    if result.stdout:
                        logger.info(f"Command stdout: {result.stdout.strip()}")

                    if result.stderr:
                        logger.info(f"Command stderr: {result.stderr.strip()}")

                    processed_count += 1

                except subprocess.CalledProcessError as e:
                    logger.error(f"Error running generator for {cur_wkdir}: {e}")
                    if e.stderr:
                        logger.error(f"Error output: {e.stderr.strip()}")
                    error_folders.append(cur_wkdir)
                except FileNotFoundError:
                    logger.error(f"Generator executable not found: {cfg.generator}")
                    error_folders.append(cur_wkdir)
                except Exception as e:
                    logger.error(f"Unexpected error processing {cur_wkdir}: {e}")
                    error_folders.append(cur_wkdir)

    # Summary
    logger.info(f"Processing complete. Successfully processed: {processed_count}, Errors: {len(error_folders)}")

    for path in error_folders:
        print(path)


if __name__ == '__main__':
    # -p /tmp/mutate_result -g /home/lzz/RustProjects/sv-analysis/target/debug/test_analysis
    parser = ArgumentParser()
    parser.add_argument("--path", "-p", help="root path of mutate result", required=True)
    parser.add_argument("--generator", "-g", help="path of test_analysis", required=True)

    args = parser.parse_args()
    main(args)
