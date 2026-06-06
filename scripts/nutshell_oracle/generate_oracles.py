#!/usr/bin/env python3
"""
NutShell bug oracle data for BluesFL metric calculation.

Each oracle maps a bug to its ground-truth localization:
- bid: the block ID (determined by running sv_analysis first, if known)
- module_name: the Verilog module name containing the bug
- scope_name: the FST scope path to the buggy module
- description: human-readable bug description
- bug_file: the Chisel source file that was patched
"""

import json
import sys
from pathlib import Path

# Scope prefix for NutShell (split-file RTL)
PFX = "TOP.SimTop.cpu.soc.nutcore.cpu.soc.nutcore"

ORACLES = {
    # A. Basic ISA & datapath
    "U6_jalr_bit0_not_cleared": {
        "bid": 5841,
        "module_name": "ALU",
        "scope_name": f"{PFX}.backend.exu.alu",
        "description": "JALR target bit0 not cleared",
        "bug_file": "src/main/scala/nutcore/backend/fu/ALU.scala",
    },
    "U1_addiw_signext": {
        "module_name": "ALU",
        "scope_name": f"{PFX}.backend.exu.alu",
        "description": "ADDIW sign-extension error (zero-extend instead)",
        "bug_file": "src/main/scala/nutcore/backend/fu/ALU.scala",
    },
    "U7_lb_lbu_ext_swap": {
        "module_name": "UnpipelinedLSU",
        "scope_name": f"{PFX}.backend.fu.UnpipelinedLSU",
        "description": "LB/LBU sign/zero extension swapped",
        "bug_file": "src/main/scala/nutcore/backend/fu/UnpipelinedLSU.scala",
    },
    "M1_div_by_zero": {
        "module_name": "MDU",
        "scope_name": f"{PFX}.backend.exu.mdu",
        "description": "DIV/REM divide-by-zero semantics error",
        "bug_file": "src/main/scala/nutcore/backend/fu/MDU.scala",
    },
    # B. Decode & illegal instruction
    "D1_sub_sra_decode": {
        "module_name": "RVI",
        "scope_name": f"{PFX}.isa.RVI",
        "description": "SUB/SRA decode confusion",
        "bug_file": "src/main/scala/nutcore/isa/RVI.scala",
    },
    "X1_fence_fencei_field_check": {
        "module_name": "RVZifencei",
        "scope_name": f"{PFX}.isa.RVZifencei",
        "description": "FENCE/FENCE.I reserved field check missing",
        "bug_file": "src/main/scala/nutcore/isa/RVZifencei.scala",
    },
    "X2_load_store_funct3_illegal": {
        "module_name": "RVI",
        "scope_name": f"{PFX}.isa.RVI",
        "description": "Illegal LOAD/STORE funct3 not trapped",
        "bug_file": "src/main/scala/nutcore/isa/RVI.scala",
    },
    "RE1_branch_misaligned": {
        "module_name": "BRU",
        "scope_name": f"{PFX}.backend.exu.bru",
        "description": "Conditional branch target alignment check missing",
        "bug_file": "src/main/scala/nutcore/backend/fu/BRU.scala",
    },
    # C. CSR / Trap / Privilege
    "P4_mepc_save_error": {
        "module_name": "CSR",
        "scope_name": f"{PFX}.backend.fu.CSR",
        "description": "mepc saved as pc+4 instead of pc on trap",
        "bug_file": "src/main/scala/nutcore/backend/fu/CSR.scala",
    },
    "P6_mret_privilege_mode": {
        "module_name": "CSR",
        "scope_name": f"{PFX}.backend.fu.CSR",
        "description": "MRET restores wrong privilege mode",
        "bug_file": "src/main/scala/nutcore/backend/fu/CSR.scala",
    },
    "NE1_mtval_high_bits": {
        "module_name": "CSR",
        "scope_name": f"{PFX}.backend.fu.CSR",
        "description": "mtval high bits not writable (sign-extended only)",
        "bug_file": "src/main/scala/nutcore/backend/fu/CSR.scala",
    },
    "NE3_xret_privilege_check": {
        "module_name": "CSR",
        "scope_name": f"{PFX}.backend.fu.CSR",
        "description": "SRET executable from U-mode without trap",
        "bug_file": "src/main/scala/nutcore/backend/fu/CSR.scala",
    },
    "X3_csr_privilege_access": {
        "module_name": "CSR",
        "scope_name": f"{PFX}.backend.fu.CSR",
        "description": "Privileged CSR access check missing",
        "bug_file": "src/main/scala/nutcore/backend/fu/CSR.scala",
    },
    # D. Exception & side-effect suppression
    "E1_precise_exception_writeback": {
        "module_name": "EXU",
        "scope_name": f"{PFX}.backend.seq.EXU",
        "description": "Younger instructions commit after exception",
        "bug_file": "src/main/scala/nutcore/backend/seq/EXU.scala",
    },
    "E2_load_exception_writeback": {
        "module_name": "EXU",
        "scope_name": f"{PFX}.backend.seq.EXU",
        "description": "Load fault still writes to destination register",
        "bug_file": "src/main/scala/nutcore/backend/seq/EXU.scala",
    },
    "RE2_store_misaligned_flush": {
        "module_name": "UnpipelinedLSU",
        "scope_name": f"{PFX}.backend.fu.UnpipelinedLSU",
        "description": "Store misaligned exception does not suppress write",
        "bug_file": "src/main/scala/nutcore/backend/fu/UnpipelinedLSU.scala",
    },
    "X4_exception_type_priority": {
        "module_name": "EXU",
        "scope_name": f"{PFX}.backend.seq.EXU",
        "description": "Exception type or priority error",
        "bug_file": "src/main/scala/nutcore/backend/seq/EXU.scala",
    },
    "X5_lr_sc_reservation": {
        "module_name": "UnpipelinedLSU",
        "scope_name": f"{PFX}.backend.fu.UnpipelinedLSU",
        "description": "LR/SC reservation or exception handling error",
        "bug_file": "src/main/scala/nutcore/backend/fu/UnpipelinedLSU.scala",
    },
    # E. Pipeline
    "C1_alu_forwarding_disabled": {
        "module_name": "ISU",
        "scope_name": f"{PFX}.backend.seq.ISU",
        "description": "ALU-ALU forwarding disabled",
        "bug_file": "src/main/scala/nutcore/backend/seq/ISU.scala",
    },
    "C3_branch_no_redirect": {
        "module_name": "ALU",
        "scope_name": f"{PFX}.backend.exu.alu",
        "description": "Branch taken but no redirect generated",
        "bug_file": "src/main/scala/nutcore/backend/fu/ALU.scala",
    },
    # F. MMU / TLB
    "PT4_pte_rw_legality_check": {
        "module_name": "EmbeddedTLB",
        "scope_name": f"{PFX}.mem.EmbeddedTLB",
        "description": "PTE R/W legality check missing",
        "bug_file": "src/main/scala/nutcore/mem/EmbeddedTLB.scala",
    },
    "PT9_superpage_mask_error": {
        "module_name": "EmbeddedTLB",
        "scope_name": f"{PFX}.mem.EmbeddedTLB",
        "description": "Superpage alignment mask error",
        "bug_file": "src/main/scala/nutcore/mem/EmbeddedTLB.scala",
    },
    "PT10_sfence_vma_flush_disabled": {
        "module_name": "EmbeddedTLB",
        "scope_name": f"{PFX}.mem.EmbeddedTLB",
        "description": "SFENCE.VMA does not flush TLB",
        "bug_file": "src/main/scala/nutcore/mem/EmbeddedTLB.scala",
    },
    # G. Performance counter
    "X6_minstret_count_error": {
        "module_name": "CSR",
        "scope_name": f"{PFX}.backend.fu.CSR",
        "description": "minstret count error for certain instructions",
        "bug_file": "src/main/scala/nutcore/backend/fu/CSR.scala",
    },
}


