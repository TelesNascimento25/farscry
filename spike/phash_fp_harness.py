#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["Pillow>=10.0", "numpy>=1.24", "scipy>=1.11", "imagehash>=4.3"]
# ///
"""
pHash False Positive / False Negative Harness for farscry.

Answers the @arromber question: "qual a curva ROC do threshold?"

Generates synthetic screenshot pairs at increasing "drift" magnitudes:
  - Drift 0:   identical screenshots (true SF — should detect)
  - Drift 1:   1 pixel changed (cursor blink equivalent)
  - Drift N:   N random pixels changed
  - Spinner:   16x16 region changed (loading spinner equivalent)
  - Clock:     digits-sized region changed (clock tick equivalent)
  - Major:     25% of pixels changed (actual meaningful change — should NOT detect as SF)

For each pair, computes pHash Hamming distance and reports:
  - At exact match (threshold=0): FP rate, FN rate
  - At thresholds 1-8: full ROC curve

Usage:
  uv run spike/phash_fp_harness.py
  uv run spike/phash_fp_harness.py --n 500 --output roc.csv
"""

import argparse
import csv
import math
import random
import sys
from dataclasses import dataclass
from io import BytesIO
from typing import Optional

from PIL import Image
import numpy as np


# --- pHash implementation matching farscry's hash.rs ---

import imagehash

def phash_image(img: Image.Image) -> int:
    """
    Compute pHash using imagehash.phash() which implements the same algorithm
    as farscry's hash.rs: 32x32 resize, grayscale, 2D DCT-II, top-8x8, median threshold.
    Returns 64-bit integer.
    """
    h = imagehash.phash(img, hash_size=8, highfreq_factor=4)
    return int(str(h), 16)


def hamming_distance(h1: int, h2: int) -> int:
    return bin(h1 ^ h2).count("1")


# --- Synthetic image generation ---

def make_base_image(w: int = 1280, h: int = 800, seed: int = 42) -> Image.Image:
    rng = random.Random(seed)
    img = Image.new("RGB", (w, h))
    pixels = img.load()
    for y in range(h):
        for x in range(w):
            r = rng.randint(30, 220)
            g = rng.randint(30, 220)
            b = rng.randint(30, 220)
            pixels[x, y] = (r, g, b)
    return img


def add_pixel_noise(img: Image.Image, n_pixels: int, rng: random.Random) -> Image.Image:
    """Change n_pixels random pixels to random colors."""
    out = img.copy()
    pixels = out.load()
    w, h = out.size
    for _ in range(n_pixels):
        x = rng.randint(0, w - 1)
        y = rng.randint(0, h - 1)
        pixels[x, y] = (rng.randint(0, 255), rng.randint(0, 255), rng.randint(0, 255))
    return out


def add_region_change(img: Image.Image, rx: int, ry: int, rw: int, rh: int,
                       rng: random.Random) -> Image.Image:
    """Change a rectangular region (spinner, clock, etc.)."""
    out = img.copy()
    pixels = out.load()
    for y in range(ry, min(ry + rh, img.height)):
        for x in range(rx, min(rx + rw, img.width)):
            pixels[x, y] = (rng.randint(0, 255), rng.randint(0, 255), rng.randint(0, 255))
    return out


def add_major_change(img: Image.Image, pct: float, rng: random.Random) -> Image.Image:
    """Change pct% of pixels — simulates a real navigation change."""
    n = int(img.width * img.height * pct)
    return add_pixel_noise(img, n, rng)


# --- Test case definitions ---

@dataclass
class TestCase:
    name: str
    is_true_sf: bool  # True = action had no effect (should detect SF), False = action worked
    description: str


def generate_pairs(n: int, seed: int = 0):
    """
    Generate (before, after, TestCase) triples.
    Returns list of (h_before, h_after, test_case).
    """
    base = make_base_image(seed=seed)
    rng = random.Random(seed + 1)
    results = []

    for i in range(n):
        rng_i = random.Random(seed + i * 100)
        base_i = make_base_image(seed=seed + i)  # Unique base per pair

        cases = [
            # TRUE SILENT FAILURES (is_true_sf=True): before == after
            ("identical", True, "Exact same screenshot — clear SF",
             base_i, base_i.copy()),

            # ANIMATION-MASKED FAILURES (is_true_sf=True but detector might miss)
            ("cursor_blink", True, "1 pixel changed (cursor blink) — action failed",
             base_i, add_pixel_noise(base_i, 1, rng_i)),

            ("clock_tick", True, "Small region changed (clock tick, ~400px) — action failed",
             base_i, add_region_change(base_i, 1200, 0, 20, 20, rng_i)),

            ("spinner", True, "16x16 spinner region changed — action failed",
             base_i, add_region_change(base_i, 600, 380, 16, 16, rng_i)),

            ("minor_noise_10", True, "10 random pixels changed — action failed",
             base_i, add_pixel_noise(base_i, 10, rng_i)),

            # TRUE ACTION EFFECTS (is_true_sf=False): before != after
            ("nav_change", False, "25% of screen changed — real navigation",
             base_i, add_major_change(base_i, 0.25, rng_i)),

            ("form_fill", False, "2% of screen changed — form field filled",
             base_i, add_major_change(base_i, 0.02, rng_i)),

            ("button_press", False, "Small but meaningful region changed — button pressed",
             base_i, add_region_change(base_i, 400, 300, 80, 30, rng_i)),
        ]

        for name, is_sf, desc, before, after in cases:
            h_before = phash_image(before)
            h_after = phash_image(after)
            dist = hamming_distance(h_before, h_after)
            results.append({
                "name": name,
                "is_true_sf": is_sf,
                "description": desc,
                "hamming_distance": dist,
            })

    return results


