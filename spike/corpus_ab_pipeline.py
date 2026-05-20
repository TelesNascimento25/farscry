#!/usr/bin/env python3
import argparse
import json
import struct
import sys
from pathlib import Path

import zstandard as zstd

VASF_MAGIC = b"VASF"
_dctx = zstd.ZstdDecompressor()


def _decompress(data: bytes) -> bytes:
    try:
        return _dctx.stream_reader(data).read()
    except Exception:
        return b""


def read_session(path: Path) -> dict:
    try:
        with open(path, "rb") as f:
            if f.read(4) != VASF_MAGIC:
                return {}
            f.read(2)
            fc = struct.unpack("<I", f.read(4))[0]
            f.read(8 + 4)
            frames = []
            for _ in range(fc):
                sid = struct.unpack("<Q", f.read(8))[0]
                f.read(8)
                vl = struct.unpack("<I", f.read(4))[0]
                vd = _decompress(f.read(vl)) if vl > 0 else b""
                dl = struct.unpack("<I", f.read(4))[0]
                if dl > 0:
                    f.read(dl)
                frames.append((sid, vd.decode("utf-8", errors="replace")))
    except Exception:
        return {}

    markers = [i for i, (_, v) in enumerate(frames) if v.startswith("action_marker")]
    sf = ae = 0
    for midx in markers:
        before = next((frames[i][0] for i in range(midx - 1, -1, -1) if not frames[i][1].startswith("action_marker")), None)
        after = next((frames[i][0] for i in range(midx + 1, len(frames)) if not frames[i][1].startswith("action_marker")), None)
        if before is not None and after is not None:
            if before == after:
                sf += 1
            else:
                ae += 1

    terminal_type = ""
    for _, v in reversed(frames):
        if not v.startswith("action_marker"):
            for line in v.splitlines():
                if line.startswith("screen_type:"):
                    terminal_type = line.split(":", 1)[1].strip().strip('"').lower()
            break

    return {
        "fc": len(frames),
        "markers": len(markers),
        "sf": sf,
        "ae": ae,
        "total_actions": sf + ae,
        "terminal_type": terminal_type,
        "has_augment": len(markers) > 0,
    }


def load_corpus(directory: Path, failed_names: set[str]) -> list[dict]:
    sessions = []
    for vf in sorted(directory.glob("*.vasf")):
        s = read_session(vf)
        if not s:
            continue
        total = s["sf"] + s["ae"]
        high_sf = total >= 3 and s["sf"] / max(total, 1) > 0.5
        failed = vf.name in failed_names or s["terminal_type"] == "error" or high_sf
        sessions.append({"filename": vf.name, "failed": failed, **s})
    return sessions


def metrics(sessions: list[dict]) -> dict:
    n = len(sessions)
    n_failed = sum(1 for s in sessions if s["failed"])
    n_ok = n - n_failed
    total_actions = sum(s["total_actions"] for s in sessions)
    sf = sum(s["sf"] for s in sessions)
    ae = sum(s["ae"] for s in sessions)
    return {
        "n": n,
        "n_failed": n_failed,
        "n_ok": n_ok,
        "tcr": n_ok / n if n > 0 else 0.0,
        "tcr_pct": round(n_ok / n * 100, 1) if n > 0 else 0.0,
        "total_actions": total_actions,
        "sf": sf,
        "ae": ae,
        "aer": ae / total_actions if total_actions > 0 else 0.0,
        "aer_pct": round(ae / total_actions * 100, 1) if total_actions > 0 else 0.0,
        "sf_rate_pct": round(sf / total_actions * 100, 1) if total_actions > 0 else 0.0,
        "sessions_with_augment": sum(1 for s in sessions if s["has_augment"]),
        "sessions_0_actions": sum(1 for s in sessions if s["total_actions"] == 0),
    }


def simulate_run_a(sessions_b: list[dict]) -> list[dict]:
    simulated = []
    for s in sessions_b:
        sim = dict(s)
        sim["has_augment"] = False
        if s["sf"] > 0 and not s["failed"]:
            sim["failed"] = True
        simulated.append(sim)
    return simulated


def fisher_p(n_ok_a: int, n_a: int, n_ok_b: int, n_b: int) -> float:
    try:
        from scipy.stats import fisher_exact
        _, p = fisher_exact([[n_ok_a, n_a - n_ok_a], [n_ok_b, n_b - n_ok_b]], alternative="less")
        return float(p)
    except ImportError:
        return -1.0


