#!/usr/bin/env python3
"""
NutShell FL Run All — Step 4 of BluesFL pipeline for NutShell.

For each bug in the nutshell-sbfl dataset:
1. Apply patch → rebuild NutShell (Chisel → Verilog → Verilator emu)
2. Compile test case → run simulation → capture emu output
3. Run nutshell_test_analysis.py → generate test_info.json
4. Probe FST waveform to determine precise start_time (if needed)
5. Run sv_analysis → localize bug
6. Save results

Usage:
    python3 nutshell_fl_run_all.py \
        --nutshell-path /home/yuan/nutshell-sbfl \
        --localizer /home/yuan/bluesfl/target/debug/sv_analysis \
        --bug U6 \
        --output-dir ./e2e_results
"""

import json
import os
import re
import subprocess
import sys
import shutil
import time
from argparse import ArgumentParser
from datetime import datetime
from pathlib import Path
from typing import Optional


# NutShell scope constants
BACKEND_SCOPE = "TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore.backend"
TOP_SCOPE = "TOP.SimTop.cpu.soc.nutcore"
TOP_MODULE = "SimTop"

# Time step for NutShell (Verilator: each clock edge = 1 time unit, 2 per cycle)
TIME_STEP = 2


def run_cmd(cmd, cwd=None, check=True, capture=True, timeout=None):
    """Run a command and return the result."""
    print(f"  $ {' '.join(cmd) if isinstance(cmd, list) else cmd}")
    result = subprocess.run(
        cmd, cwd=cwd, capture_output=capture, text=True,
        check=False, timeout=timeout
    )
    if check and result.returncode != 0:
        print(f"  FAILED (exit {result.returncode})")
        if result.stderr:
            print(f"  stderr: {result.stderr[-500:]}")
        raise subprocess.CalledProcessError(result.returncode, cmd)
    return result


def apply_patch(nutshell_path: Path, bug_id: str) -> bool:
    """Apply the patch for a given bug. Returns True on success."""
    patch_file = nutshell_path / "patch" / f"{bug_id}.patch"
    if not patch_file.exists():
        # Try with full name pattern
        patches = list((nutshell_path / "patch").glob(f"{bug_id}*.patch"))
        if not patches:
            print(f"  No patch file found for {bug_id}")
            return False
        patch_file = patches[0]

    # First, clean any previous patches
    run_cmd(["git", "checkout", "--", "."], cwd=nutshell_path, check=False)

    # Apply the patch
    result = run_cmd(
        ["git", "apply", str(patch_file)],
        cwd=nutshell_path, check=False
    )
    if result.returncode != 0:
        print(f"  Patch apply failed, trying with -p1...")
        result = run_cmd(
            ["git", "apply", "-p1", str(patch_file)],
            cwd=nutshell_path, check=False
        )
    return result.returncode == 0


def build_nutshell(nutshell_path: Path) -> bool:
    """Build NutShell: first generate Verilog, then build emu with FST trace."""
    print("  Step 1/2: Generating Verilog (Chisel → SystemVerilog)...")
    # make verilog generates split .sv files in build/rtl/
    # This produces proper modular RTL (NutCore.sv ~1.6K lines, not 34K)
    result = subprocess.run(
        ["make", "verilog"],
        cwd=nutshell_path,
        capture_output=True, text=True, timeout=600  # 10 min for Chisel
    )
    if result.returncode != 0:
        print(f"  Verilog generation failed: {result.stderr[-500:]}")
        return False

    print("  Step 2/2: Building emu (Verilator + FST)...")
    env = os.environ.copy()
    env["EMU_TRACE"] = "fst"
    env["RTL_SUFFIX"] = "sv"

    result = subprocess.run(
        ["make", "emu", "RTL_SUFFIX=sv"],
        cwd=nutshell_path, env=env,
        capture_output=True, text=True, timeout=1800  # 30 min
    )
    if result.returncode != 0:
        print(f"  Build failed: {result.stderr[-500:]}")
        return False
    return True


