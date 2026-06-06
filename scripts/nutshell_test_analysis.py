#!/usr/bin/env python3
"""
NutShell Test Analysis — Step 3 of BluesFL pipeline for NutShell.

Parses NutShell emu (DiffTest) output to generate test_info.json,
aligned with the paper's Test Report format (I, sig, t, E).

Test Report = (I, sig, t, E):
  I: failing instruction (decoded from commit trace)
  sig: suspicious signal (inferred from mismatch type)
  t: failure time (probed from FST waveform or manually specified)
  E: expected behavior in natural language, e.g.
     "Instruction jalr incorrectly jumps to 0x80000011,
      whereas it should jump to 0x80000010"

Usage:
    python3 nutshell_test_analysis.py \\
        --emu-log /tmp/emu_output.log \\
        --bug-id U6_jalr_bit0_not_cleared \\
        --output test_info.json

    # With auto-probe of start_time from FST:
    python3 nutshell_test_analysis.py \\
        --emu-log /tmp/emu_output.log \\
        --bug-id U6_jalr_bit0_not_cleared \\
        --output test_info.json \\
        --wave-path /path/to/sim.fst \\
        --sv-analysis ./target/debug/sv_analysis \\
        --project-path /path/to/build/rtl

The emu log is captured by running:
    ./build/emu ... --dump-commit-trace 2>&1 | tee /tmp/emu_output.log
"""

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from dataclasses import dataclass, field
from typing import Optional


# NutShell scope constants
BACKEND_SCOPE = "TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend"
TOP_SCOPE = "TOP.SimTop.cpu.soc.nutcore"
TOP_MODULE = "SimTop"


# ---------------------------------------------------------------------------
# RISC-V instruction decoding (minimal, for test report generation)
# ---------------------------------------------------------------------------

_REG_NAMES = [
    'zero', 'ra', 'sp', 'gp', 'tp', 't0', 't1', 't2',
    's0', 's1', 'a0', 'a1', 'a2', 'a3', 'a4', 'a5',
    'a6', 'a7', 's2', 's3', 's4', 's5', 's6', 's7',
    's8', 's9', 's10', 's11', 't3', 't4', 't5', 't6',
]

_REG_ALIASES = {
    'zero': 'x0', 'ra': 'x1', 'sp': 'x2', 'gp': 'x3', 'tp': 'x4',
}


def _reg(idx):
    return _REG_NAMES[idx] if idx < len(_REG_NAMES) else f'x{idx}'


def _sign_extend(val, bits):
    if val & (1 << (bits - 1)):
        val -= (1 << bits)
    return val


