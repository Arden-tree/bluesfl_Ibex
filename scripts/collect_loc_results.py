#!/usr/bin/env python3
"""
Script to merge multiple JSON files from different bug_id folders.
Only uses the highest numbered ${res_prefix}_* subfolder for each bug_id.
"""

import json
import os
import re
from pathlib import Path
from typing import List, Dict, Any

res_prefix = "llm"


def find_highest_res_folder(bug_folder: Path) -> Path:
    """
    Find the subfolder with the highest ${res_prefix}_* number in a bug folder.

    Args:
        bug_folder: Path to the bug folder (e.g., mutate_result/75/)

    Returns:
        Path to the highest numbered ${res_prefix}_* folder
    """
    loc_folders = []

    # Look for ${res_prefix}_* folders
    for item in bug_folder.iterdir():
        if item.is_dir() and item.name.startswith(f'{res_prefix}_'):
            # Extract the number from ${res_prefix}_res_X
            match = re.search(rf'{res_prefix}_(\d+)', item.name)
            if match:
                res_number = int(match.group(1))
                loc_folders.append((res_number, item))

    if not loc_folders:
        raise ValueError(f"No {res_prefix}_* folders found in {bug_folder}")

    # Sort by number and return the highest one
    loc_folders.sort(key=lambda x: x[0], reverse=True)
    return loc_folders[0][1]


def load_json_file(file_path: Path) -> List[Dict[Any, Any]]:
    """
    Load JSON data from a file.

    Args:
        file_path: Path to the JSON file

    Returns:
        List of dictionaries containing the JSON data
    """
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            data = json.load(f)
            if not isinstance(data, list):
                print(f"Warning: {file_path} doesn't contain a list. Wrapping in list.")
                return [data]
            return data
    except FileNotFoundError:
        print(f"Warning: File not found: {file_path}")
        return []
    except json.JSONDecodeError as e:
        print(f"Error: Invalid JSON in {file_path}: {e}")
        return []
    except Exception as e:
        print(f"Error reading {file_path}: {e}")
        return []


def merge_json_files(root_folder: str = "mutate_result", output_file: str = "merged_results.json") -> None:
    """
    Merge JSON files from different bug_id folders.

    Args:
        root_folder: Root folder containing bug_id subfolders
        output_file: Output file name for merged JSON
    """
    root_path = Path(root_folder)

    if not root_path.exists():
        raise FileNotFoundError(f"Root folder '{root_folder}' not found")

    merged_data = []
    processed_bugs = []

    # Iterate through all subdirectories in the root folder
    for bug_folder in root_path.iterdir():
        if not bug_folder.is_dir():
            continue

        # Extract bug_id from folder name
        bug_id = bug_folder.name

        try:
            # Find the highest numbered ${res_prefix}_* folder
            highest_res_folder = find_highest_res_folder(bug_folder)

            # Find matching files
            matching_files = list(highest_res_folder.glob(f"*_loc_results_{bug_id}.json"))

            if not matching_files:
                print(f"No matching file found for bug_id {bug_id}")
                json_data = None
            else:
                # Take the first match (sorted for consistency)
                json_file_path = sorted(matching_files)[0]
                print(f"Processing bug_id {bug_id}: {json_file_path}")
                # Load JSON data
                json_data = load_json_file(json_file_path)

            if json_data:
                merged_data.extend(json_data)
                processed_bugs.append(bug_id)
                print(f"  - Added {len(json_data)} records from bug_id {bug_id}")
            else:
                print(f"  - No data found for bug_id {bug_id}")

        except ValueError as e:
            print(f"Error processing bug_id {bug_id}: {e}")
        except Exception as e:
            print(f"Unexpected error processing bug_id {bug_id}: {e}")

    # Write merged data to output file
    try:
        output_file = Path(output_file)
        if not output_file.parent.exists():
            output_file.parent.mkdir(parents=True, exist_ok=True)

        with open(output_file, 'w', encoding='utf-8') as f:
            json.dump(merged_data, f, indent=2, ensure_ascii=False)

        print(f"\nMerge completed successfully!")
        print(f"Total records merged: {len(merged_data)}")
        print(
            f"Processed bug_ids: {sorted(processed_bugs, key=int) if all(bid.isdigit() for bid in processed_bugs) else sorted(processed_bugs)}")
        print(f"Output file: {output_file}")

    except Exception as e:
        print(f"Error writing output file: {e}")


def main():
    """Main function to run the JSON merger."""
    import argparse

    parser = argparse.ArgumentParser(description="Merge JSON files from different bug_id folders")
    parser.add_argument(
        "--root", "-r",
        default="mutate_result",
        help="Root folder containing bug_id subfolders (default: mutate_result)"
    )
    parser.add_argument(
        "--output", "-o",
        default="merged_results.json",
        help="Output file name (default: merged_results.json)"
    )
    parser.add_argument(
        "--prefix", "-p",
        default="llm",
        help="Result file prefix, {prefix}_loc_results_{bug_id}.json"
    )
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Enable verbose output"
    )

    args = parser.parse_args()
    global res_prefix
    res_prefix = args.prefix

    try:
        merge_json_files(args.root, args.output)
    except Exception as e:
        print(f"Fatal error: {e}")
        return 1

    return 0


if __name__ == "__main__":
    # --root=/home/lzz/dac26/hdl_fl_data/mutate_result
    # --output=./sbfl_merged_results.json
    # --prefix=llm
    exit(main())
