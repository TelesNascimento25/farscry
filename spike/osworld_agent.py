#!/usr/bin/env python3
import sys
import types

for _pkg in ["acoustid", "librosa", "fastdtw", "PyPDF2", "borb", "mutagen", "pdfplumber"]:
    if _pkg not in sys.modules:
        sys.modules[_pkg] = types.ModuleType(_pkg)

import argparse
import base64
import json
import os
import struct
import subprocess
import time
from io import BytesIO
from pathlib import Path

import requests

VL_SERVER = os.environ.get("VL_SERVER", "http://localhost:8083")
FARSCRY_BIN = os.environ.get("FARSCRY_BIN", "farscry")
SESSION_DIR = Path(os.environ.get("FARSCRY_SESSION_DIR", os.path.expanduser("~/.farscry/osworld")))

SYSTEM_BASE = """You control a desktop. Output a single pyautogui Python statement to complete the next step toward the task.
Examples:
  pyautogui.click(850, 420)
  pyautogui.doubleClick(400, 300)
  pyautogui.typewrite('hello world', interval=0.05)
  pyautogui.hotkey('ctrl', 'c')
  pyautogui.scroll(800, 500, clicks=3)
  DONE
  FAIL: reason

Output only the statement. Do not explain."""

SYSTEM_AUG = SYSTEM_BASE + """

Screen state (VASP) is provided as structured text with element positions.
When you see SILENT_FAILURE, your last action had no visual effect — choose a different action."""


def encode(path: str) -> str:
    with open(path, "rb") as f:
        return base64.b64encode(f.read()).decode()


