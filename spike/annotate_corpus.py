#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["anthropic>=0.28", "numpy>=1.24", "zstandard>=0.22"]
# ///

import argparse
import json
import os
import struct
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

import anthropic
import numpy as np
import zstandard as zstd

VASF_MAGIC = b"VASF"
ACTION_MARKER_PREFIX = "action_marker"
_dctx = zstd.ZstdDecompressor()


def _decompress(data: bytes) -> bytes:
    if not data:
        return b""
    try:
        return _dctx.stream_reader(data).read()
    except Exception:
        return b""


@dataclass
class VasfFrame:
    state_id: int
    vasp_text: str


def read_vasf(path: Path) -> list[VasfFrame]:
    frames = []
    with open(path, "rb") as f:
        if f.read(4) != VASF_MAGIC:
            raise ValueError(f"not a VASF file: {path}")
        f.read(2)
        frame_count = struct.unpack("<I", f.read(4))[0]
        f.read(8 + 4)
        for _ in range(frame_count):
            state_id = struct.unpack("<Q", f.read(8))[0]
            f.read(8)
            vl = struct.unpack("<I", f.read(4))[0]
            vasp = _decompress(f.read(vl)) if vl > 0 else b""
            dl = struct.unpack("<I", f.read(4))[0]
            if dl > 0:
                f.read(dl)
            frames.append(VasfFrame(state_id=state_id, vasp_text=vasp.decode("utf-8", errors="replace")))
    return frames


def is_marker(frame: VasfFrame) -> bool:
    return frame.vasp_text.startswith(ACTION_MARKER_PREFIX)


def vasp_field(text: str, field: str) -> str:
    for line in text.splitlines():
        if line.startswith(field):
            return line[len(field):].strip().strip('"')
    return ""


@dataclass
class ActionLabel:
    action_index: int
    state_id_before: int
    state_id_after: int
    vasp_before: str
    vasp_after: str
    farscry_label: str
    llm_label: Optional[str] = None
    llm_reasoning: Optional[str] = None


def extract_actions(frames: list[VasfFrame]) -> list[ActionLabel]:
    marker_indices = [i for i, f in enumerate(frames) if is_marker(f)]
    actions = []
    for idx, midx in enumerate(marker_indices):
        before = next((frames[i] for i in range(midx - 1, -1, -1) if not is_marker(frames[i])), None)
        after = next((frames[i] for i in range(midx + 1, len(frames)) if not is_marker(frames[i])), None)
        if before is None or after is None:
            continue
        label = "SF" if before.state_id == after.state_id else "AE"
        actions.append(ActionLabel(
            action_index=idx,
            state_id_before=before.state_id,
            state_id_after=after.state_id,
            vasp_before=before.vasp_text,
            vasp_after=after.vasp_text,
            farscry_label=label,
        ))
    return actions


PROMPT = """You are annotating a computer-use agent corpus to validate silent failure detection.

Silent Failure (SF): the agent took an action but the screen did not change.
Action Effect (AE): the action visibly changed the screen.

State ID before: {state_id_before}
State ID after:  {state_id_after}

=== SCREEN BEFORE ACTION ===
{vasp_before}

=== SCREEN AFTER ACTION ===
{vasp_after}

Respond with exactly this JSON:
{{"label": "SF" or "AE", "confidence": 0.0-1.0, "reasoning": "one sentence"}}"""


def annotate(client: anthropic.Anthropic, action: ActionLabel, model: str) -> tuple[str, str]:
    resp = client.messages.create(
        model=model,
        max_tokens=256,
        messages=[{"role": "user", "content": PROMPT.format(
            state_id_before=hex(action.state_id_before),
            state_id_after=hex(action.state_id_after),
            vasp_before=action.vasp_before[:800] or "(empty)",
            vasp_after=action.vasp_after[:800] or "(empty)",
        )}],
    )
    text = resp.content[0].text.strip()
    try:
        parsed = json.loads(text)
        label = parsed.get("label", "UNKNOWN").upper()
        if label not in ("SF", "AE"):
            label = "UNKNOWN"
        return label, parsed.get("reasoning", "")
    except json.JSONDecodeError:
        if "SF" in text.upper() and "AE" not in text.upper():
            return "SF", text[:100]
        if "AE" in text.upper():
            return "AE", text[:100]
        return "UNKNOWN", text[:100]


