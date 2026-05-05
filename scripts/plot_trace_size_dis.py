#!/usr/bin/env python3
"""
Only uses the highest numbered ${res_prefix}_* subfolder for each bug_id.
"""

import json
import re
from pathlib import Path
from typing import List, Dict, Any
import matplotlib.pyplot as plt
import numpy as np
from matplotlib.patches import Patch


def find_highest_res_folder(prefix: str, bug_folder: Path) -> Path:
    """
    Find the subfolder with the highest ${prefix}_* number in a bug folder.

    Args:
        bug_folder: Path to the bug folder (e.g., mutate_result/75/)

    Returns:
        Path to the highest numbered ${prefix}_* folder
    """
    loc_folders = []

    # Look for ${prefix}_* folders
    for item in bug_folder.iterdir():
        if item.is_dir() and item.name.startswith(f'{prefix}_'):
            # Extract the number from ${prefix}_res_X
            match = re.search(rf'{prefix}_(\d+)', item.name)
            if match:
                res_number = int(match.group(1))
                loc_folders.append((res_number, item))

    if not loc_folders:
        raise ValueError(f"No {prefix}_* folders found in {bug_folder}")

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


def merge_trace_size(prefix: str, root_folder: str):
    """
    Merge JSON files from different bug_id folders.

    Args:
        root_folder: Root folder containing bug_id subfolders
        output_file: Output file name for merged JSON
    """
    root_path = Path(root_folder)

    if not root_path.exists():
        raise FileNotFoundError(f"Root folder '{root_folder}' not found")

    merged_data = {}
    processed_bugs = []

    # Iterate through all subdirectories in the root folder
    for bug_folder in root_path.iterdir():
        if not bug_folder.is_dir():
            continue

        # Extract bug_id from folder name
        bug_id = bug_folder.name

        try:
            # Find the highest numbered ${prefix}_* folder
            highest_res_folder = find_highest_res_folder(prefix, bug_folder)

            # Find matching files
            trace_file = list(highest_res_folder.glob(f"trace.json"))[0]
            print(f"Processing bug_id {bug_id}: {trace_file}")
            json_data = load_json_file(trace_file)

            if json_data:
                merged_data[bug_id] = json_data
                processed_bugs.append(bug_id)
                # print(f"  - Added {trace_file} records from bug_id {bug_id}")
            else:
                print(f"  - No trace data found for bug_id {bug_id}")

        except ValueError as e:
            print(f"Error processing bug_id {bug_id}: {e}")
        except Exception as e:
            print(f"Unexpected error processing bug_id {bug_id}: {e}")

    return merged_data


