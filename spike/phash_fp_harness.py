#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["Pillow>=10.0", "numpy>=1.24", "scipy>=1.11", "imagehash>=4.3"]
# ///

import argparse
import csv
import random
import sys
from dataclasses import dataclass

from PIL import Image
import numpy as np
import imagehash


def phash(img: Image.Image) -> int:
    h = imagehash.phash(img, hash_size=8, highfreq_factor=4)
    return int(str(h), 16)


def hamming(h1: int, h2: int) -> int:
    return bin(h1 ^ h2).count("1")


def make_base(w: int = 1280, h: int = 800, seed: int = 42) -> Image.Image:
    rng = random.Random(seed)
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        for x in range(w):
            px[x, y] = (rng.randint(30, 220), rng.randint(30, 220), rng.randint(30, 220))
    return img


def noise(img: Image.Image, n: int, rng: random.Random) -> Image.Image:
    out = img.copy()
    px = out.load()
    w, h = out.size
    for _ in range(n):
        x, y = rng.randint(0, w - 1), rng.randint(0, h - 1)
        px[x, y] = (rng.randint(0, 255), rng.randint(0, 255), rng.randint(0, 255))
    return out


def region(img: Image.Image, rx: int, ry: int, rw: int, rh: int, rng: random.Random) -> Image.Image:
    out = img.copy()
    px = out.load()
    for y in range(ry, min(ry + rh, img.height)):
        for x in range(rx, min(rx + rw, img.width)):
            px[x, y] = (rng.randint(0, 255), rng.randint(0, 255), rng.randint(0, 255))
    return out


def generate(n: int, seed: int = 0):
    results = []
    for i in range(n):
        rng = random.Random(seed + i * 100)
        base = make_base(seed=seed + i)
        cases = [
            ("identical",       True,  base,                              base.copy()),
            ("cursor_blink",    True,  base,                              noise(base, 1, rng)),
            ("clock_tick",      True,  base,                              region(base, 1200, 0, 20, 20, rng)),
            ("spinner",         True,  base,                              region(base, 600, 380, 16, 16, rng)),
            ("minor_noise_10",  True,  base,                              noise(base, 10, rng)),
            ("nav_change",      False, base,                              noise(base, int(1280 * 800 * 0.25), rng)),
            ("form_fill",       False, base,                              noise(base, int(1280 * 800 * 0.02), rng)),
            ("button_press",    False, base,                              region(base, 400, 300, 80, 30, rng)),
        ]
        for name, is_sf, before, after in cases:
            results.append({
                "name": name,
                "is_true_sf": is_sf,
                "hamming_distance": hamming(phash(before), phash(after)),
            })
    return results


def roc(pairs, max_t: int = 8):
    true_sfs = [p for p in pairs if p["is_true_sf"]]
    true_aes = [p for p in pairs if not p["is_true_sf"]]
    n_sf, n_ae = len(true_sfs), len(true_aes)
    rows = []
    for t in range(max_t + 1):
        tp = sum(1 for p in true_sfs if p["hamming_distance"] <= t)
        fp = sum(1 for p in true_aes if p["hamming_distance"] <= t)
        fn, tn = n_sf - tp, n_ae - fp
        tpr = tp / n_sf if n_sf > 0 else 0.0
        fpr = fp / n_ae if n_ae > 0 else 0.0
        prec = tp / (tp + fp) if (tp + fp) > 0 else 0.0
        f1 = 2 * prec * tpr / (prec + tpr) if (prec + tpr) > 0 else 0.0
        rows.append({"threshold": t, "tp": tp, "fp": fp, "fn": fn, "tn": tn,
                     "tpr": tpr, "fpr": fpr, "precision": prec, "recall": tpr, "f1": f1})
    return rows


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--n", type=int, default=50)
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--output", type=str, default=None)
    p.add_argument("--max-threshold", type=int, default=8)
    args = p.parse_args()

    pairs = generate(args.n, seed=args.seed)
    curve = roc(pairs, max_t=args.max_threshold)

    n_sf = sum(1 for p in pairs if p["is_true_sf"])
    n_ae = len(pairs) - n_sf
    print(f"N={len(pairs)}  true_SF={n_sf}  true_AE={n_ae}")
    print(f"{'T':>3}  {'TPR':>6}  {'FPR':>6}  {'F1':>6}  {'TP':>4}  {'FP':>4}  {'FN':>4}  {'TN':>4}")
    print("-" * 48)
    for r in curve:
        note = ""
        if r["threshold"] == 0: note = "  <- farscry exact match"
        if r["threshold"] == 3: note = "  <- Hamming-3 dedup"
        print(f"{r['threshold']:>3}  {r['tpr']:>6.3f}  {r['fpr']:>6.3f}  {r['f1']:>6.3f}"
              f"  {r['tp']:>4}  {r['fp']:>4}  {r['fn']:>4}  {r['tn']:>4}{note}")

    by_name: dict[str, list] = {}
    for pair in pairs:
        by_name.setdefault(pair["name"], []).append(pair["hamming_distance"])
    print()
    print(f"  {'case':25}  {'SF?':4}  {'mean':>5}  {'std':>5}  {'min':>3}  {'max':>3}")
    for name, dists in sorted(by_name.items()):
        is_sf = next(p["is_true_sf"] for p in pairs if p["name"] == name)
        arr = np.array(dists, dtype=float)
        print(f"  {name:25}  {'SF' if is_sf else 'AE':4}  {arr.mean():>5.2f}  {arr.std():>5.2f}"
              f"  {arr.min():>3.0f}  {arr.max():>3.0f}")

    exact = curve[0]
    best = max(curve, key=lambda r: r["f1"])
    print(f"\nexact match (t=0): TPR={exact['tpr']:.1%}  FPR={exact['fpr']:.1%}  F1={exact['f1']:.3f}")
    print(f"best F1 at t={best['threshold']}: TPR={best['tpr']:.1%}  FPR={best['fpr']:.1%}  F1={best['f1']:.3f}")

    if args.output:
        with open(args.output, "w", newline="") as f:
            w = csv.DictWriter(f, fieldnames=list(curve[0].keys()))
            w.writeheader()
            w.writerows(curve)


if __name__ == "__main__":
    main()
