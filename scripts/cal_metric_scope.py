#!/usr/bin/env python3
"""
Top-K evaluator that matches by scope_name (not bid).

Oracle bids come from cpufuzz mutator's internal numbering, which differs from
BluesFL BlockManager's bid assignment. Since the oracle's scope_name is reliable
(instances under the same ibex_alu / ibex_decoder / etc.), we treat ALL our
blocks that fall under the oracle's scope_name as "oracle blocks" — any of them
appearing in Top-K counts as a hit.

Usage:
    python3 cal_metric_scope.py \\
        --built-root /home/yuan/dataset(1)/built \\
        --prefix llm_v5_topk \\
        --latest
"""
import argparse
import json
import re
from pathlib import Path


def find_latest_res_folder(case_dir: Path, prefix: str) -> Path | None:
    pat = re.compile(rf"{re.escape(prefix)}_(\d+)$")
    candidates = []
    for d in case_dir.iterdir():
        if not d.is_dir():
            continue
        m = pat.match(d.name)
        if m:
            candidates.append((int(m.group(1)), d))
    if not candidates:
        return None
    candidates.sort()
    return candidates[-1][1]


def get_oracle_bids_for_scope(blocks: list, oracle_scope: str) -> set[int]:
    """All our bids that live under the oracle's hierarchical scope."""
    result = set()
    for b in blocks:
        scope = b.get("scope", "")
        # Exact match OR scope is a child instance (e.g. alu_i.gen_X)
        if scope == oracle_scope or scope.startswith(oracle_scope + "."):
            result.add(b["bid"])
    return result


def get_oracle_bids_for_module(blocks: list, oracle_module: str) -> set[int]:
    """All our bids whose scope's instance name matches the module."""
    # module_name in oracle_info.json is the *definition* name (e.g. ibex_alu);
    # the scope ends with `<inst_name>_i` which often equals the module name's
    # snake_case form. We match the last scope segment containing the module.
    result = set()
    for b in blocks:
        scope = b.get("scope", "")
        # Naive heuristic: oracle_module appears as a substring of last segment
        last = scope.split(".")[-1] if scope else ""
        if oracle_module in last or oracle_module.replace("ibex_", "") in last:
            result.add(b["bid"])
    return result