def generate_files(output_dir: str):
    """Generate oracle files in cal_metric-compatible directory structure."""
    out = Path(output_dir)
    out.mkdir(parents=True, exist_ok=True)

    for bug_id, oracle in ORACLES.items():
        # cal_metric expects: <oracle_root>/<bug_id>/oracle_info.json
        bug_dir = out / bug_id
        bug_dir.mkdir(exist_ok=True)
        oracle_path = bug_dir / "oracle_info.json"
        with open(oracle_path, 'w') as f:
            json.dump(oracle, f, indent=2)
        print(f"  {bug_id}/oracle_info.json")

    # Also generate flat files for convenience
    for bug_id, oracle in ORACLES.items():
        path = out / f"{bug_id}.json"
        with open(path, 'w') as f:
            json.dump(oracle, f, indent=2)

    print(f"\n  Total: {len(ORACLES)} oracle entries")
    print(f"  NOTE: bid field is 0 for bugs not yet analyzed.")
    print(f"  Run sv_analysis first, then update bid from trace output.")


if __name__ == "__main__":
    import argparse

    ap = argparse.ArgumentParser(description="NutShell oracle generator for BluesFL")
    ap.add_argument("bug_id", nargs="?", help="Print oracle for a specific bug")
    ap.add_argument("--update-bid", metavar="RESULTS_DIR",
                    help="Update bid fields from sv_analysis results directory "
                         "(searches for llm_loc_results_*.json to extract block_id)")
    args = ap.parse_args()

    if args.update_bid:
        results_dir = Path(args.update_bid)
        if not results_dir.exists():
            print(f"Results directory not found: {results_dir}", file=sys.stderr)
            sys.exit(1)

        updated = 0
        for result_file in results_dir.rglob("llm_loc_results_*.json"):
            try:
                with open(result_file) as f:
                    data = json.load(f)
            except (json.JSONDecodeError, OSError) as e:
                print(f"  Skipping {result_file}: {e}")
                continue

            choices = data.get("choices", [])
            if not choices:
                continue

            # Use the top-1 choice to find matching oracle
            top_choice = choices[0]
            bid = top_choice.get("block_id")
            module = top_choice.get("module_name")
            if not bid:
                continue

            # Find matching oracle by module_name
            for bug_id, oracle in ORACLES.items():
                if oracle.get("module_name") == module and "bid" not in oracle:
                    oracle["bid"] = bid
                    print(f"  Updated {bug_id}: bid={bid} (from {result_file.name})")
                    updated += 1
                    break

        if updated:
            output_dir = str(Path(__file__).parent)
            generate_files(output_dir)
            print(f"\n  Updated {updated} oracle(s)")
        else:
            print("  No new bid values to update")

    elif args.bug_id:
        bug_id = args.bug_id
        if bug_id in ORACLES:
            print(json.dumps(ORACLES[bug_id], indent=2))
        else:
            print(f"Unknown bug: {bug_id}", file=sys.stderr)
            sys.exit(1)
    else:
        output_dir = str(Path(__file__).parent)
        generate_files(output_dir)