def save_violin_fig(name_map, data, save_path="trace_len_dis.pdf"):
    plot_data = {}
    for exp_name, bids in data.items():
        plot_data[exp_name] = {}
        for bid in bids:
            plot_data[exp_name][bid] = len(bids[bid])

    # Prepare data for all experiments
    exp_names = list(plot_data.keys())
    all_trace_lengths = []
    valid_exp_names = []

    for exp_name in exp_names:
        trace_lengths = [t for t in plot_data[exp_name].values() if t > 0]
        if trace_lengths:
            all_trace_lengths.append(trace_lengths)
            if exp_name not in name_map:
                print(f"Cannot found {exp_name} in name_map, use default")
            exp_name = name_map[exp_name] if exp_name in name_map else exp_name
            valid_exp_names.append(exp_name)

    if not all_trace_lengths:
        print(f"Warning: No valid trace data found in any experiment")
        print(f"Available experiments: {list(data.keys())}")
        return

    # Create violin plot for all experiments
    num_exps = len(valid_exp_names)
    fig, ax = plt.subplots(figsize=(8, 3))

    # Adjust margins to minimize whitespace
    plt.subplots_adjust(left=0.1, right=0.98, top=0.95, bottom=0.05)

    positions = list(range(1, num_exps + 1))
    parts = ax.violinplot(all_trace_lengths,
                          positions=positions,
                          showmeans=True,
                          showmedians=True,
                          widths=0.7)

    # Customize violin colors
    colors = ['#8dd3c7', '#ffffb3', '#bebada', '#fb8072', '#80b1d3']
    for i, pc in enumerate(parts['bodies']):
        pc.set_facecolor(colors[i % len(colors)])
        pc.set_alpha(0.7)
        pc.set_edgecolor('black')
        pc.set_linewidth(1.5)

    # Customize other elements
    parts['cmeans'].set_color('red')
    parts['cmeans'].set_linewidth(2)
    parts['cmeans'].set_label('Mean')
    parts['cmedians'].set_color('blue')
    parts['cmedians'].set_linewidth(2)
    parts['cmedians'].set_label('Median')
    parts['cbars'].set_color('black')
    parts['cmaxes'].set_color('black')
    parts['cmins'].set_color('black')

    # Add labels and title
    ax.set_ylabel('# of Checked Blocks', fontsize=14)
    # ax.set_title('Trace Length Distribution Across Experiments', fontsize=14)
    ax.set_xticks([])  # Remove x-axis ticks
    ax.grid(axis='y', alpha=0.3, linestyle='--')

    # Add statistics for each experiment
    for i, (pos, trace_lengths) in enumerate(zip(positions, all_trace_lengths)):
        # stats_text = f'n={len(trace_lengths)}\nμ={np.mean(trace_lengths):.1f}\nM={np.median(trace_lengths):.1f}'
        stats_text = f'μ={np.mean(trace_lengths):.1f}'
        y_pos = np.max(trace_lengths) * 0.95
        ax.text(pos, y_pos, stats_text,
                fontsize=12, ha='center', verticalalignment='top',
                bbox=dict(boxstyle='round', facecolor='white', alpha=0.7, edgecolor='gray'))

    # Create legend with experiment names and colors
    legend_elements = []
    for i, (exp_name, color) in enumerate(zip(valid_exp_names, colors)):
        legend_elements.append(Patch(facecolor=color, edgecolor='black',
                                     alpha=0.7, label=exp_name))

    # Add mean and median to legend
    from matplotlib.lines import Line2D
    legend_elements.append(Line2D([0], [0], color='red', linewidth=2, label='Mean'))
    legend_elements.append(Line2D([0], [0], color='blue', linewidth=2, label='Median'))

    ax.legend(handles=legend_elements, loc='upper center', fontsize=12,
              framealpha=0.9, edgecolor='black')

    plt.tight_layout()
    plt.savefig(save_path, dpi=300, bbox_inches='tight', pad_inches=0.05)
    print(f"Violin plot saved to {save_path}")
    # plt.close()
    plt.show()


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
        default="trace_size_dis.pdf",
        help="Output file name (default: merged_results.json)"
    )
    parser.add_argument(
        "--prefix", "-p",
        type=str,
        nargs="+",
        required=True,
        help="list of prefix"
    )
    parser.add_argument(
        "--name-map", "-n",
        type=str,
        nargs="+",
        required=True,
        help="list of prefix"
    )
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Enable verbose output"
    )

    args = parser.parse_args()
    # global res_prefix
    # res_prefix = args.prefix

    assert len(args.name_map) == len(args.prefix)
    name_map = dict([(args.prefix[i], args.name_map[i]) for i in range(len(args.prefix))])

    trace_size_data = {}

    for prefix in args.prefix:
        try:
            trace_data = merge_trace_size(prefix, args.root)
            trace_size_data[prefix] = trace_data
        except Exception as e:
            print(f"Fatal error: {e}")
            return 1

    save_violin_fig(name_map, trace_size_data, args.output)
    return 0


if __name__ == "__main__":
    """
    --root /home/lzz/dac26/hdl_fl_data/dataset
    --prefix biosfl_res_gpt-4o_vt2_vk2 biosfl_res_b268051_ablation_rm_exe_path_gpt-4o_vt2_vk2 biosfl_res_fd36321_ablation_rm_sig_values_gpt-4o_vt2_vk2
    --name-map BluesFL "w/o instruction path" "w/o signal values"
    """
    exit(main())