def decode_rv64i(inst):
    """Decode a 32-bit RISC-V RV64I instruction to a readable string.

    Returns (mnemonic, operands_str) or ('unknown', hex) if unrecognized.
    """
    opcode = inst & 0x7f
    rd = (inst >> 7) & 0x1f
    funct3 = (inst >> 12) & 0x7
    rs1 = (inst >> 15) & 0x1f
    rs2 = (inst >> 20) & 0x1f
    funct7 = (inst >> 25) & 0x7f

    # I-type immediate
    imm_i = _sign_extend((inst >> 20) & 0xfff, 12)

    # S-type immediate
    imm_s = _sign_extend(((inst >> 7) & 0x1f) | (((inst >> 25) & 0x7f) << 5), 12)

    # B-type immediate
    imm_b = _sign_extend(
        (((inst >> 8) & 0xf) << 1) | (((inst >> 25) & 0x3f) << 5) |
        (((inst >> 7) & 0x1) << 11) | (((inst >> 31) & 0x1) << 12), 13)

    # U-type immediate
    imm_u = _sign_extend((inst >> 12) & 0xfffff, 20) << 12
    # Actually U-type is not sign-extended in the standard way, keep upper 20 bits
    imm_u = ((inst >> 12) & 0xfffff) << 12

    # J-type immediate
    imm_j = _sign_extend(
        (((inst >> 21) & 0x3ff) << 1) | (((inst >> 20) & 0x1) << 11) |
        (((inst >> 12) & 0xff) << 12) | (((inst >> 31) & 0x1) << 20), 21)

    if opcode == 0x37:  # LUI
        return 'lui', f'{_reg(rd)}, 0x{(imm_u >> 12):x}'
    elif opcode == 0x17:  # AUIPC
        return 'auipc', f'{_reg(rd)}, 0x{(imm_u >> 12):x}'
    elif opcode == 0x6f:  # JAL
        target = imm_j  # signed offset
        return 'jal', f'{_reg(rd)}, {imm_j}'
    elif opcode == 0x67:  # JALR
        return 'jalr', f'{_reg(rd)}, {imm_i}({_reg(rs1)})'
    elif opcode == 0x63:  # Branch
        bops = {0: 'beq', 1: 'bne', 4: 'blt', 5: 'bge', 6: 'bltu', 7: 'bgeu'}
        op = bops.get(funct3, f'b?{funct3}')
        return op, f'{_reg(rs1)}, {_reg(rs2)}, {imm_b}'
    elif opcode == 0x03:  # Load
        lops = {0: 'lb', 1: 'lh', 2: 'lw', 3: 'ld', 4: 'lbu', 5: 'lhu', 6: 'lwu'}
        op = lops.get(funct3, f'l?{funct3}')
        return op, f'{_reg(rd)}, {imm_i}({_reg(rs1)})'
    elif opcode == 0x23:  # Store
        sops = {0: 'sb', 1: 'sh', 2: 'sw', 3: 'sd'}
        op = sops.get(funct3, f's?{funct3}')
        return op, f'{_reg(rs2)}, {imm_s}({_reg(rs1)})'
    elif opcode == 0x13:  # OP-IMM
        iops = {0: 'addi', 4: 'xori', 6: 'ori', 7: 'andi'}
        if funct3 == 1:
            shamt = (inst >> 20) & 0x3f
            return 'slli', f'{_reg(rd)}, {_reg(rs1)}, {shamt}'
        elif funct3 == 5:
            shamt = (inst >> 20) & 0x3f
            if funct7 == 0x10:
                return 'srai', f'{_reg(rd)}, {_reg(rs1)}, {shamt}'
            else:
                return 'srli', f'{_reg(rd)}, {_reg(rs1)}, {shamt}'
        else:
            op = iops.get(funct3, f'op-imm?{funct3}')
            return op, f'{_reg(rd)}, {_reg(rs1)}, {imm_i}'
    elif opcode == 0x33:  # OP
        rops = {
            (0, 0): 'add', (0, 0x20): 'sub',
            (1, 0): 'sll', (2, 0): 'slt', (3, 0): 'sltu',
            (4, 0): 'xor', (5, 0): 'srl', (5, 0x20): 'sra',
            (6, 0): 'or', (7, 0): 'and',
        }
        op = rops.get((funct3, funct7), f'op?{funct3}.{funct7}')
        return op, f'{_reg(rd)}, {_reg(rs1)}, {_reg(rs2)}'
    elif opcode == 0x1b:  # OP-IMM-32
        if funct3 == 0:
            return 'addiw', f'{_reg(rd)}, {_reg(rs1)}, {imm_i}'
        elif funct3 == 1:
            shamt = (inst >> 20) & 0x3f
            return 'slliw', f'{_reg(rd)}, {_reg(rs1)}, {shamt}'
        elif funct3 == 5:
            shamt = (inst >> 20) & 0x3f
            op = 'sraiw' if funct7 == 0x10 else 'srliw'
            return op, f'{_reg(rd)}, {_reg(rs1)}, {shamt}'
    elif opcode == 0x3b:  # OP-32
        if funct3 == 0 and funct7 == 0:
            return 'addw', f'{_reg(rd)}, {_reg(rs1)}, {_reg(rs2)}'
        elif funct3 == 0 and funct7 == 0x20:
            return 'subw', f'{_reg(rd)}, {_reg(rs1)}, {_reg(rs2)}'
    elif opcode == 0x73:  # SYSTEM
        csr = (inst >> 20) & 0xfff
        if funct3 == 0:
            if inst >> 20 == 0:
                return 'ecall', ''
            elif inst >> 20 == 1:
                return 'ebreak', ''
            elif (inst >> 20) & 0xfff == 0x105:
                return 'wfi', ''
            elif (inst >> 20) & 0xfff == 0x120:
                return 'sfence.vma', f'{_reg(rs1)}, {_reg(rs2)}' if rs1 or rs2 else 'sfence.vma'
            elif funct7 == 0x09:
                return 'sfmt', ''
            else:
                return 'mret' if csr == 0x302 else 'priv', f'0x{csr:x}'
        else:
            cops = {1: 'csrrw', 2: 'csrrs', 3: 'csrrc', 5: 'csrrwi', 6: 'csrrsi', 7: 'csrrci'}
            op = cops.get(funct3, f'system?{funct3}')
            return op, f'{_reg(rd)}, 0x{csr:x}, {_reg(rs1)}'
    elif opcode == 0x0f:  # MISC-MEM
        if funct3 == 0:
            return 'fence', ''
        elif funct3 == 1:
            return 'fence.i', ''

    return 'unknown', f'0x{inst:08x}'


