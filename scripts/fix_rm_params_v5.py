#!/usr/bin/env python3
"""
fix_rm_params_v5.py — 把 Verilator 5.x 输出的 _009_param.tree.json 转成
BluesFL master 期望的 rm_params.tree.json (兼容 Verilator 4.x 的 dead 字段)。

背景
----
BluesFL `src/coverage/param.rs` 的 parse_module 用 `?` 处理 dead 字段:
    let dead = map.get("dead").and_then(|d| d.as_bool())?;
    (!dead).then(|| (origin_name.to_string(), coverages))

Verilator 4.x 对每个 MODULE 都写显式 dead 字段 (alive=false, dead=true)。
Verilator 5.x 只对 dead module 写 dead: true, 对 alive module 完全省略字段。
→ `?` 在字段缺失时返回 None → 整个 module 被丢弃 → ParameterCoverageReport 变空
→ CompositeCoverageTracker 对 Assign block 全报 not covered。

修复
----
后处理: 对每个 MODULE, 若 dead 字段缺失, 补 `"dead": false`。
保留 Verilator 5.x 对 dead module 的 dead: true 标记, 行为等价于 Verilator 4.x。

用法
----
    python3 fix_rm_params_v5.py \
        --input  build/.../sim-verilator/Vibex_simple_system_009_param.tree.json \
        --output build/.../sim-verilator/rm_params.tree.json
"""

import argparse
import json
import sys
from pathlib import Path


def fix_tree(data: dict) -> tuple[int, int]:
    """in-place 给所有缺失 dead 字段的 MODULE 补 dead: false。

    Returns: (total_modules, fixed_count)
    """
    modules = data.get("modulesp", [])
    total = 0
    fixed = 0
    for mod in modules:
        if not isinstance(mod, dict):
            continue
        if mod.get("type") != "MODULE":
            continue
        total += 1
        if "dead" not in mod:
            mod["dead"] = False
            fixed += 1
    return total, fixed


def main():
    parser = argparse.ArgumentParser(
        description="Post-process Verilator 5.x _009_param.tree.json "
                    "into BluesFL-compatible rm_params.tree.json"
    )
    parser.add_argument("--input", "-i", required=True,
                        help="Verilator 5.x _009_param.tree.json path")
    parser.add_argument("--output", "-o", required=True,
                        help="output rm_params.tree.json path")
    args = parser.parse_args()

    in_path = Path(args.input)
    out_path = Path(args.output)

    if not in_path.exists():
        print(f"ERROR: input not found: {in_path}", file=sys.stderr)
        sys.exit(1)

    print(f"[fix_rm_params_v5] reading {in_path}")
    with in_path.open("r") as f:
        data = json.load(f)

    total, fixed = fix_tree(data)

    out_path.parent.mkdir(parents=True, exist_ok=True)
    # BluesFL 用 serde_json 解析, 默认 compact format 即可
    with out_path.open("w") as f:
        json.dump(data, f, separators=(",", ":"))

    print(f"[fix_rm_params_v5] total MODULE: {total}, "
          f"missing-dead patched: {fixed}, dead-marked: {total - fixed}")
    print(f"[fix_rm_params_v5] wrote {out_path}")


if __name__ == "__main__":
    main()
