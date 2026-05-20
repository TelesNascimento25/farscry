#!/usr/bin/env python3
import sys
import types

class _Stub(types.ModuleType):
    def __getattr__(self, name):
        sub = _Stub(self.__name__ + "." + name)
        sys.modules[sub.__name__] = sub
        return sub
    def __call__(self, *a, **kw):
        return _Stub("_call")

for _pkg in ["acoustid", "librosa", "fastdtw", "PyPDF2", "borb", "borb.pdf",
             "mutagen", "pdfplumber", "ag2", "agp_client", "easyocr", "torch", "cv2"]:
    if _pkg not in sys.modules:
        sys.modules[_pkg] = _Stub(_pkg)

import argparse
import base64
import json
import os
import re as _re
import struct
import subprocess
import time
import xml.etree.ElementTree as ET
from io import BytesIO
from pathlib import Path

import requests

VL_SERVER = os.environ.get("VL_SERVER", "http://localhost:8083")
FARSCRY_BIN = os.environ.get("FARSCRY_BIN", "farscry")
SESSION_DIR = Path(os.environ.get("FARSCRY_SESSION_DIR", os.path.expanduser("~/.farscry/osworld")))

_A11Y_STATE_NS = "https://accessibility.ubuntu.example.org/ns/state"
_A11Y_COMP_NS  = "https://accessibility.ubuntu.example.org/ns/component"

INTERACTIVE_ROLES = {
    "button", "check-box", "combo-box", "entry", "link", "menu", "menuitem",
    "radio-button", "searchbox", "slider", "spin-button", "text", "textbox",
    "textarea", "textfield", "toggle-button", "push-button", "menu-item",
}

SYSTEM_BASE = """You control a desktop. Output a single pyautogui Python statement.
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

Accessible UI elements are listed with exact coordinates. Use them for clicks.
If you receive a [SILENT_FAILURE] warning, try a completely different action."""


def parse_a11y_tree(xml_str: str, max_items: int = 25) -> list[dict]:
    if not xml_str:
        return []
    try:
        root = ET.fromstring(xml_str)
    except ET.ParseError:
        return []

    modal_names: set[str] = set()
    for node in root.iter():
        role = (node.tag.split("}")[-1] if "}" in node.tag else node.tag).lower()
        if role in ("dialog", "alert", "file-chooser", "color-chooser", "font-chooser"):
            name = (node.get("name") or "").strip()
            if name:
                modal_names.add(name)

    elements = []
    for node in root.iter():
        tag = node.tag.split("}")[-1] if "}" in node.tag else node.tag
        role = tag.lower()

        showing = node.get("{%s}showing" % _A11Y_STATE_NS, "false")
        visible = node.get("{%s}visible" % _A11Y_STATE_NS, "false")
        if showing != "true" or visible != "true":
            continue

        name = (node.get("name") or node.text or "").strip()
        if not name:
            continue

        coord_str = node.get("{%s}screencoord" % _A11Y_COMP_NS, "")
        size_str  = node.get("{%s}size" % _A11Y_COMP_NS, "")
        if not coord_str or not size_str:
            continue

        try:
            x, y = map(int, _re.findall(r"-?\d+", coord_str)[:2])
            w, h = map(int, _re.findall(r"-?\d+", size_str)[:2])
        except (ValueError, IndexError):
            continue

        if w <= 0 or h <= 0 or x < 0 or y < 0:
            continue

        cx, cy = x + w // 2, y + h // 2
        elements.append({
            "role": role, "name": name[:60],
            "x": cx, "y": cy,
            "in_modal": any(m in name for m in modal_names),
        })

        if len(elements) >= max_items * 5:
            break

    if modal_names:
        modal_els = [e for e in elements if e["in_modal"] and e["role"] in INTERACTIVE_ROLES]
        if modal_els:
            elements = modal_els

    interactive = [e for e in elements if e["role"] in INTERACTIVE_ROLES]
    if not interactive:
        interactive = elements

    seen: set[tuple] = set()
    result = []
    for e in interactive:
        key = (e["role"], e["name"][:25])
        if key not in seen:
            seen.add(key)
            result.append(e)
        if len(result) >= max_items:
            break

    return result


def format_a11y_context(elements: list[dict]) -> str:
    if not elements:
        return ""
    lines = ["UI elements (role name → click coords):"]
    for e in elements:
        lines.append(f"  {e['role']:12s} \"{e['name']}\" → ({e['x']}, {e['y']})")
    return "\n".join(lines)


def encode(path: str) -> str:
    with open(path, "rb") as f:
        return base64.b64encode(f.read()).decode()


def vl_call(screenshot_path: str, task: str, history: list,
            a11y_context: str = "", sf_feedback: str = "") -> str:
    augmented = bool(a11y_context)
    parts = [f"Task: {task}"]
    if sf_feedback:
        parts.append(sf_feedback)
    if a11y_context:
        parts.append(a11y_context)
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
                      timeout=180)
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