def format_instruction(pc, inst):
    """Format a single instruction as 'mnemonic operands'."""
    mnem, ops = decode_rv64i(inst)
    return f'{mnem} {ops}'.strip()


# ---------------------------------------------------------------------------
# Emu log parsing
# ---------------------------------------------------------------------------

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
    ref_regs: dict = field(default_factory=dict)


def parse_emu_log(log_content: str) -> EmuResult:
    """Parse NutShell emu stderr output."""
    result = EmuResult()

    # Parse ABORT pc
    m = re.search(r'ABORT at pc = (0x[0-9a-fA-F]+)', log_content)
    if m:
        result.abort_pc = int(m.group(1), 16)

    # Parse instrCnt and cycleCnt
    m = re.search(r'Core-0 instrCnt = (\d+), cycleCnt = ([\d,]+)', log_content)
    if m:
        result.instr_cnt = int(m.group(1))
        result.cycle_cnt = int(m.group(2).replace(',', ''))

    # Parse guest cycle
    m = re.search(r'Guest cycle spent: ([\d,]+)', log_content)
    if m:
        result.guest_cycle = int(m.group(1).replace(',', ''))

    # Parse commit instruction trace (deduplicate by idx)
    seen = set()
    for m in re.finditer(
        r'\[(\d+)\] commit pc ([0-9a-fA-F]+) inst ([0-9a-fA-F]+) wen (\d) dst (\d+) data ([0-9a-fA-F]+)',
        log_content
    ):
        idx = int(m.group(1))
        if idx in seen:
            continue
        seen.add(idx)
        result.commit_instrs.append({
            'idx': idx,
            'pc': int(m.group(2), 16),
            'inst': int(m.group(3), 16),
            'wen': int(m.group(4)),
            'dst': int(m.group(5)),
            'data': int(m.group(6), 16),
        })

    # Parse REPORT_DIFFERENCE lines
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

    # Parse REF registers (for inferring expected values)
    for m in re.finditer(r'(ra|\$\d+|[a-z]\d{0,2}):\s*(0x[0-9a-fA-F]+)', log_content):
        name = m.group(1)
        val = int(m.group(2), 16)
        result.ref_regs[name] = val

    # Determine dut_pc / ref_pc
    for mf in result.mismatch_fields:
        if mf['field'] == 'pc':
            result.dut_pc = mf['wrong']
            result.ref_pc = mf['right']
            break

    if not result.mismatch_fields and result.commit_instrs and result.abort_pc:
        last_instr = result.commit_instrs[-1]
        result.dut_pc = result.abort_pc
        result.ref_pc = last_instr['data']

    return result


# ---------------------------------------------------------------------------
# Test Report generation: (I, sig, t, E)
# ---------------------------------------------------------------------------