# --- ROC curve computation ---

def compute_roc(pairs, max_threshold: int = 8):
    """
    For each threshold t (0..max_threshold):
      Predict SF if hamming_distance <= t.
    Returns list of (threshold, tpr, fpr, precision, recall, f1).
    """
    true_sfs = [p for p in pairs if p["is_true_sf"]]
    true_aes = [p for p in pairs if not p["is_true_sf"]]
    n_sf = len(true_sfs)
    n_ae = len(true_aes)

    rows = []
    for t in range(max_threshold + 1):
        tp = sum(1 for p in true_sfs if p["hamming_distance"] <= t)
        fp = sum(1 for p in true_aes if p["hamming_distance"] <= t)
        fn = n_sf - tp
        tn = n_ae - fp

        tpr = tp / n_sf if n_sf > 0 else 0.0
        fpr = fp / n_ae if n_ae > 0 else 0.0
        precision = tp / (tp + fp) if (tp + fp) > 0 else 0.0
        recall = tpr
        f1 = 2 * precision * recall / (precision + recall) if (precision + recall) > 0 else 0.0

        rows.append({
            "threshold": t,
            "tp": tp, "fp": fp, "fn": fn, "tn": tn,
            "tpr": tpr, "fpr": fpr,
            "precision": precision, "recall": recall, "f1": f1,
        })
    return rows


def print_roc_table(roc, pairs):
    n = len(pairs)
    n_sf = sum(1 for p in pairs if p["is_true_sf"])
    n_ae = n - n_sf
    print(f"\npHash ROC Curve  (N={n}, true_SF={n_sf}, true_AE={n_ae})")
    print(f"{'Threshold':>9}  {'TPR':>6}  {'FPR':>6}  {'Precision':>9}  {'Recall':>6}  {'F1':>6}  {'TP':>4}  {'FP':>4}  {'FN':>4}  {'TN':>4}")
    print("-" * 75)
    for r in roc:
        marker = " <-- current farscry (exact match)" if r["threshold"] == 0 else ""
        marker = " <-- Hamming-3 (farscry dedup)" if r["threshold"] == 3 else marker
        print(
            f"  {r['threshold']:>7}  {r['tpr']:>6.3f}  {r['fpr']:>6.3f}"
            f"  {r['precision']:>9.3f}  {r['recall']:>6.3f}  {r['f1']:>6.3f}"
            f"  {r['tp']:>4}  {r['fp']:>4}  {r['fn']:>4}  {r['tn']:>4}{marker}"
        )


def print_breakdown(pairs):
    by_name: dict[str, list] = {}
    for p in pairs:
        by_name.setdefault(p["name"], []).append(p["hamming_distance"])

    print("\nHamming distance by test case (mean ± std):")
    print(f"  {'Case':25}  {'SF?':5}  {'mean':>6}  {'std':>6}  {'min':>4}  {'max':>4}")
    print("  " + "-" * 58)
    for name, dists in sorted(by_name.items()):
        is_sf = next(p["is_true_sf"] for p in pairs if p["name"] == name)
        arr = np.array(dists, dtype=float)
        print(
            f"  {name:25}  {'SF' if is_sf else 'AE':5}  "
            f"{arr.mean():>6.2f}  {arr.std():>6.2f}  {arr.min():>4.0f}  {arr.max():>4.0f}"
        )


def main():
    parser = argparse.ArgumentParser(description="pHash false positive / false negative harness")
    parser.add_argument("--n", type=int, default=50,
                        help="Number of pairs per test case (default: 50)")
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--output", type=str, default=None,
                        help="Write ROC CSV to this path")
    parser.add_argument("--max-threshold", type=int, default=8)
    args = parser.parse_args()

    print(f"Generating {args.n} pairs per test case (seed={args.seed})...")
    pairs = generate_pairs(args.n, seed=args.seed)
    print(f"Total pairs: {len(pairs)}")

    roc = compute_roc(pairs, max_threshold=args.max_threshold)
    print_roc_table(roc, pairs)
    print_breakdown(pairs)

    if args.output:
        with open(args.output, "w", newline="") as f:
            writer = csv.DictWriter(f, fieldnames=list(roc[0].keys()))
            writer.writeheader()
            writer.writerows(roc)
        print(f"\nROC data written to {args.output}")

    # Summary verdict
    exact = roc[0]
    best_f1 = max(roc, key=lambda r: r["f1"])
    print(f"\n--- VERDICT ---")
    print(f"Exact match (t=0):  TPR={exact['tpr']:.1%}  FPR={exact['fpr']:.1%}  F1={exact['f1']:.3f}")
    print(f"Best F1 at t={best_f1['threshold']}:  TPR={best_f1['tpr']:.1%}  FPR={best_f1['fpr']:.1%}  F1={best_f1['f1']:.3f}")
    if best_f1["threshold"] == 0:
        print("Exact match IS the optimal threshold. pHash is clean.")
    else:
        print(f"Relaxing to t={best_f1['threshold']} would improve recall at cost of {best_f1['fpr']:.1%} FPR.")
    print()
    print("Note: 'animation-masked SF' cases (clock_tick, spinner, cursor_blink)")
    print("represent FALSE NEGATIVES — SFs the detector MISSES because the animation")
    print("changes the pHash slightly. These are the real adversarial cases.")


if __name__ == "__main__":
    main()
