#!/usr/bin/env python3
"""
NutShell Test Analysis — Step 3 of BluesFL pipeline for NutShell.

Parses NutShell emu (DiffTest) output to generate test_info.json,
similar to the paper's test_analysis.rs for Ibex.

Usage:
    python3 nutshell_test_analysis.py \
        --emu-log /tmp/emu_output.log \
        --bug-id U6 \
        --output test_info.json

The emu log is captured by running:
    ./build/emu ... --dump-commit-trace 2>&1 | tee /tmp/emu_output.log
"""

import argparse
import json
import re
import sys
from pathlib import Path
from dataclasses import dataclass, field
from typing import Optional


# NutShell scope prefix for Backend
BACKEND_SCOPE = "TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend"
TOP_SCOPE = "TOP.SimTop.cpu.soc.nutcore"
TOP_MODULE = "SimTop"


@dataclass
class EmuResult:
    """Parsed result from NutShell emu output."""
    abort_pc: Optional[int] = None
    instr_cnt: Optional[int] = None
    cycle_cnt: Optional[int] = None
    guest_cycle: Optional[int] = None
    commit_instrs: list = field(default_factory=list)
    mismatch_fields: list = field(default_factory=list)
    dut_pc: Optional[int] = None
    ref_pc: Optional[int] = None


def parse_emu_log(log_content: str) -> EmuResult:
    """Parse NutShell emu stderr output."""
    result = EmuResult()

    # Parse ABORT pc: "ABORT at pc = 0x80000012"
    m = re.search(r'ABORT at pc = (0x[0-9a-fA-F]+)', log_content)
    if m:
        result.abort_pc = int(m.group(1), 16)

    # Parse instrCnt and cycleCnt: "Core-0 instrCnt = 4, cycleCnt = 1,536"
    m = re.search(r'Core-0 instrCnt = (\d+), cycleCnt = ([\d,]+)', log_content)
    if m:
        result.instr_cnt = int(m.group(1))
        result.cycle_cnt = int(m.group(2).replace(',', ''))

    # Parse guest cycle: "Guest cycle spent: 1,537"
    m = re.search(r'Guest cycle spent: ([\d,]+)', log_content)
    if m:
        result.guest_cycle = int(m.group(1).replace(',', ''))

    # Parse commit instruction trace:
    # "[03] commit pc 000000008000000c inst 00008067 wen 0 dst 00 data 0000000080000011 idx 000"
    for m in re.finditer(
        r'\[(\d+)\] commit pc ([0-9a-fA-F]+) inst ([0-9a-fA-F]+) wen (\d) dst (\d+) data ([0-9a-fA-F]+)',
        log_content
    ):
        result.commit_instrs.append({
            'idx': int(m.group(1)),
            'pc': int(m.group(2), 16),
            'inst': int(m.group(3), 16),
            'wen': int(m.group(4)),
            'dst': int(m.group(5)),
            'data': int(m.group(6), 16),
        })

    # Parse REPORT_DIFFERENCE lines:
    # "      pc different at pc = 0x80000012, right = 0x80000010, wrong = 0x80000012"
    for m in re.finditer(
        r'(\w+) different at pc = (0x[0-9a-fA-F]+), right = (0x[0-9a-fA-F]+), wrong = (0x[0-9a-fA-F]+)',
        log_content
    ):
        result.mismatch_fields.append({
            'field': m.group(1),
            'pc': int(m.group(2), 16),
            'right': int(m.group(3), 16),
            'wrong': int(m.group(4), 16),
        })

    # Parse DUT commit pc vs REF pc for pc_mismatch
    # Look for "pc different" in mismatch fields
    for mf in result.mismatch_fields:
        if mf['field'] == 'pc':
            result.dut_pc = mf['wrong']
            result.ref_pc = mf['right']
            break

    # If no explicit mismatch field but we have commit trace + ABORT,
    # the last committed instruction is where the error manifests
    if not result.mismatch_fields and result.commit_instrs and result.abort_pc:
        last_instr = result.commit_instrs[-1]
        result.dut_pc = result.abort_pc
        result.ref_pc = last_instr['data']  # data field often has the expected value

    return result


def determine_start_sig(result: EmuResult) -> tuple:
    """
    Determine start_scope and start_sig based on mismatch type.

    Returns (start_scope, start_sig, test_info_description)
    """
    if result.mismatch_fields:
        first_mismatch = result.mismatch_fields[0]
        field = first_mismatch['field']

        if field == 'pc':
            # PC mismatch — usually a redirect/branch target error
            return (
                BACKEND_SCOPE,
                "io_redirect_target",
                f"PC mismatch at pc=0x{first_mismatch['pc']:x}, "
                f"expected 0x{first_mismatch['right']:x}, "
                f"got 0x{first_mismatch['wrong']:x}"
            )
        elif field.startswith('x') or field in ('ra', 'sp', 'gp', 'tp', 't0', 'a0'):
            # Register value mismatch
            return (
                BACKEND_SCOPE,
                "io_out_bits",
                f"Register {field} mismatch at pc=0x{first_mismatch['pc']:x}, "
                f"expected 0x{first_mismatch['right']:x}, "
                f"got 0x{first_mismatch['wrong']:x}"
            )
        else:
            # Other CSR/special register mismatch
            return (
                BACKEND_SCOPE,
                "io_redirect_target",
                f"{field} mismatch at pc=0x{first_mismatch['pc']:x}"
            )
    else:
        # No explicit mismatch line — use commit trace + ABORT to infer
        if result.commit_instrs:
            last = result.commit_instrs[-1]
            inst_hex = f"0x{last['inst']:08x}"
            return (
                BACKEND_SCOPE,
                "io_redirect_target",
                f"ABORT at pc=0x{result.abort_pc:x} after {result.instr_cnt} instructions, "
                f"last commit pc=0x{last['pc']:x} inst={inst_hex}"
            )
        else:
            return (
                BACKEND_SCOPE,
                "io_redirect_target",
                f"ABORT at pc=0x{result.abort_pc:x} after {result.instr_cnt} instructions"
            )