def generate_test_report_E(result: EmuResult) -> str:
    """Generate the E (expected behavior) field aligned with the paper's format.

    Paper example (Figure 6):
      "Instruction jmp pc + 0xa0c0 incorrectly jumps to address 0x000F5FC0,
       whereas it should jump to 0x0010A140."

    Our format follows: "Instruction <mnem> <detail> incorrectly <behavior>,
       whereas it should <expected>."
    """
    if result.mismatch_fields:
        first = result.mismatch_fields[0]
        field = first['field']
        wrong_val = first['wrong']
        right_val = first['right']
        pc = first['pc']

        # Find the instruction at this pc
        instr_desc = _find_instr_at_pc(result, pc)

        if field == 'pc':
            return (
                f"Instruction {instr_desc} at pc=0x{pc:x} incorrectly jumps to "
                f"0x{wrong_val:x}, whereas it should jump to 0x{right_val:x}."
            )
        else:
            return (
                f"Instruction {instr_desc} at pc=0x{pc:x} incorrectly produces "
                f"{field}=0x{wrong_val:x}, whereas it should be 0x{right_val:x}."
            )

    # No explicit mismatch — infer from commit trace + ABORT
    if result.commit_instrs and result.abort_pc:
        last = result.commit_instrs[-1]
        instr_desc = format_instruction(last['pc'], last['inst'])
        dut_pc = result.dut_pc or result.abort_pc
        ref_pc = result.ref_pc or last['data']

        # Detect JALR/JAL redirect error (common case)
        mnem, _ = decode_rv64i(last['inst'])
        if mnem in ('jal', 'jalr'):
            return (
                f"Instruction {instr_desc} at pc=0x{last['pc']:x} incorrectly redirects "
                f"to 0x{dut_pc:x}, whereas it should redirect to 0x{ref_pc:x}."
            )
        else:
            return (
                f"Instruction {instr_desc} at pc=0x{last['pc']:x} causes ABORT at "
                f"pc=0x{result.abort_pc:x}, execution should continue at 0x{ref_pc:x}."
            )

    if result.abort_pc:
        return (
            f"Processor aborts at pc=0x{result.abort_pc:x} after "
            f"{result.instr_cnt or '?'} instructions."
        )

    return "Unknown failure."


def _find_instr_at_pc(result, target_pc):
    """Find and decode the instruction at a given PC from commit trace."""
    for instr in result.commit_instrs:
        if instr['pc'] == target_pc:
            return format_instruction(instr['pc'], instr['inst'])
    return f"(pc=0x{target_pc:x})"


def determine_start_sig(result: EmuResult) -> tuple:
    """Determine (start_scope, start_sig) based on mismatch type."""
    if result.mismatch_fields:
        field = result.mismatch_fields[0]['field']
        if field == 'pc':
            return BACKEND_SCOPE, "io_redirect_target"
        elif field.startswith('x') or field in ('ra', 'sp', 'gp', 'tp', 't0', 'a0'):
            return BACKEND_SCOPE, "io_out_bits"
        else:
            return BACKEND_SCOPE, "io_redirect_target"
    else:
        return BACKEND_SCOPE, "io_redirect_target"


# ---------------------------------------------------------------------------
# start_time probing
# ---------------------------------------------------------------------------

