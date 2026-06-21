#!/usr/bin/env python3
"""
apply_and_build.py — 编译阶段 (Phase A)

对 dataset(1)/dataset/dataset_X/ 下的每个 case:
  1. (可选) 把 ~/ibex 还原成 golden (git checkout + 删 .orig)
  2. 拷贝 golden ~/ibex 到 wkdir (排除 .git / build)
  3. 用 case 里的 mutated .sv 覆盖 wkdir/rtl/ 对应文件
  4. fusesoc 编译 cosim, 生成 Vibex_simple_system 二进制
  5. 写 build_status.json

注意: 本脚本只编译, 不跑 cosim。cosim 在 Phase B (ibex_fl_run_all.py) 里执行。

输出结构 (匹配 ibex_fl_run_all.py 期望的 cur_dir.name == cur_dir.parent.name):
  <output>/<case_id>/<case_id>/
      rtl/                          # mutated RTL 树
      vendor/...
      examples/.../coremark.elf
      fusesoc_build.log
      build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator/
          Vibex_simple_system      # 编译产物, 供 Phase B 跑 cosim
  <output>/<case_id>/build_status.json

用法 (编译整个 dataset_0):
  python3 scripts/apply_and_build.py \
      --dataset-root "/home/yuan/dataset(1)/dataset" \
      --dataset dataset_0 \
      --restore-ibex

用法 (只编译指定 case):
  python3 scripts/apply_and_build.py --only dataset_0_0 dataset_0_1
"""

import argparse
import json
import logging
import os
import shutil
import subprocess
import time
from datetime import datetime
from pathlib import Path

logger = logging.getLogger("apply_and_build")


def setup_logging():
    logs_dir = Path("./logs")
    logs_dir.mkdir(exist_ok=True)
    ts = datetime.now().strftime("%Y%m%d_%H%M%S")
    log_file = logs_dir / f"apply_and_build_{ts}.log"
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(message)s",
        handlers=[logging.FileHandler(log_file), logging.StreamHandler()],
    )
    logger.info(f"log file: {log_file}")


# ---------- Step 0: 还原 ~/ibex 成 golden ----------
def restore_ibex_golden(ibex_src: Path):
    """git checkout -- rtl/ 并删除 rtl/*.orig 备份文件"""
    logger.info(f"Restoring {ibex_src} to golden state...")
    if not (ibex_src / ".git").exists():
        logger.warning(f"  {ibex_src} is not a git repo, skip restore")
        return

    # 丢弃 rtl/ 的所有修改
    try:
        subprocess.run(
            ["git", "checkout", "--", "rtl/"],
            cwd=ibex_src, check=True, capture_output=True, text=True,
        )
        logger.info("  git checkout -- rtl/ done")
    except subprocess.CalledProcessError as e:
        logger.error(f"  git checkout failed: {e.stderr}")
        return

    # 删除 .orig 备份
    orig_files = list((ibex_src / "rtl").glob("*.orig"))
    for f in orig_files:
        f.unlink()
    if orig_files:
        logger.info(f"  removed {len(orig_files)} .orig files")


# ---------- Step 1: 拷贝 golden ibex 到 wkdir ----------
def copy_golden_to_wkdir(ibex_src: Path, wkdir: Path):
    """cp -r ibex wkdir, 排除 .git / build"""
    if wkdir.exists():
        logger.info(f"  removing existing wkdir: {wkdir}")
        shutil.rmtree(wkdir)
    wkdir.parent.mkdir(parents=True, exist_ok=True)

    logger.info(f"  copying {ibex_src} -> {wkdir} (excluding .git, build)")
    ignore = shutil.ignore_patterns(".git", "build", "*.orig", "build_*")
    shutil.copytree(ibex_src, wkdir, ignore=ignore)


# ---------- Step 2: 应用 mutated .sv ----------
def apply_mutated_sv(case_dir: Path, wkdir: Path):
    """把 case_dir 下所有 *.sv (非 .sv.diff) 覆盖到 wkdir/rtl/"""
    rtl_dir = wkdir / "rtl"
    applied = []
    for sv_file in case_dir.glob("*.sv"):
        dst = rtl_dir / sv_file.name
        shutil.copy2(sv_file, dst)
        applied.append(sv_file.name)
    logger.info(f"  applied mutated RTL: {applied}")
    return applied


