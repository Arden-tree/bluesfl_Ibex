"""
This script is used to update project files, then inject the mutated files.
"""
import shutil
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

    folders = [p for p in root_path.glob("*") if "tmp" not in p.name]
    logger.info(f"Found {len(folders)} folders to process (excluding tmp folders)")

    error_folders = []

    for cur_dir in folders:
        if not cur_dir.is_dir():
            continue

        # Skip "tmp" folder and its subdirectories
        if "tmp" in cur_dir.name:
            continue

        diff_file = None
        for f in cur_dir.glob("*.diff"):
            if f.is_file():
                diff_file = f

        cur_ibex_root = cur_dir / cur_dir.name

        if diff_file:
            logger.info(f"processing in cur_dir={cur_dir}")
            file_suffix = diff_file.name.split(".")[1]
            file_name = diff_file.name.split(".")[0]
            mutated_file_name = (file_name + "." + file_suffix)
            mutated_file_path = cur_dir / mutated_file_name
            shutil.rmtree(cur_ibex_root)
            shutil.copytree(Path(cfg.ibex_root), cur_ibex_root)
            shutil.copyfile(mutated_file_path, cur_ibex_root / "rtl" / mutated_file_name)

            # Run the ibex boot script
            try:
                cmd = [
                    "bash",
                    cfg.ibex_boot_script,
                    str(cur_ibex_root)
                ]
                logger.info(f"Running command: {' '.join(cmd)}")
                result = subprocess.run(cmd, capture_output=True, text=True, check=True, cwd=cur_ibex_root)

                logger.info(f"Successfully processed {cur_ibex_root}")
                if result.stdout:
                    logger.info(f"Command stdout: {result.stdout.strip()}")

                if result.stderr:
                    logger.info(f"Command stderr: {result.stderr.strip()}")

            except subprocess.CalledProcessError as e:
                logger.error(f"Error running ibex boot script for {cur_ibex_root}: {e}")
                if e.stderr:
                    logger.error(f"Error output: {e.stderr.strip()}")
                error_folders.append(cur_ibex_root)
            except FileNotFoundError:
                logger.error(f"ibex boot script executable not found: {cfg.ibex_boot_script}")
                error_folders.append(cur_ibex_root)
            except Exception as e:
                logger.error(f"Unexpected error processing {cur_ibex_root}: {e}")
                error_folders.append(cur_ibex_root)

    # Summary
    logger.info(f"Processing complete. Errors: {len(error_folders)}")

    for path in error_folders:
        print(path)


if __name__ == '__main__':
    # -p /tmp/mutate_result
    # -r /home/lzz/exp_wkdir/ibex_test/ibex
    # -s /home/lzz/RustProjects/sv-analysis/scripts/ibex_boot.sh
    parser = ArgumentParser()
    parser.add_argument("--path", "-p", help="root path of mutate result", required=True)
    parser.add_argument("--ibex-root", "-r", help="root path of Ibex project", required=True)
    parser.add_argument("--ibex-boot-script", "-s", help="path of Ibex boot script", required=True)

    args = parser.parse_args()
    main(args)