def probe_start_time(
    wave_path: str,
    start_scope: str,
    start_sig: str,
    sv_analysis: str,
    project_path: str,
    include_paths: list,
    top_module: str = TOP_MODULE,
    top_scope: str = TOP_SCOPE,
    time_step: int = 2,
) -> Optional[int]:
    """Probe FST waveform to find the exact start_time.

    Strategy: run sv_analysis with --start-time 0 (a lightweight probe that
    parses RTL but does minimal tracing), then scan the log for the first
    time the start_sig shows a non-trivial (non-zero, non-reset) value.

    If that fails, try with start_time = guest_cycle * time_step as a hint.

    Returns the probed start_time, or None if probing fails.
    """
    if not os.path.isfile(sv_analysis):
        print(f"  [probe] sv_analysis not found: {sv_analysis}", file=sys.stderr)
        return None
    if not os.path.isfile(wave_path):
        print(f"  [probe] wave file not found: {wave_path}", file=sys.stderr)
        return None

    rm_params = "/dev/null"
    coverage_dir = "/tmp/coverage_empty"
    os.makedirs(coverage_dir, exist_ok=True)

    cmd = [
        sv_analysis,
        f"--bug-id=probe",
        "--agent-type=open-ai",
        "--model=gpt-4o-mini",  # won't be used, just needed for arg parse
        f"--project-path={project_path}",
        f"--include-paths={','.join(include_paths)}",
        f"--rm-params-path={rm_params}",
        f"--coverage-path={coverage_dir}",
        f"--wave-path={wave_path}",
        f"--top-module={top_module}",
        f"--top-scope={top_scope}",
        f"--start-scope={start_scope}",
        f"--start-sig={start_sig}",
        "--start-time=0",
        "--time-bound=0",
        f"--time-step={time_step}",
        "--output-path=/tmp/probe_start_time",
        "--vote-total=1",
        "--vote-top-k=1",
        "--test-info=probe run",
    ]

    print(f"  [probe] Running sv_analysis with start_time=0 to find signal timing...")
    try:
        proc = subprocess.run(
            cmd, capture_output=True, text=True,
            timeout=600,  # 10 min max for probe
        )
        output = proc.stdout + proc.stderr
    except subprocess.TimeoutExpired:
        print("  [probe] sv_analysis timed out", file=sys.stderr)
        return None
    except Exception as e:
        print(f"  [probe] sv_analysis failed: {e}", file=sys.stderr)
        return None

    # Parse the log to find signal values at different times
    # Look for lines like: "TRACE: get_block_result for scope=..., sig=..., results=..."
    # or signal value lines from waveform reading
    #
    # Strategy: find the first time io_redirect_target (or start_sig) has a
    # value that is not 0 and not 0x80000000 (typical reset values).
    times_found = []

    # Pattern 1: from display_signal_values_at_time_json output in logs
    # WARN/TRACE lines containing signal values
    for m in re.finditer(
        rf'{re.escape(start_sig)}.*?time[=:]\s*(\d+).*?value[=:]\s*"?(0x[0-9a-fA-F]+)"?',
        output, re.IGNORECASE
    ):
        t = int(m.group(1))
        val = int(m.group(2), 16)
        times_found.append((t, val))

    # Pattern 2: from waveform mgr signal reading
    for m in re.finditer(
        rf'"signal_name":\s*"{re.escape(start_sig)}".*?"time":\s*(\d+).*?"value":\s*"(0x[0-9a-fA-F]+)"',
        output, re.DOTALL
    ):
        t = int(m.group(1))
        val = int(m.group(2), 16)
        times_found.append((t, val))

    if not times_found:
        # Pattern 3: look for any mention of the signal with a time annotation
        for m in re.finditer(
            rf'{re.escape(start_sig)}.*?time[_ ]*(?:annotation)?[=:\s]+(\d+)',
            output, re.IGNORECASE
        ):
            times_found.append((int(m.group(1)), None))

    if times_found:
        # Deduplicate and sort by time
        seen = set()
        unique = []
        for t, v in sorted(times_found):
            if t not in seen:
                seen.add(t)
                unique.append((t, v))

        print(f"  [probe] Found {len(unique)} time points for {start_sig}:")
        for t, v in unique[:10]:
            val_str = f"0x{v:x}" if v is not None else "?"
            print(f"    t={t} val={val_str}")

        # Find the first time where value is non-trivial
        for t, v in unique:
            if v is not None and v != 0 and v != 0x80000000:
                print(f"  [probe] First non-trivial value at t={t} (val=0x{v:x})")
                return t

        # If all values are trivial, return the last time point
        if unique:
            t, v = unique[-1]
            print(f"  [probe] All values trivial, using last time t={t}")
            return t

    print("  [probe] Could not find signal timing from sv_analysis output", file=sys.stderr)
    return None


# ---------------------------------------------------------------------------
# time_bound computation (aligned with paper)
# ---------------------------------------------------------------------------

def compute_time_bound(start_time: int, instr_cnt: int = 0, time_step: int = 2) -> int:
    """Compute time_bound aligned with paper formula.

    Paper: time_bound = start_time - 2 * ibex_cycle
    ibex_cycle is the number of cycles the failing instruction takes to execute.
    For NutShell: approximate as instr_cnt * time_step (each instruction ~1 cycle).
    Falls back to: start_time - 15 * time_step if instr_cnt unknown.
    """
    if start_time <= 0:
        return 0
    if instr_cnt and instr_cnt > 0:
        lookback = instr_cnt * time_step
    else:
        lookback = 15 * time_step
    return max(start_time - lookback, 0)


# ---------------------------------------------------------------------------
# Main generation
# ---------------------------------------------------------------------------