def assemble_testcase(nutshell_path: Path, bug_id: str, output_bin: Path) -> bool:
    """Assemble the test case for a given bug into a binary."""
    case_dir = nutshell_path / "case"
    asm_file = case_dir / f"{bug_id}.S"

    if not asm_file.exists():
        # Try matching with glob
        matches = list(case_dir.glob(f"{bug_id}*.S"))
        if not matches:
            print(f"  No test case found for {bug_id}")
            return False
        asm_file = matches[0]

    # Use riscv64-unknown-elf-gcc or riscv64-linux-gnu-gcc to assemble
    gcc = shutil.which("riscv64-unknown-elf-gcc") or shutil.which("riscv64-linux-gnu-gcc")
    if not gcc:
        print("  No RISC-V cross-compiler found")
        return False

    result = run_cmd([
        gcc, "-nostdlib", "-nostartfiles", "-T", str(case_dir / "link.ld"),
        "-o", str(output_bin), str(asm_file)
    ], check=False)

    if result.returncode != 0:
        # Try without linker script
        result = run_cmd([
            gcc, "-nostdlib", "-nostartfiles", "-static",
            "-o", str(output_bin), str(asm_file)
        ], check=False)

    return result.returncode == 0


def run_simulation(nutshell_path: Path, test_bin: Path, ref_so: Path,
                   emu_log_path: Path) -> bool:
    """Run NutShell emu with DiffTest and capture output."""
    emu_bin = nutshell_path / "build" / "emu"
    if not emu_bin.exists():
        print(f"  emu binary not found at {emu_bin}")
        return False

    # Create a temp FST path
    fst_path = emu_log_path.parent / "sim.fst"

    cmd = [
        str(emu_bin),
        "-i", str(test_bin),
        "--diff", str(ref_so),
        "--dump-commit-trace",
        "--dump-wave",
        f"--wave-path={fst_path}",
    ]

    print(f"  Running simulation...")
    result = subprocess.run(
        cmd, cwd=nutshell_path,
        capture_output=True, text=True, timeout=300  # 5 min timeout
    )

    # Save output (both stdout and stderr)
    with open(emu_log_path, 'w') as f:
        f.write("=== STDOUT ===\n")
        f.write(result.stdout)
        f.write("\n=== STDERR ===\n")
        f.write(result.stderr)

    # Check if ABORT happened (expected for buggy design)
    output = result.stdout + result.stderr
    if "ABORT" not in output:
        print("  Warning: No ABORT in emu output — bug may not have triggered")

    return True


def probe_start_time(wave_path: Path, start_scope: str, start_sig: str,
                     localizer: Path, project_path: str, include_paths: str,
                     signal_name_map: str) -> Optional[int]:
    """
    Probe the FST waveform to find the exact start_time.

    Strategy: Run sv_analysis with start_time=0 and a large time_bound,
    then parse the trace output to find when start_scope/start_sig first
    shows an error value.
    """
    # For now, return None — the probe logic requires understanding
    # sv_analysis's output format, which varies.
    # The user will need to manually specify start_time or use the
    # interactive probing approach.
    return None


def run_sv_analysis(cfg, work_dir: Path, test_info: dict,
                    fst_path: Path, result_dir: Path) -> bool:
    """Run sv_analysis for bug localization."""
    # Ensure signal_name_map.json exists
    signal_map = Path(cfg.bluesfl_path) / "signal_name_map.json"

    # Empty rm_params if not available
    rm_params = work_dir / "rm_params.tree.json"
    if not rm_params.exists():
        rm_params.write_text("{}")

    # Coverage path (may not exist for NutShell, use empty dir)
    coverage_dir = work_dir / "coverage"
    coverage_dir.mkdir(exist_ok=True)

    localizer_abs = Path(cfg.localizer).resolve()
    cmd = [
        str(localizer_abs),
        f"--bug-id={test_info['bug_id']}",
        f"--agent-type={cfg.agent_type}",
        f"--model={cfg.model}",
        f"--project-path={Path(cfg.nutshell_path).resolve()}/build/rtl",
        f"--include-paths={Path(cfg.nutshell_path).resolve()}/build/rtl,{Path(cfg.nutshell_path).resolve()}/build/generated-src",
        f"--rm-params-path={str(rm_params.resolve())}",
        f"--coverage-path={str(coverage_dir.resolve())}",
        f"--wave-path={str(fst_path.resolve())}",
        f"--top-module={TOP_MODULE}",
        f"--top-scope={TOP_SCOPE}",
        f"--start-scope={test_info['start_scope']}",
        f"--start-sig={test_info['start_sig']}",
        f"--start-time={test_info['start_time']}",
        f"--time-bound={test_info['time_bound']}",
        f"--time-step={TIME_STEP}",
        f"--output-path={str(result_dir.resolve())}",
        f"--vote-top-k={cfg.vote_top_k}",
        f"--vote-total={cfg.vote_total}",
        f"--test-info={test_info['test_info']}",
    ]

    if cfg.env_file:
        cmd.append(f"--dot-env={cfg.env_file}")

    if signal_map.exists():
        # sv_analysis doesn't have a direct --signal-map flag,
        # but it reads signal_name_map.json from project_path
        # So we copy it to the work directory
        shutil.copy2(signal_map, work_dir / "signal_name_map.json")

    # Save the command for reproducibility
    with open(work_dir / "boot_sv_analysis.sh", 'w') as f:
        f.write(' '.join(cmd))

    print(f"  Running sv_analysis...")
    result = run_cmd(cmd, cwd=work_dir, check=False, timeout=None)

    # Save output
    with open(work_dir / "sv_analysis_output.log", 'w') as f:
        f.write("STDOUT:\n")
        f.write(result.stdout if result.stdout else "")
        f.write("\nSTDERR:\n")
        f.write(result.stderr if result.stderr else "")

    return result.returncode == 0