# ---------- Step 3: fusesoc 编译 ----------
def fusesoc_build(wkdir: Path, spike_prefix: Path) -> bool:
    """运行 fusesoc 编译 cosim。返回是否成功。"""
    logger.info(f"  running fusesoc build (this may take 3-5 min)...")
    cmd = [
        "fusesoc", "--cores-root=.", "run", "--target=sim",
        "--setup", "--build",
        "lowrisc:ibex:ibex_simple_system_cosim",
        "--RV32E=0", "--RV32M=ibex_pkg::RV32MFast",
    ]
    # Spike cosim 需要 PKG_CONFIG_PATH / LD_LIBRARY_PATH
    env = os.environ.copy()
    pkg_dir = spike_prefix / "lib" / "pkgconfig"
    lib_dir = spike_prefix / "lib"
    if pkg_dir.exists():
        env["PKG_CONFIG_PATH"] = f"{pkg_dir}:{env.get('PKG_CONFIG_PATH', '')}"
    if lib_dir.exists():
        env["LD_LIBRARY_PATH"] = f"{lib_dir}:{env.get('LD_LIBRARY_PATH', '')}"
    log_path = wkdir / "fusesoc_build.log"
    with open(log_path, "w") as f:
        try:
            result = subprocess.run(
                cmd, cwd=wkdir, env=env, stdout=f, stderr=subprocess.STDOUT,
                text=True, check=False,
            )
        except FileNotFoundError:
            logger.error("  fusesoc not found in PATH")
            return False
    if result.returncode != 0:
        logger.error(f"  fusesoc build FAILED (rc={result.returncode}), see {log_path}")
        return False

    sim_dir = (wkdir / "build/lowrisc_ibex_ibex_simple_system_cosim_0/sim-verilator")
    binary = sim_dir / "Vibex_simple_system"
    if not binary.exists():
        logger.error(f"  build finished but binary not found: {binary}")
        return False
    logger.info(f"  fusesoc build OK, binary at {binary}")

    # Post-process Verilator 5.x _009_param.tree.json into rm_params.tree.json
    # (BluesFL param.rs expects every MODULE to have explicit `dead` field,
    #  Verilator 5.x omits it for alive modules → post-process patches it)
    fix_script = Path(__file__).parent / "fix_rm_params_v5.py"
    if fix_script.exists():
        # locate _009_param pass output (file name pattern: Vibex_simple_system_009_param.tree.json)
        candidates = list(sim_dir.glob("*_009_param.tree.json"))
        if candidates:
            src = candidates[0]
            dst = sim_dir / "rm_params.tree.json"
            logger.info(f"  post-processing {src.name} → rm_params.tree.json")
            try:
                subprocess.run(
                    ["python3", str(fix_script),
                     "--input", str(src), "--output", str(dst)],
                    check=True, capture_output=True, text=True,
                )
                logger.info(f"  rm_params.tree.json written to {dst}")
            except subprocess.CalledProcessError as e:
                logger.error(f"  fix_rm_params_v5 failed: {e.stderr.strip()}")
                # non-fatal: Phase B can still fall back to golden rm_params
        else:
            logger.warning(f"  no *_009_param.tree.json found in {sim_dir}, "
                           f"rm_params will fall back to golden")
    else:
        logger.warning(f"  fix_rm_params_v5.py not found at {fix_script}")

    return True