def print_comparison(ma: dict, mb: dict, label_a: str, label_b: str):
    print()
    print(f"  {'metric':<30}  {'run_a':>10}  {'run_b':>10}  {'delta':>8}")
    print("  " + "-" * 58)

    def delta(a, b):
        d = b - a
        return f"{d:+.1f}%"

    print(f"  {'sessions':<30}  {ma['n']:>10}  {mb['n']:>10}")
    print(f"  {'TCR [primary]':<30}  {ma['tcr_pct']:>9.1f}%  {mb['tcr_pct']:>9.1f}%  {delta(ma['tcr_pct'], mb['tcr_pct']):>8}")
    print(f"  {'AER [diagnostic]':<30}  {ma['aer_pct']:>9.1f}%  {mb['aer_pct']:>9.1f}%  {delta(ma['aer_pct'], mb['aer_pct']):>8}")
    print(f"  {'SF rate':<30}  {ma['sf_rate_pct']:>9.1f}%  {mb['sf_rate_pct']:>9.1f}%  {delta(ma['sf_rate_pct'], mb['sf_rate_pct']):>8}")
    print(f"  {'total actions':<30}  {ma['total_actions']:>10}  {mb['total_actions']:>10}")
    print(f"  {'sessions w/ augment':<30}  {ma['sessions_with_augment']:>10}  {mb['sessions_with_augment']:>10}")
    print(f"  {'sessions 0 actions':<30}  {ma['sessions_0_actions']:>10}  {mb['sessions_0_actions']:>10}")

    tcr_delta = mb["tcr_pct"] - ma["tcr_pct"]
    p = fisher_p(ma["n_ok"], ma["n"], mb["n_ok"], mb["n"])
    print(f"\n  TCR delta: {tcr_delta:+.1f}%")
    if p >= 0:
        print(f"  Fisher p:  {p:.4f}  {'(p<0.05)' if p < 0.05 else '(not significant)'}")

    print()
    if tcr_delta > 5 and (p < 0.05 or p < 0):
        print("  VERDICT: farscry augment causally improved task completion.")
    elif tcr_delta > 5:
        print("  VERDICT: TCR improved but not statistically significant — scale up N.")
    elif tcr_delta > 0:
        print("  VERDICT: marginal improvement — not conclusive.")
    else:
        print("  VERDICT: no improvement detected.")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--run-a", type=Path, default=None)
    p.add_argument("--run-b", type=Path, required=True)
    p.add_argument("--failed-a", type=Path, default=None)
    p.add_argument("--failed-b", type=Path, default=None)
    p.add_argument("--simulate-run-a", action="store_true")
    p.add_argument("--output", type=Path, default=None)
    args = p.parse_args()

    failed_b: set[str] = set()
    if args.failed_b and args.failed_b.exists():
        failed_b = {l.strip() for l in args.failed_b.read_text().splitlines() if l.strip()}

    sessions_b = load_corpus(args.run_b, failed_b)
    if not sessions_b:
        print(f"no sessions in {args.run_b}", file=sys.stderr)
        sys.exit(1)
    mb = metrics(sessions_b)

    if args.simulate_run_a:
        sessions_a = simulate_run_a(sessions_b)
        label_a = "run_a (simulated)"
    elif args.run_a:
        failed_a: set[str] = set()
        if args.failed_a and args.failed_a.exists():
            failed_a = {l.strip() for l in args.failed_a.read_text().splitlines() if l.strip()}
        sessions_a = load_corpus(args.run_a, failed_a)
        if not sessions_a:
            print(f"no sessions in {args.run_a}", file=sys.stderr)
            sys.exit(1)
        label_a = "run_a (baseline)"
    else:
        print("provide --run-a or --simulate-run-a", file=sys.stderr)
        sys.exit(1)

    ma = metrics(sessions_a)
    print_comparison(ma, mb, label_a=label_a, label_b="run_b (augmented)")

    if args.output:
        result = {
            "run_a": {"label": label_a, "metrics": ma},
            "run_b": {"label": "run_b (augmented)", "metrics": mb},
            "tcr_delta_pct": round(mb["tcr_pct"] - ma["tcr_pct"], 1),
        }
        args.output.write_text(json.dumps(result, indent=2))
        print(f"\nresults written to {args.output}")


if __name__ == "__main__":
    main()