def process_bug(cfg, bug_id: str) -> bool:
    """Process a single bug through the full pipeline."""
    nutshell_path = Path(cfg.nutshell_path)
    output_dir = Path(cfg.output_dir) / bug_id
    output_dir.mkdir(parents=True, exist_ok=True)

    print(f"\n{'='*60}")
    print(f"Processing bug: {bug_id}")
    print(f"{'='*60}")

    # Step 1: Apply patch and build
    if not cfg.skip_build:
        print("\n[Step 1] Applying patch and building...")
        if not apply_patch(nutshell_path, bug_id):
            return False
        if not build_nutshell(nutshell_path):
            return False

    # Step 2: Assemble test case and run simulation
    if not cfg.skip_sim:
        print("\n[Step 2] Running simulation...")
        test_bin = output_dir / f"{bug_id}.bin"

        if not cfg.skip_asm:
            if not assemble_testcase(nutshell_path, bug_id, test_bin):
                # Try using a pre-built binary if assembly fails
                prebuilt = nutshell_path / "build" / f"{bug_id}.bin"
                if prebuilt.exists():
                    test_bin = prebuilt
                else:
                    print("  Failed to assemble test case")
                    return False

        ref_so = Path(cfg.ref_so) if cfg.ref_so else nutshell_path / "build" / "riscv64-nemu-interpreter-so"
        emu_log = output_dir / "emu_output.log"

        if not run_simulation(nutshell_path, test_bin, ref_so, emu_log):
            return False

        # Copy FST to output
        fst_src = nutshell_path / "sim.fst"
        fst_dst = output_dir / "sim.fst"
        if fst_src.exists():
            shutil.copy2(fst_src, fst_dst)
    else:
        emu_log = output_dir / "emu_output.log"
        fst_dst = output_dir / "sim.fst"

    # Step 3: Generate test_info.json
    print("\n[Step 3] Generating test_info.json...")
    test_info_path = output_dir / "test_info.json"

    test_info_cmd = [
        sys.executable,
        str(Path(__file__).parent / "nutshell_test_analysis.py"),
        "--emu-log", str(emu_log),
        "--bug-id", bug_id,
        "--output", str(test_info_path),
    ]

    # Add overrides if provided
    if cfg.start_time is not None:
        test_info_cmd.extend(["--start-time", str(cfg.start_time)])
    if cfg.start_scope:
        test_info_cmd.extend(["--start-scope", cfg.start_scope])
    if cfg.start_sig:
        test_info_cmd.extend(["--start-sig", cfg.start_sig])

    result = run_cmd(test_info_cmd, check=False)
    if result.returncode != 0:
        print("  Failed to generate test_info.json")
        return False

    # Load generated test_info
    with open(test_info_path) as f:
        test_info = json.load(f)

    # If start_time is 0 (placeholder), try to use a provided value
    if test_info['start_time'] == 0 and cfg.start_time is not None:
        test_info['start_time'] = cfg.start_time
        test_info['time_bound'] = max(cfg.start_time - 30, 0)
        test_info['start_time_probed'] = True
        with open(test_info_path, 'w') as f:
            json.dump(test_info, f, indent=2)

    print(f"  start_time={test_info['start_time']}, time_bound={test_info['time_bound']}")

    if test_info['start_time'] == 0:
        print("  WARNING: start_time is 0 — need manual specification via --start-time")
        if not cfg.skip_analysis:
            return False

    # Step 4: Run sv_analysis
    if not cfg.skip_analysis:
        print("\n[Step 4] Running sv_analysis...")

        # Create numbered result directory
        result_prefix = cfg.prefix or "llm"
        max_num = 0
        for d in output_dir.glob(f"{result_prefix}_*"):
            m = re.match(rf"{result_prefix}_(\d+)", d.name)
            if m:
                max_num = max(max_num, int(m.group(1)))
        result_dir = output_dir / f"{result_prefix}_{max_num + 1}"
        result_dir.mkdir(parents=True, exist_ok=True)

        if not fst_dst.exists():
            print(f"  FST file not found: {fst_dst}")
            return False

        cfg.bluesfl_path = str(Path(__file__).parent.parent)
        success = run_sv_analysis(cfg, output_dir, test_info, fst_dst, result_dir)

        if success:
            print(f"  Results saved to {result_dir}")
        else:
            print(f"  sv_analysis failed (check logs)")

    # Cleanup: restore NutShell source
    if not cfg.skip_build:
        run_cmd(["git", "checkout", "--", "."], cwd=nutshell_path, check=False)

    return True