def evaluate_case(case_dir: Path, prefix: str, use_scope: bool = True) -> dict | None:
    case_id = case_dir.name  # dataset_0_3
    # Locate wkdir (same name nested inside)
    wkdir = case_dir / case_id
    if not wkdir.exists():
        return None

    # Oracle: prefer scope_name match
    # cpufuzz mutator nests oracle_info.json at dataset/dataset_X/<N>/oracle_info.json
    # case_id is like "dataset_1_9" → split into (dataset_name="dataset_1", case_num="9")
    parts = case_id.rsplit("_", 1)
    dataset_name, case_num = parts[0], parts[1]  # "dataset_1", "9"
    oracle_path = wkdir / "dataset" / dataset_name / case_num / "oracle_info.json"
    if not oracle_path.exists():
        return None
    oracle = json.load(open(oracle_path))

    # Our blocks.json
    blocks_path = wkdir / "blocks.json"
    if not blocks_path.exists():
        return None
    blocks = json.load(open(blocks_path))

    # Resolve oracle to our bid space
    if use_scope and oracle.get("scope_name"):
        oracle_bids = get_oracle_bids_for_scope(blocks, oracle["scope_name"])
        match_mode = "scope"
    else:
        oracle_bids = get_oracle_bids_for_module(blocks, oracle["module_name"])
        match_mode = "module"

    # Latest result folder
    res_dir = find_latest_res_folder(case_dir, prefix)
    if res_dir is None:
        return None

    # Load choices (LLM-ranked)
    loc_file = res_dir / f"llm_loc_results_{case_id}.json"
    if not loc_file.exists():
        return None
    loc = json.load(open(loc_file))
    choices = sorted(loc.get("choices", []),
                     key=lambda c: -(c.get("score") or 0))

    # Load trace (BFS path)
    trace_file = res_dir / "trace.json"
    trace = json.load(open(trace_file)) if trace_file.exists() else []

    # Top-K via choices score
    top1_hit_choices = any(c.get("block_id") in oracle_bids for c in choices[:1])
    top5_hit_choices = any(c.get("block_id") in oracle_bids for c in choices[:5])
    top10_hit_choices = any(c.get("block_id") in oracle_bids for c in choices[:10])
    any_hit_choices = any(c.get("block_id") in oracle_bids for c in choices)

    # Top-K via trace position (paper Python cal_metric.py style)
    trace_bids = [t["bid"] for t in trace]
    top1_hit_trace = any(b in oracle_bids for b in trace_bids[-1:])
    top5_hit_trace = any(b in oracle_bids for b in trace_bids[-5:])
    top10_hit_trace = any(b in oracle_bids for b in trace_bids[-10:])
    any_hit_trace = any(b in oracle_bids for b in trace_bids)

    # Module-level hit (regardless of bid)
    choice_modules = {c.get("module_name") for c in choices}
    module_hit = oracle["module_name"] in choice_modules

    return {
        "case_id": case_id,
        "oracle_module": oracle["module_name"],
        "oracle_scope": oracle.get("scope_name", ""),
        "match_mode": match_mode,
        "oracle_bid_pool_size": len(oracle_bids),
        "choices_count": len(choices),
        "trace_count": len(trace),
        "top1_choices": top1_hit_choices,
        "top5_choices": top5_hit_choices,
        "top10_choices": top10_hit_choices,
        "any_choices": any_hit_choices,
        "top1_trace": top1_hit_trace,
        "top5_trace": top5_hit_trace,
        "top10_trace": top10_hit_trace,
        "any_trace": any_hit_trace,
        "module_hit": module_hit,
        "top1_module": choices[0].get("module_name") if choices else None,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--built-root", required=True,
                    help="root containing dataset_X_N/ folders")
    ap.add_argument("--prefix", default="llm_v5_topk")
    ap.add_argument("--latest", action="store_true", default=True)
    ap.add_argument("--by-module", action="store_true",
                    help="match by module_name instead of scope_name")
    args = ap.parse_args()

    root = Path(args.built_root)
    use_scope = not args.by_module

    case_dirs = sorted([p for p in root.glob("dataset_*_*") if p.is_dir()])
    print(f"{'case':<18} {'oracle_mod':<18} {'top1_mod':<18} "
          f"{'choices/trace':<14} {'T1ch T5ch T10ch':<18} "
          f"{'T1tr T5tr T10tr':<18} {'mod_hit'}")
    print("-" * 130)

    agg = {"top1_choices": 0, "top5_choices": 0, "top10_choices": 0,
           "top1_trace": 0, "top5_trace": 0, "top10_trace": 0,
           "module_hit": 0, "total": 0}

    for cd in case_dirs:
        r = evaluate_case(cd, args.prefix, use_scope=use_scope)
        if r is None:
            print(f"{cd.name:<18} (skipped — missing oracle/results)")
            continue
        agg["total"] += 1
        for k in ["top1_choices", "top5_choices", "top10_choices",
                  "top1_trace", "top5_trace", "top10_trace", "module_hit"]:
            agg[k] += int(r[k])

        def yn(b): return "Y" if b else "."
        print(f"{r['case_id']:<18} {r['oracle_module']:<18} {r['top1_module'] or '-':<18} "
              f"{r['choices_count']:>3}/{r['trace_count']:<3} ({r['oracle_bid_pool_size']:>3} bids)  "
              f"{yn(r['top1_choices'])}    {yn(r['top5_choices'])}    {yn(r['top10_choices'])}      "
              f"{yn(r['top1_trace'])}    {yn(r['top5_trace'])}    {yn(r['top10_trace'])}      "
              f"{yn(r['module_hit'])}")

    n = max(agg["total"], 1)
    print("-" * 130)
    print(f"TOTAL {agg['total']} cases")
    print(f"  By choices (LLM reranker):  "
          f"Top-1={agg['top1_choices']}/{agg['total']} ({agg['top1_choices']/n*100:.1f}%)  "
          f"Top-5={agg['top5_choices']}/{agg['total']} ({agg['top5_choices']/n*100:.1f}%)  "
          f"Top-10={agg['top10_choices']}/{agg['total']} ({agg['top10_choices']/n*100:.1f}%)")
    print(f"  By trace   (BFS position):  "
          f"Top-1={agg['top1_trace']}/{agg['total']} ({agg['top1_trace']/n*100:.1f}%)  "
          f"Top-5={agg['top5_trace']}/{agg['total']} ({agg['top5_trace']/n*100:.1f}%)  "
          f"Top-10={agg['top10_trace']}/{agg['total']} ({agg['top10_trace']/n*100:.1f}%)")
    print(f"  Module-level hit rate:      "
          f"{agg['module_hit']}/{agg['total']} ({agg['module_hit']/n*100:.1f}%)")


if __name__ == "__main__":
    main()