ESCAPE_LADDER = [
    "pyautogui.press('escape')",
    "pyautogui.hotkey('alt', 'F4')",
    "pyautogui.hotkey('ctrl', 'z')",
    "pyautogui.hotkey('ctrl', 'z')",
    "pyautogui.click(960, 540)",
]


def run_task(env, task_id: str, task_instr: str, task_config: dict,
             max_steps: int, augmented: bool, out_dir: Path,
             a11y_only: bool = False) -> dict:
    SESSION_DIR.mkdir(parents=True, exist_ok=True)
    out_dir.mkdir(parents=True, exist_ok=True)

    session_id = f"{int(time.time())}-{task_id.replace('/', '_')}"
    history: list[dict] = []
    sf_frames: list[tuple] = []
    max_csf = 0
    consecutive_sf = 0
    total_escapes = 0
    state_before = ""
    agent_step = 0
    sf_feedback_count = 0

    obs = env.reset(task_config=task_config)

    while agent_step < max_steps:
        step = agent_step
        shot_path = str(out_dir / f"{session_id}-s{step:02d}.png")
        save_screenshot(obs, shot_path)

        if not os.path.exists(shot_path):
            break

        a11y_context = ""
        sf_feedback = ""

        if a11y_only:
            a11y_xml = obs.get("accessibility_tree") if obs else None
            elements = parse_a11y_tree(a11y_xml) if a11y_xml else []
            a11y_context = format_a11y_context(elements)

        if augmented:
            state_before, vasp_text = farscry_state(shot_path)
            sf_frames.append((state_before, "state", vasp_text))
            sf_frames.append((None, "marker", ""))

            a11y_xml = obs.get("accessibility_tree") if obs else None
            elements = parse_a11y_tree(a11y_xml) if a11y_xml else []
            a11y_context = format_a11y_context(elements)

            if consecutive_sf >= 1:
                escape_action = ESCAPE_LADDER[min(total_escapes, len(ESCAPE_LADDER) - 1)]

                if total_escapes == 2:
                    history = history[-1:] if history else []
                    print(f"    [augment] SF total={total_escapes+1} → history cleared")

                print(f"    [augment] SF x{consecutive_sf} total={total_escapes+1} → {escape_action}")
                obs, _, _, _ = env.step(escape_action, pause=1.0)
                sf_feedback_count += 1
                total_escapes += 1
                shot_path = str(out_dir / f"{session_id}-s{step:02d}r.png")
                save_screenshot(obs, shot_path)
                if os.path.exists(shot_path):
                    state_before, _ = farscry_state(shot_path)
                    a11y_xml = obs.get("accessibility_tree") if obs else None
                    elements = parse_a11y_tree(a11y_xml) if a11y_xml else []
                    a11y_context = format_a11y_context(elements)

                consecutive_sf = 0
                sf_feedback = (
                    f"[SILENT_FAILURE] Screen was unchanged. Escape action executed. "
                    f"Try something completely different."
                )

        raw = vl_call(shot_path, task_instr, history,
                      a11y_context=a11y_context, sf_feedback=sf_feedback)
        history.append({"role": "assistant", "content": raw})

        if raw.upper().startswith("DONE"):
            env.step("DONE", pause=0.5)
            break
        if raw.upper().startswith("FAIL"):
            env.step("FAIL", pause=0.5)
            break

        action_str = action_to_pyautogui(raw)
        if not action_str:
            break

        obs, reward, done, info = env.step(action_str, pause=0.5)
        agent_step += 1

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
        "steps": agent_step,
        "sf_feedback_count": sf_feedback_count,
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
    p.add_argument("--mode", choices=["run_a", "run_b_a11y", "run_b"], required=True)
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
    a11y_only = args.mode == "run_b_a11y"
    needs_a11y = augmented or a11y_only
    args.result_dir.mkdir(parents=True, exist_ok=True)

    tasks = json.loads(args.tasks.read_text())[:args.n]
    print(f"mode={args.mode}  augmented={augmented}  a11y_only={a11y_only}  n={len(tasks)}")

    print("Starting DesktopEnv...")
    env = DesktopEnv(
        path_to_vm=args.vm_path,
        provider_name=args.provider,
        action_space="pyautogui",
        require_a11y_tree=needs_a11y,
        headless=True,
    )
    print("DesktopEnv ready.")

    results = []
    try:
        for task in tasks:
            task_id = task["id"]
            task_instr = task["instruction"]
            config_path = task.get("config_path", "")
            task_config = json.loads(Path(config_path).read_text()) if config_path and Path(config_path).exists() else {}

            print(f"\n--- {task_id}: {task_instr[:70]}")

            try:
                r = run_task(env, task_id, task_instr, task_config,
                             max_steps=args.max_steps,
                             augmented=augmented, a11y_only=a11y_only,
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