def compute_start_time(result: EmuResult, time_step: int = 2) -> int:
    """
    Compute start_time from simulation results.

    NOTE: NutShell's FST time units do NOT map 1:1 to cycle*2.
    The actual start_time must be determined by probing the FST waveform
    (running sv_analysis once with start_time=0 and reading the log).

    This function returns 0 as a placeholder — the fl_run_all pipeline
    will do a probe run to find the exact start_time.
    """
    return 0


def compute_time_bound(start_time: int, time_step: int = 2) -> int:
    """
    Compute time_bound (how far back to trace).
    Paper formula: time_bound = start_time - 2 * ibex_cycle
    For NutShell: use a fixed lookback of 15 time_step units.
    """
    if start_time > 0:
        return max(start_time - 30, 0)
    return 0


def generate_test_info(
    emu_log_path: str,
    bug_id: str,
    output_path: str,
    time_step: int = 2,
    start_scope_override: str = None,
    start_sig_override: str = None,
    start_time_override: int = None,
    time_bound_override: int = None,
):
    """Main entry: parse emu log and generate test_info.json."""

    log_content = Path(emu_log_path).read_text()
    result = parse_emu_log(log_content)

    if result.abort_pc is None:
        print(f"Warning: No ABORT found in {emu_log_path}", file=sys.stderr)
        # Still generate with defaults

    # Determine scope and signal
    if start_scope_override and start_sig_override:
        start_scope = start_scope_override
        start_sig = start_sig_override
        test_info = f"Bug {bug_id}: manual override"
    else:
        start_scope, start_sig, test_info = determine_start_sig(result)

    # Compute times
    if start_time_override is not None:
        start_time = start_time_override
    else:
        start_time = compute_start_time(result, time_step)

    if time_bound_override is not None:
        time_bound = time_bound_override
    else:
        time_bound = compute_time_bound(start_time, time_step)

    meta = {
        "bug_id": bug_id,
        "start_scope": start_scope,
        "start_sig": start_sig,
        "start_time": start_time,
        "start_time_probed": False,  # True after probe run determines actual time
        "test_info": test_info,
        "time_bound": time_bound,
        "time_step": time_step,
        "top_module": TOP_MODULE,
        "top_scope": TOP_SCOPE,
        # NutShell-specific fields
        "emu_abort_pc": result.abort_pc,
        "emu_instr_cnt": result.instr_cnt,
        "emu_cycle_cnt": result.cycle_cnt,
        "emu_guest_cycle": result.guest_cycle,
        "commit_instrs": result.commit_instrs,
        "mismatch_fields": result.mismatch_fields,
    }

    output = Path(output_path)
    output.parent.mkdir(parents=True, exist_ok=True)
    with open(output, 'w') as f:
        json.dump(meta, f, indent=2, default=str)

    print(f"Generated {output_path}:")
    print(f"  bug_id     = {bug_id}")
    print(f"  start_scope = {start_scope}")
    print(f"  start_sig  = {start_sig}")
    print(f"  start_time = {start_time}")
    print(f"  time_bound = {time_bound}")
    print(f"  abort_pc   = 0x{result.abort_pc:x}" if result.abort_pc else "  abort_pc   = N/A")

    return meta


def main():
    parser = argparse.ArgumentParser(description="NutShell Test Analysis for BluesFL")
    parser.add_argument("--emu-log", required=True, help="Path to emu output log")
    parser.add_argument("--bug-id", required=True, help="Bug identifier (e.g., U6)")
    parser.add_argument("--output", required=True, help="Output test_info.json path")
    parser.add_argument("--time-step", type=int, default=2, help="Time step (default: 2)")
    parser.add_argument("--start-scope", default=None, help="Override start scope")
    parser.add_argument("--start-sig", default=None, help="Override start signal")
    parser.add_argument("--start-time", type=int, default=None, help="Override start time")
    parser.add_argument("--time-bound", type=int, default=None, help="Override time bound")

    args = parser.parse_args()
    generate_test_info(
        emu_log_path=args.emu_log,
        bug_id=args.bug_id,
        output_path=args.output,
        time_step=args.time_step,
        start_scope_override=args.start_scope,
        start_sig_override=args.start_sig,
        start_time_override=args.start_time,
        time_bound_override=args.time_bound,
    )


if __name__ == "__main__":
    main()