# ---------- 主流程 ----------
def process_case(case_id: str, case_dir: Path, ibex_src: Path,
                 output_root: Path, spike_prefix: Path):
    """处理单个 case (纯编译)。返回 status 字符串。"""
    logger.info(f"==== CASE {case_id} ({case_dir}) ====")

    wkdir = output_root / case_id / case_id
    status_file = wkdir.parent / "build_status.json"

    # 1. 拷贝 golden
    try:
        copy_golden_to_wkdir(ibex_src, wkdir)
    except Exception as e:
        logger.error(f"  copy failed: {e}")
        _write_status(status_file, case_id, "copy_fail")
        return "copy_fail"

    # 2. 应用 mutated
    try:
        apply_mutated_sv(case_dir, wkdir)
    except Exception as e:
        logger.error(f"  apply mutated failed: {e}")
        _write_status(status_file, case_id, "apply_fail")
        return "apply_fail"

    # 3. 编译
    t0 = time.time()
    ok = fusesoc_build(wkdir, spike_prefix)
    build_time = time.time() - t0
    if not ok:
        _write_status(status_file, case_id, "build_fail", build_time=build_time)
        return "build_fail"
    logger.info(f"  build took {build_time:.1f}s")

    _write_status(status_file, case_id, "built", build_time=build_time)
    return "built"


def _write_status(path: Path, case_id: str, status: str, **extra):
    data = {"case_id": case_id, "status": status,
            "timestamp": datetime.now().isoformat(), **extra}
    path.write_text(json.dumps(data, indent=2))


def collect_cases(dataset_root: Path, dataset_name: str):
    """
    返回 [(case_id, case_dir), ...]
    case_id 形如 'dataset_0_0'（dataset 名 + 子目录名，扁平化）
    """
    dataset_dir = dataset_root / dataset_name
    if not dataset_dir.exists():
        logger.error(f"dataset dir not found: {dataset_dir}")
        return []
    cases = []
    for sub in sorted(dataset_dir.iterdir()):
        if not sub.is_dir():
            continue
        if not sub.name.isdigit():
            continue
        # 至少有一个 .sv 文件
        if not list(sub.glob("*.sv")):
            continue
        case_id = f"{dataset_name}_{sub.name}"
        cases.append((case_id, sub))
    return cases


def main():
    parser = argparse.ArgumentParser(
        description="Phase A: apply mutated RTL + fusesoc build (no cosim)")
    parser.add_argument("--dataset-root", default="/home/yuan/dataset(1)/dataset",
                        help="dataset 根目录 (含 dataset_0, dataset_1, ...)")
    parser.add_argument("--dataset", default="dataset_0",
                        help="要处理的 dataset_X 名 (默认 dataset_0)")
    parser.add_argument("--ibex-src", default="/home/yuan/ibex",
                        help="golden ibex 源目录")
    parser.add_argument("--spike-prefix", default="/home/yuan/ibex-spike-cosim/install",
                        help="Spike cosim install 前缀 (含 lib/pkgconfig)")
    parser.add_argument("--output", default="/home/yuan/dataset(1)/built",
                        help="wkdir 输出根目录")
    parser.add_argument("--restore-ibex", action="store_true",
                        help="开始前 git checkout 还原 ibex-src")
    parser.add_argument("--only", nargs="+", default=None,
                        help="只处理指定 case_id (可多个, 如 --only dataset_0_0 dataset_0_1)")
    args = parser.parse_args()

    setup_logging()
    logger.info(f"args: {vars(args)}")

    ibex_src = Path(args.ibex_src).expanduser().resolve()
    spike_prefix = Path(args.spike_prefix).expanduser().resolve()
    output_root = Path(args.output).expanduser().resolve()
    dataset_root = Path(args.dataset_root).expanduser().resolve()

    if args.restore_ibex:
        restore_ibex_golden(ibex_src)

    cases = collect_cases(dataset_root, args.dataset)
    if not cases:
        logger.error("no cases found, exit")
        return

    if args.only:
        wanted = set(args.only)
        cases = [(cid, cdir) for cid, cdir in cases if cid in wanted]
        if not cases:
            logger.error(f"--only {args.only} not found")
            return

    logger.info(f"processing {len(cases)} cases in {args.dataset}")
    summary = {}
    for case_id, case_dir in cases:
        try:
            status = process_case(
                case_id, case_dir, ibex_src, output_root, spike_prefix,
            )
        except Exception as e:
            logger.exception(f"unexpected error on {case_id}: {e}")
            status = "exception"
        summary[case_id] = status
        logger.info(f"  -> {case_id}: {status}\n")

    logger.info("==== SUMMARY ====")
    for cid, st in summary.items():
        logger.info(f"  {cid}: {st}")


if __name__ == "__main__":
    main()