def generate_test_info(
    emu_log_path: str,
    bug_id: str,
    output_path: str,
    time_step: int = 2,
    start_scope_override: str = None,
    start_sig_override: str = None,
    start_time_override: int = None,
    time_bound_override: int = None,
    wave_path: str = None,
    sv_analysis_path: str = None,
    project_path: str = None,
    include_paths: list = None,
):
    """Main entry: parse emu log and generate test_info.json."""

    log_content = Path(emu_log_path).read_text()
    result = parse_emu_log(log_content)

    if result.abort_pc is None:
        print(f"Warning: No ABORT found in {emu_log_path}", file=sys.stderr)

    # --- I: failing instruction ---
    last_instr = result.commit_instrs[-1] if result.commit_instrs else None
    instr_desc = ""
    if last_instr:
        instr_desc = format_instruction(last_instr['pc'], last_instr['inst'])

    # --- sig: start signal ---
    if start_scope_override and start_sig_override:
        start_scope = start_scope_override
        start_sig = start_sig_override
    else:
        start_scope, start_sig = determine_start_sig(result)

    # --- E: expected behavior (paper format) ---
    test_report_E = generate_test_report_E(result)

    # --- t: start_time ---
    start_time = start_time_override
    start_time_probed = False

    if start_time is None:
        # Try auto-probe if wave + sv_analysis paths are provided
        if wave_path and sv_analysis_path and project_path:
            inc = include_paths or [project_path]
            print(f"Auto-probing start_time from {wave_path}...")
            probed = probe_start_time(
                wave_path, start_scope, start_sig,
                sv_analysis_path, project_path, inc,
                time_step=time_step,
            )
            if probed is not None:
                start_time = probed
                start_time_probed = True
                print(f"  Probed start_time = {start_time}")
            else:
                start_time = 0
                print("  Probing failed, start_time = 0 (needs manual override)", file=sys.stderr)
        else:
            start_time = 0
            print("  No --wave-path/--sv-analysis provided, start_time = 0", file=sys.stderr)

    # --- time_bound ---
    if time_bound_override is not None:
        time_bound = time_bound_override
    else:
        time_bound = compute_time_bound(start_time, result.instr_cnt, time_step)

    meta = {
        "bug_id": bug_id,
        "start_scope": start_scope,
        "start_sig": start_sig,
        "start_time": start_time,
        "start_time_probed": start_time_probed,
        "test_info": test_report_E,
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

    print(f"\nGenerated {output_path}:")
    print(f"  bug_id      = {bug_id}")
    print(f"  I (instr)   = {instr_desc}")
    print(f"  sig         = {start_sig}")
    print(f"  t (time)    = {start_time} {'(probed)' if start_time_probed else ''}")
    print(f"  E (report)  = {test_report_E}")
    print(f"  time_bound  = {time_bound}")

    return meta


def main():
    parser = argparse.ArgumentParser(
        description="NutShell Test Analysis for BluesFL — generates Test Report (I,sig,t,E)")
    parser.add_argument("--emu-log", required=True, help="Path to emu output log")
    parser.add_argument("--bug-id", required=True, help="Bug identifier (e.g., U6_jalr_bit0_not_cleared)")
    parser.add_argument("--output", required=True, help="Output test_info.json path")
    parser.add_argument("--time-step", type=int, default=2, help="Time step (default: 2)")
    parser.add_argument("--start-scope", default=None, help="Override start scope")
    parser.add_argument("--start-sig", default=None, help="Override start signal")
    parser.add_argument("--start-time", type=int, default=None, help="Override start time")
    parser.add_argument("--time-bound", type=int, default=None, help="Override time bound")
    parser.add_argument("--wave-path", default=None, help="FST waveform path (for auto-probe)")
    parser.add_argument("--sv-analysis", default=None, help="Path to sv_analysis binary (for auto-probe)")
    parser.add_argument("--project-path", default=None, help="RTL project path (for auto-probe)")
    parser.add_argument("--include-paths", nargs='*', default=None, help="Include paths (for auto-probe)")

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
        wave_path=args.wave_path,
        sv_analysis_path=args.sv_analysis,
        project_path=args.project_path,
        include_paths=args.include_paths,
    )


if __name__ == "__main__":
    main()