def vl_call(screenshot_path: str, task: str, history: list,
             vasp: str = "", sf_feedback: str = "") -> str:
    augmented = bool(vasp)
    parts = [f"Task: {task}"]
    if sf_feedback:
        parts.append(sf_feedback)
    if vasp:
        parts.append(f"Screen (VASP):\n{vasp[:1500]}")
    parts.append("Next action:")

    messages = [{"role": "system", "content": SYSTEM_AUG if augmented else SYSTEM_BASE}]
    for h in history[-4:]:
        messages.append(h)
    messages.append({"role": "user", "content": [
        {"type": "text", "text": "\n".join(parts)},
        {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{encode(screenshot_path)}"}},
    ]})

    r = requests.post(f"{VL_SERVER}/v1/chat/completions",
                      json={"model": "qwen2.5-vl", "messages": messages,
                            "max_tokens": 128, "temperature": 0.0},
                      timeout=60)
    r.raise_for_status()
    return r.json()["choices"][0]["message"]["content"].strip()


def farscry_state(screenshot_path: str) -> tuple[str, str]:
    try:
        r = subprocess.run([FARSCRY_BIN, "extract", screenshot_path],
                          capture_output=True, text=True, timeout=10)
        out = r.stdout
        for line in out.splitlines():
            if line.startswith("state_id:"):
                return line.split(":", 1)[1].strip(), out
        return "", out
    except Exception as e:
        return "", str(e)


def save_screenshot(obs: dict, path: str):
    screenshot = obs.get("screenshot")
    if screenshot is None:
        return
    if isinstance(screenshot, bytes):
        with open(path, "wb") as f:
            f.write(screenshot)
    else:
        try:
            from PIL import Image
            import numpy as np
            img = Image.fromarray(np.array(screenshot, dtype="uint8"))
            img.save(path)
        except Exception:
            pass


def action_to_pyautogui(raw: str) -> str | None:
    raw = raw.strip()
    if raw.upper().startswith("DONE") or raw.upper().startswith("FAIL"):
        return None
    if "pyautogui." in raw:
        return raw
    return None


def run_task(env, task_id: str, task_instr: str, task_config: dict,
             max_steps: int, augmented: bool, out_dir: Path) -> dict:
    SESSION_DIR.mkdir(parents=True, exist_ok=True)
    out_dir.mkdir(parents=True, exist_ok=True)

    session_id = f"{int(time.time())}-{task_id.replace('/', '_')}"
    history: list[dict] = []
    sf_frames: list[tuple] = []
    max_csf = 0
    consecutive_sf = 0
    state_before = ""

    obs = env.reset(task_config=task_config)

    for step in range(max_steps):
        shot_path = str(out_dir / f"{session_id}-s{step:02d}.png")
        save_screenshot(obs, shot_path)

        if not os.path.exists(shot_path):
            break

        vasp_text = ""
        if augmented:
            state_before, vasp_text = farscry_state(shot_path)
            sf_frames.append((state_before, "state", vasp_text))
            sf_frames.append((None, "marker", ""))

        sf_feedback = ""
        if augmented and consecutive_sf >= 1:
            sf_feedback = f"SILENT_FAILURE (x{consecutive_sf}): action had no visual effect. Try something different."

        raw = vl_call(shot_path, task_instr, history, vasp=vasp_text, sf_feedback=sf_feedback)
        history.append({"role": "assistant", "content": raw})

        if raw.upper().startswith("DONE") or raw.upper().startswith("FAIL"):
            break

        action_str = action_to_pyautogui(raw)
        if not action_str:
            break

        obs, reward, done, info = env.step(action_str)

        if augmented:
            shot_after = str(out_dir / f"{session_id}-s{step:02d}b.png")
            save_screenshot(obs, shot_after)
            if os.path.exists(shot_after):
                state_after, vasp_after = farscry_state(shot_after)
                sf_frames.append((state_after, "state", vasp_after))
                if state_before and state_after and state_before == state_after:
                    consecutive_sf += 1
                    max_csf = max(max_csf, consecutive_sf)
                else:
                    consecutive_sf = 0

        if done:
            break

    score = float(env.evaluate())

    vasf_path = str(SESSION_DIR / f"{session_id}.vasf")
    _write_vasf(vasf_path, sf_frames, augmented)

    return {
        "task_id": task_id,
        "session_id": session_id,
        "augmented": augmented,
        "steps": step + 1,
        "score": score,
        "passed": score > 0.5,
        "max_consecutive_sf": max_csf,
        "vasf": vasf_path,
    }


def _write_vasf(path: str, frames: list, augmented: bool):
    try:
        import zstandard as zstd
        cctx = zstd.ZstdCompressor(level=3)
    except ImportError:
        return

    actual_frames = [(sid, vasp) for sid, kind, vasp in frames
                     if kind in ("state", "marker")]

    with open(path, "wb") as f:
        f.write(b"VASF")
        f.write(struct.pack("<H", 2))
        f.write(struct.pack("<I", len(actual_frames)))
        f.write(struct.pack("<q", int(time.time())))
        f.write(struct.pack("<I", len(actual_frames)))

        for sid, kind, vasp in frames:
            is_marker = kind == "marker"
            vasp_text = "action_marker\n" if is_marker else (vasp or "screen_type: unknown\n")
            bits = 0 if is_marker else _state_bits(sid or "")
            compressed = cctx.compress(vasp_text.encode("utf-8"))
            f.write(struct.pack("<Q", bits))
            f.write(struct.pack("<q", int(time.time() * 1000)))
            f.write(struct.pack("<I", len(compressed)))
            f.write(compressed)
            f.write(struct.pack("<I", 0))


def _state_bits(state_id: str) -> int:
    try:
        return int(state_id.replace("phash:", ""), 16)
    except ValueError:
        return 0


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--mode", choices=["run_a", "run_b"], required=True)
    p.add_argument("--tasks", type=Path, required=True)
    p.add_argument("--n", type=int, default=30)
    p.add_argument("--max-steps", type=int, default=15)
    p.add_argument("--result-dir", type=Path, default=Path("./osworld_results"))
    p.add_argument("--vm-path", type=str, default=None)
    p.add_argument("--provider", type=str, default="docker")
    p.add_argument("--output", type=Path, default=None)
    args = p.parse_args()

    from desktop_env.desktop_env import DesktopEnv

    augmented = args.mode == "run_b"
    args.result_dir.mkdir(parents=True, exist_ok=True)

    tasks = json.loads(args.tasks.read_text())[:args.n]
    print(f"mode={args.mode}  augmented={augmented}  n={len(tasks)}")

    results = []
    for task in tasks:
        task_id = task["id"]
        task_instr = task["instruction"]
        config_path = task.get("config_path", "")
        task_config = json.loads(Path(config_path).read_text()) if config_path and Path(config_path).exists() else {}

        print(f"\n--- {task_id}: {task_instr[:70]}")

        env = DesktopEnv(
            path_to_vm=args.vm_path,
            provider_name=args.provider,
            action_space="pyautogui",
            observation_type="screenshot",
            headless=True,
        )

        try:
            r = run_task(env, task_id, task_instr, task_config,
                         max_steps=args.max_steps, augmented=augmented,
                         out_dir=args.result_dir)
            results.append(r)
            status = "PASS" if r["passed"] else "FAIL"
            print(f"    {status}  score={r['score']:.2f}  steps={r['steps']}  max_csf={r['max_consecutive_sf']}")
        except Exception as e:
            print(f"    ERROR: {e}")
            results.append({"task_id": task_id, "score": 0.0, "passed": False, "error": str(e)})
        finally:
            try:
                env.close()
            except Exception:
                pass

    n = len(results)
    n_pass = sum(1 for r in results if r.get("passed", False))
    tcr = n_pass / n if n > 0 else 0.0
    print(f"\n{'='*40}")
    print(f"mode={args.mode}  TCR={tcr:.1%}  ({n_pass}/{n})")
    print(f"{'='*40}")

    if args.output:
        out = {"mode": args.mode, "augmented": augmented, "n": n,
               "n_pass": n_pass, "tcr": tcr, "tcr_pct": round(tcr * 100, 1),
               "results": results}
        args.output.write_text(json.dumps(out, indent=2))
        print(f"written to {args.output}")


if __name__ == "__main__":
    main()