def cohens_kappa(labels_a: list[str], labels_b: list[str]) -> dict:
    n = len(labels_a)
    if n == 0:
        return {"kappa": 0.0, "n": 0}
    tp = sum(1 for a, b in zip(labels_a, labels_b) if a == "SF" and b == "SF")
    fp = sum(1 for a, b in zip(labels_a, labels_b) if a == "AE" and b == "SF")
    fn = sum(1 for a, b in zip(labels_a, labels_b) if a == "SF" and b == "AE")
    tn = sum(1 for a, b in zip(labels_a, labels_b) if a == "AE" and b == "AE")
    po = (tp + tn) / n
    p_sf_a = (tp + fn) / n
    p_sf_b = (tp + fp) / n
    p_ae_a = (fp + tn) / n
    p_ae_b = (fn + tn) / n
    pe = p_sf_a * p_sf_b + p_ae_a * p_ae_b
    kappa = (po - pe) / (1 - pe) if pe < 1.0 else 1.0
    interp = ("almost perfect" if kappa >= 0.80 else "substantial" if kappa >= 0.60
              else "moderate" if kappa >= 0.40 else "fair" if kappa >= 0.20 else "slight or poor")
    return {
        "kappa": round(kappa, 4),
        "interpretation": interp,
        "observed_agreement": round(po, 4),
        "n": n,
        "confusion_matrix": {"TP": tp, "FP": fp, "FN": fn, "TN": tn},
    }


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--vasf-dir", type=Path, required=True)
    p.add_argument("--failed-txt", type=Path, default=None)
    p.add_argument("--output", type=Path, default=Path("annotations.json"))
    p.add_argument("--model", default="claude-3-5-haiku-20241022")
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--max-actions", type=int, default=None)
    args = p.parse_args()

    if not args.vasf_dir.is_dir():
        print(f"not a directory: {args.vasf_dir}", file=sys.stderr)
        sys.exit(1)

    vasf_files = sorted(args.vasf_dir.glob("*.vasf"))
    if not vasf_files:
        print(f"no .vasf files in {args.vasf_dir}", file=sys.stderr)
        sys.exit(1)

    failed_names: set[str] = set()
    if args.failed_txt and args.failed_txt.exists():
        failed_names = {l.strip() for l in args.failed_txt.read_text().splitlines() if l.strip()}

    all_actions: list[tuple[str, ActionLabel]] = []
    sessions_meta = []

    for vf in vasf_files:
        try:
            frames = read_vasf(vf)
        except Exception as e:
            print(f"warn: {vf.name}: {e}", file=sys.stderr)
            continue
        actions = extract_actions(frames)
        sf = sum(1 for a in actions if a.farscry_label == "SF")
        ae = sum(1 for a in actions if a.farscry_label == "AE")
        is_failed = vf.name in failed_names
        print(f"  {vf.name}: {len(actions)} actions  SF={sf}  AE={ae}  {'FAILED' if is_failed else 'OK'}")
        sessions_meta.append({"filename": vf.name, "n_actions": len(actions),
                               "sf_count": sf, "ae_count": ae, "is_failed": is_failed})
        for action in actions:
            all_actions.append((vf.name, action))

    print(f"\ntotal actions: {len(all_actions)}")
    print(f"farscry SF: {sum(1 for _, a in all_actions if a.farscry_label == 'SF')}")
    print(f"farscry AE: {sum(1 for _, a in all_actions if a.farscry_label == 'AE')}")

    if args.dry_run:
        return

    api_key = os.environ.get("ANTHROPIC_API_KEY")
    if not api_key:
        print("ANTHROPIC_API_KEY not set", file=sys.stderr)
        sys.exit(1)

    client = anthropic.Anthropic(api_key=api_key)
    to_annotate = all_actions[:args.max_actions] if args.max_actions else all_actions
    print(f"\nannotating {len(to_annotate)} actions with {args.model}...")

    annotated: list[tuple[str, ActionLabel]] = []
    for i, (session, action) in enumerate(to_annotate, 1):
        try:
            llm_label, reasoning = annotate(client, action, model=args.model)
            action.llm_label = llm_label
            action.llm_reasoning = reasoning
            annotated.append((session, action))
            match = "=" if llm_label == action.farscry_label else "!"
            print(f"  [{i:3}/{len(to_annotate)}] {session} a{action.action_index}: "
                  f"farscry={action.farscry_label} llm={llm_label} {match}")
        except Exception as e:
            print(f"  [{i:3}/{len(to_annotate)}] error: {e}", file=sys.stderr)

    valid = [(s, a) for s, a in annotated
             if a.farscry_label in ("SF", "AE") and a.llm_label in ("SF", "AE")]
    kappa = cohens_kappa(
        [a.farscry_label for _, a in valid],
        [a.llm_label for _, a in valid],
    )

    print(f"\nkappa={kappa['kappa']:.4f}  ({kappa['interpretation']})")
    print(f"agreement={kappa['observed_agreement']:.1%}  n={kappa['n']}")
    cm = kappa["confusion_matrix"]
    print(f"TP={cm['TP']} FP={cm['FP']} FN={cm['FN']} TN={cm['TN']}")

    output = {
        **kappa,
        "model": args.model,
        "sessions": sessions_meta,
        "actions": [
            {"session": s, "action_index": a.action_index,
             "state_before": hex(a.state_id_before), "state_after": hex(a.state_id_after),
             "farscry": a.farscry_label, "llm": a.llm_label,
             "llm_reasoning": a.llm_reasoning, "agreement": a.farscry_label == a.llm_label}
            for s, a in valid
        ],
    }
    args.output.write_text(json.dumps(output, indent=2))
    print(f"\nresults written to {args.output}")


if __name__ == "__main__":
    main()