def main():
    parser = ArgumentParser(description="NutShell BluesFL Pipeline Runner")

    # Paths
    parser.add_argument("--nutshell-path", "-n", required=True,
                        help="Path to nutshell-sbfl repository")
    parser.add_argument("--localizer", "-l", required=True,
                        help="Path to sv_analysis executable")
    parser.add_argument("--output-dir", "-o", default="./e2e_results",
                        help="Output directory for results")
    parser.add_argument("--env-file", "-e", default=None,
                        help="Path to .env file for LLM API keys")

    # Bug selection
    parser.add_argument("--bug", "-b", nargs="+", default=None,
                        help="Bug IDs to process (e.g., U6 U1 M1). Default: all.")
    parser.add_argument("--bugs-file", default=None,
                        help="File with bug IDs, one per line")

    # LLM config
    parser.add_argument("--agent-type", default="open-ai",
                        choices=["open-ai", "claude", "ollama"])
    parser.add_argument("--model", "-m", default="gpt-4o-mini")
    parser.add_argument("--vote-total", type=int, default=2)
    parser.add_argument("--vote-top-k", type=int, default=1)
    parser.add_argument("--prefix", default="llm",
                        help="Result directory prefix")

    # Overrides
    parser.add_argument("--start-time", type=int, default=None,
                        help="Override start_time (FST time unit)")
    parser.add_argument("--start-scope", default=None)
    parser.add_argument("--start-sig", default=None)
    parser.add_argument("--ref-so", default=None,
                        help="Path to NEMU .so reference implementation")

    # Skip flags
    parser.add_argument("--skip-build", action="store_true",
                        help="Skip patch+build step")
    parser.add_argument("--skip-asm", action="store_true",
                        help="Skip test case assembly")
    parser.add_argument("--skip-sim", action="store_true",
                        help="Skip simulation step")
    parser.add_argument("--skip-analysis", action="store_true",
                        help="Skip sv_analysis step (only generate test_info)")

    cfg = parser.parse_args()

    # Determine bug list
    bugs = cfg.bug
    if bugs is None and cfg.bugs_file:
        with open(cfg.bugs_file) as f:
            bugs = [line.strip() for line in f if line.strip()]
    if bugs is None:
        # Default: all bugs with patches
        patch_dir = Path(cfg.nutshell_path) / "patch"
        bugs = sorted([p.stem for p in patch_dir.glob("*.patch")])

    print(f"Processing {len(bugs)} bugs: {bugs}")
    print(f"Output directory: {cfg.output_dir}")

    results = {}
    for bug_id in bugs:
        start_time = time.time()
        try:
            success = process_bug(cfg, bug_id)
            elapsed = time.time() - start_time
            results[bug_id] = {"success": success, "elapsed": elapsed}
            status = "OK" if success else "FAILED"
            print(f"\n  [{status}] {bug_id} ({elapsed:.1f}s)")
        except Exception as e:
            elapsed = time.time() - start_time
            results[bug_id] = {"success": False, "error": str(e), "elapsed": elapsed}
            print(f"\n  [ERROR] {bug_id}: {e}")

    # Summary
    print(f"\n{'='*60}")
    print("Summary:")
    ok = sum(1 for r in results.values() if r["success"])
    print(f"  {ok}/{len(results)} succeeded")
    for bid, r in results.items():
        status = "OK" if r["success"] else "FAIL"
        print(f"  [{status}] {bid} ({r['elapsed']:.1f}s)")

    # Save summary
    summary_path = Path(cfg.output_dir) / "pipeline_summary.json"
    with open(summary_path, 'w') as f:
        json.dump(results, f, indent=2, default=str)
    print(f"\nSummary saved to {summary_path}")


if __name__ == "__main__":
    main()
