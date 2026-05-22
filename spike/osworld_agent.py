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
import signal
import struct
import subprocess
import time
import xml.etree.ElementTree as ET
from io import BytesIO
from pathlib import Path

import requests

VL_SERVER = os.environ.get("VL_SERVER", "http://localhost:8083")
VL_MODEL  = os.environ.get("VL_MODEL", "/home/teles/llm-setup/models/uitars_hf")
# VL_MODEL_TYPE: "uitars" (default) or "qwen25vl"
VL_MODEL_TYPE = os.environ.get("VL_MODEL_TYPE", "uitars")
# OSWorld default resolution; UI-TARS outputs coords normalized [0-1000]
VL_SCREEN_W = int(os.environ.get("VL_SCREEN_W", "1920"))
VL_SCREEN_H = int(os.environ.get("VL_SCREEN_H", "1080"))
FARSCRY_BIN = os.environ.get("FARSCRY_BIN", "farscry")
SESSION_DIR = Path(os.environ.get("FARSCRY_SESSION_DIR", os.path.expanduser("~/.farscry/osworld")))

_A11Y_STATE_NS = "https://accessibility.ubuntu.example.org/ns/state"
_A11Y_COMP_NS  = "https://accessibility.ubuntu.example.org/ns/component"
_A11Y_VAL_NS   = "https://accessibility.ubuntu.example.org/ns/value"
_A11Y_ACT_NS   = "https://accessibility.ubuntu.example.org/ns/action"
_A11Y_TXT_NS   = "https://accessibility.ubuntu.example.org/ns/text"

INTERACTIVE_ROLES = {
    "button", "check-box", "combo-box", "entry", "link", "menu", "menuitem",
    "radio-button", "searchbox", "slider", "spin-button", "text", "textbox",
    "textarea", "textfield", "toggle-button", "push-button", "menu-item",
}

CONTENT_ROLES = {
    "paragraph", "table-cell", "table", "heading", "label",
    "static", "section", "document", "page",
}


def extract_semantic_state(xml_str: str, max_nodes: int = 60) -> dict:
    """
    Full semantic snapshot of the UI from AT-SPI tree.
    Returns structured data: interactive, content, values, actions.
    Works for any app that exposes AT-SPI. Zero hardcoding.
    """
    if not xml_str:
        return {"interactive": [], "content": [], "values": [], "actions": {}}
    try:
        root = ET.fromstring(xml_str)
    except ET.ParseError:
        return {"interactive": [], "content": [], "values": [], "actions": {}}

    interactive, content, values = [], [], []
    actions: dict[str, list[str]] = {}

    seen: set[str] = set()

    for node in root.iter():
        tag = node.tag.split("}")[-1] if "}" in node.tag else node.tag
        role = tag.lower()

        showing = node.get("{%s}showing" % _A11Y_STATE_NS, "false")
        if showing != "true":
            continue

        name = (node.get("name") or node.text or "").strip()
        if not name or name in seen:
            continue

        coord_str = node.get("{%s}screencoord" % _A11Y_COMP_NS, "")
        if not coord_str:
            continue
        try:
            x, y = map(int, _re.findall(r"-?\d+", coord_str)[:2])
        except (ValueError, IndexError):
            continue
        if x < 0 or y < 0:
            continue

        size_str = node.get("{%s}size" % _A11Y_COMP_NS, "")
        try:
            w, h = map(int, _re.findall(r"-?\d+", size_str)[:2])
            cx, cy = x + w // 2, y + h // 2
        except (ValueError, IndexError):
            cx, cy = x, y

        enabled = node.get("{%s}enabled" % _A11Y_STATE_NS, "true") == "true"
        checked = node.get("{%s}checked" % _A11Y_STATE_NS, "false") == "true"
        selected = node.get("{%s}selected" % _A11Y_STATE_NS, "false") == "true"
        expanded = node.get("{%s}expanded" % _A11Y_STATE_NS, "false") == "true"

        val = node.get("{%s}current_value" % _A11Y_VAL_NS, "") or \
              node.get("{%s}text" % _A11Y_TXT_NS, "")

        node_acts = []
        for attr, av in node.attrib.items():
            if _A11Y_ACT_NS in attr and av and av not in node_acts:
                node_acts.append(av)

        entry = {
            "role": role, "name": name[:60], "x": cx, "y": cy,
            "enabled": enabled,
        }
        if val:
            entry["value"] = val[:80]
        if checked:
            entry["checked"] = True
        if selected:
            entry["selected"] = True
        if expanded:
            entry["expanded"] = True

        window_chrome = cx > 1800 and cy < 50
        if not window_chrome:
            if role in INTERACTIVE_ROLES:
                interactive.append(entry)
            elif role in CONTENT_ROLES:
                content.append(entry)
                if val:
                    values.append({"name": name[:40], "value": val[:80], "x": cx, "y": cy})
            if node_acts:
                actions[name[:40]] = node_acts[:3]

        seen.add(name)
        if len(interactive) + len(content) >= max_nodes:
            break

    return {
        "interactive": interactive[:30],
        "content": content[:20],
        "values": values[:10],
        "actions": actions,
    }


def semantic_state_to_context(state: dict, task_keywords: list[str]) -> str:
    """
    Converts semantic state to model context.
    Filters by task relevance. Exposes actions, values, content.
    """
    parts = []

    relevant_interactive = [
        e for e in state["interactive"]
        if e.get("enabled", True) or any(k in e["name"].lower() for k in task_keywords)
    ]
    if relevant_interactive:
        lines = ["Interactive elements:"]
        for e in relevant_interactive[:20]:
            acts = state["actions"].get(e["name"][:40], [])
            act_str = f" [actions: {', '.join(acts[:2])}]" if acts else ""
            disabled_str = " [disabled]" if not e.get("enabled", True) else ""
            state_str = ""
            if e.get("checked"):  state_str += " [checked]"
            if e.get("selected"): state_str += " [selected]"
            if e.get("expanded"): state_str += " [expanded]"
            val_str = f" = \"{e['value']}\"" if e.get("value") else ""
            lines.append(
                f"  {e['role']:12} \"{e['name']}\"{val_str}"
                f"{disabled_str}{state_str}{act_str}"
                f" → pyautogui.click({e['x']}, {e['y']})"
            )
        parts.append("\n".join(lines))

    kw_content = [
        e for e in state["content"]
        if any(k in e["name"].lower() for k in task_keywords)
    ]
    if kw_content:
        lines = ["Relevant content on screen:"]
        for e in kw_content[:10]:
            val_str = f" = \"{e['value']}\"" if e.get("value") else ""
            lines.append(f"  {e['role']:12} \"{e['name']}\"{val_str} → pyautogui.click({e['x']}, {e['y']})")
        parts.append("\n".join(lines))

    if state["values"]:
        lines = ["Current field values:"]
        for v in state["values"][:5]:
            lines.append(f"  \"{v['name']}\" = \"{v['value']}\"")
        parts.append("\n".join(lines))

    return "\n\n".join(parts)


_GENERIC_TERMS = {
    "home", "user", "folder", "file", "open", "path", "data",
    "root", "local", "desktop", "document", "directory", "item",
}

def semantic_task_done(state: dict, task_keywords: list[str],
                       task_instr: str, initial_names: set[str]) -> bool:
    """
    Fires ONLY when specific new UI elements appear after actions.
    Uses long, unique keywords (>5 chars, not generic path terms).
    Prevents false positives for common path/file system terms.
    """
    if not task_keywords:
        return False

    specific_kw = [
        k for k in task_keywords
        if len(k) > 5 and k not in _GENERIC_TERMS
    ]
    if len(specific_kw) < 2:
        return False

    current_names = {
        e.get("name", "").lower()
        for e in state["interactive"] + state["content"]
    }
    new_names = current_names - initial_names

    if new_names:
        new_text = " ".join(new_names)
        new_matches = sum(1 for k in specific_kw if k in new_text)
        if new_matches >= min(3, len(specific_kw)):
            return True

    checked_values = " ".join(v.get("value", "").lower() for v in state["values"])
    if checked_values:
        val_matches = sum(1 for k in specific_kw if k in checked_values)
        return val_matches >= min(3, len(specific_kw))

    return False

SYSTEM_BASE = """You control an Ubuntu desktop via pyautogui. Output a single pyautogui statement.
Examples:
  pyautogui.click(850, 420)
  pyautogui.rightClick(1800, 900)
  pyautogui.doubleClick(400, 300)
  pyautogui.typewrite('hello world', interval=0.05)
  pyautogui.hotkey('ctrl', 'c')
  pyautogui.scroll(800, 500, clicks=3)
  DONE
  FAIL: reason

Key workflows:
- To rename a file/folder on desktop: rightClick the icon, then click Rename
- To open menus: click the menu bar item, then click the submenu item
- To type in a text field: click the field first, then typewrite

Output only the statement. Do not explain."""

SYSTEM_AUG = SYSTEM_BASE + """

Accessible UI elements are listed with exact coordinates. Use them for clicks.
If you receive a [SILENT_FAILURE] warning, try a completely different action."""


def detect_dialog_next_action(focused_els: list[dict], tried_names: set[str], task_instr: str = "") -> str:
    """
    Detects common dialog interaction patterns from AT-SPI state.
    Universal: file chooser, save dialog, open dialog, color picker, etc.
    Returns a concrete hint with exact coordinates when pattern found.
    """
    if not focused_els:
        return ""

    DISMISS = {"cancel", "close", "no", "quit", "abort", "back", "escape"}
    CONFIRM = {"open", "ok", "yes", "add", "save", "apply", "accept", "select",
               "choose", "confirm", "done", "next", "finish", "continue"}

    list_tried = [e for e in focused_els
                  if e["role"] in ("list-item", "table-cell", "table-row", "row")
                  and e["name"].lower() in tried_names]

    confirm_buttons = [
        e for e in focused_els
        if e["role"] in ("button", "push-button", "toggle-button")
        and e["name"].lower() not in tried_names
        and not any(d in e["name"].lower() for d in DISMISS)
    ]
    confirm_buttons.sort(key=lambda e: (
        0 if any(c in e["name"].lower() for c in CONFIRM) else 1,
        -e["x"]
    ))

    if list_tried and confirm_buttons:
        btn = confirm_buttons[0]
        item = list_tried[-1]
        return (
            f"You have selected \"{item['name']}\". "
            f"Now click the confirm button: \"{btn['name']}\" → pyautogui.click({btn['x']}, {btn['y']})"
        )

    _EXCL = {"activities", "applications", "overview", "search"}
    text_inputs = [e for e in focused_els
                   if e.get("role") in ("entry", "text", "textfield")
                   and e.get("enabled", True)
                   and e.get("name", "").lower() not in _EXCL
                   and e.get("y", 0) > 30]
    if text_inputs:
        inp = text_inputs[0]
        hint = (
            f"ACTIVE TEXT FIELD DETECTED: "
            f"'{inp['name']}' at ({inp['x']}, {inp['y']}) is ready for input. "
            f"Type the required text using pyautogui.typewrite()."
        )
        if confirm_buttons:
            btn = confirm_buttons[0]
            hint += f" Then confirm: pyautogui.click({btn['x']}, {btn['y']})"
        return hint

    return ""


def parse_focused_subtree(xml_str: str) -> list[dict]:
    """
    Returns interactive elements INSIDE the focused container.
    Traverses the AT-SPI XML tree hierarchically — same logic as
    farscry-a11y SQLite recursive CTE, but directly from XML.
    Universal: works for any app, dialog, menu, window.
    """
    if not xml_str:
        return []
    try:
        root = ET.fromstring(xml_str)
    except ET.ParseError:
        return []

    CONTAINER_ROLES = {"dialog", "frame", "window", "menu", "menubar",
                       "file-chooser", "color-chooser", "font-chooser", "alert"}

    focused_node = None
    for node in root.iter():
        role = (node.tag.split("}")[-1] if "}" in node.tag else node.tag).lower()
        focused = node.get("{%s}focused" % _A11Y_STATE_NS, "false") == "true"
        showing = node.get("{%s}showing" % _A11Y_STATE_NS, "false") == "true"
        if focused and showing and role in CONTAINER_ROLES:
            focused_node = node
            break

    if focused_node is None:
        return []

    results: list[dict] = []
    for node in focused_node.iter():
        tag = node.tag.split("}")[-1] if "}" in node.tag else node.tag
        role = tag.lower()
        if role not in INTERACTIVE_ROLES:
            continue
        showing = node.get("{%s}showing" % _A11Y_STATE_NS, "false") == "true"
        if not showing:
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
        enabled = node.get("{%s}enabled" % _A11Y_STATE_NS, "true") == "true"
        results.append({
            "role": role, "name": name[:60],
            "x": x + w // 2, "y": y + h // 2,
            "enabled": enabled,
        })
        if len(results) >= 20:
            break

    return results


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
        enabled = node.get("{%s}enabled" % _A11Y_STATE_NS, "true") == "true"
        sensitive = node.get("{%s}sensitive" % _A11Y_STATE_NS, "true") == "true"
        focused_flag = node.get("{%s}focused" % _A11Y_STATE_NS, "false") == "true"
        entry = {
            "role": role, "name": name[:60],
            "x": cx, "y": cy,
            "enabled": enabled and sensitive,
            "in_modal": any(m in name for m in modal_names),
        }
        if focused_flag:
            entry["focused"] = True
        elements.append(entry)

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


APP_CONTEXT = {
    "libreoffice_writer": """\
LibreOffice Writer — common operations:
- Change text color: select text → Format → Character → Font Effects → Font Color
- Select all: Ctrl+A. Select word: double-click. Select line: triple-click.
- Find & Replace: Ctrl+H (supports regex for bulk changes)
- Run macro for bulk ops: Tools → Macros → Organize Basic Macros
- Bold/Italic/Underline: Ctrl+B / Ctrl+I / Ctrl+U
- Save: Ctrl+S. Save as: Ctrl+Shift+S.""",

    "libreoffice_calc": """\
LibreOffice Calc — common operations:
- Format cell: right-click → Format Cells
- Enter formula: click cell → type = then formula (e.g. =SUM(A1:A10))
- Select range: click first cell → Shift+click last
- Sort: select range → Data → Sort
- AutoFill: drag bottom-right corner of cell
- Save: Ctrl+S.""",

    "libreoffice_impress": """\
LibreOffice Impress — common operations:
- Add slide: right-click panel → Insert Slide
- Edit text: double-click text box
- Insert image: Insert → Image
- Slide layout: Slide → Layout
- Export to PDF/video: File → Export As
- Save: Ctrl+S.""",

    "vs_code": """\
VS Code — common operations:
- Open file: Ctrl+P → type filename
- Open folder: File → Open Folder (or drag folder)
- Command palette: Ctrl+Shift+P
- Integrated terminal: Ctrl+backtick
- Find in files: Ctrl+Shift+F
- Go to line: Ctrl+G
- Add to workspace: drag folder to explorer panel.""",

    "thunderbird": """\
Thunderbird — common operations:
- Compose new email: Ctrl+N or click Write
- Reply: Ctrl+R. Forward: Ctrl+L.
- Attach file: Attach button or Insert → Attachment
- Create folder: right-click account/folder → New Folder
- Mark as read/unread: M key
- Search emails: Ctrl+K.""",

    "gimp": """\
GIMP — common operations:
- Open image: File → Open
- Export/Save: File → Export As (use overwrite for same format)
- Select region: R for rectangle, E for ellipse, F for free select
- Fill selection: Shift+B (bucket fill) or Edit → Fill with FG Color
- Change colors: Colors menu (Brightness, Hue, Levels)
- Undo: Ctrl+Z. Redo: Ctrl+Y.""",
}


def get_app_context(elements: list[dict]) -> str:
    names = " ".join(e["name"].lower() for e in elements)
    if "writer" in names or ("libreoffice" in names and "calc" not in names and "impress" not in names):
        return APP_CONTEXT["libreoffice_writer"]
    if "calc" in names or "spreadsheet" in names:
        return APP_CONTEXT["libreoffice_calc"]
    if "impress" in names or "presentation" in names:
        return APP_CONTEXT["libreoffice_impress"]
    if "visual studio code" in names or "vscode" in names or "explorer" in names:
        return APP_CONTEXT["vs_code"]
    if "thunderbird" in names or "write:" in names or "inbox" in names:
        return APP_CONTEXT["thunderbird"]
    if "gimp" in names or "toolbox" in names:
        return APP_CONTEXT["gimp"]
    return ""


_STOP = {
    "the","a","an","is","are","was","were","can","you","me","my","i","it",
    "this","that","for","to","in","on","at","of","and","or","but","with",
    "from","do","want","need","help","please","file","open","click","use",
    "make","get","go","have","will","your","into","some","also","then","when",
    "there","these","those","they","them","their","what","which","who","how",
}

def extract_keywords(task_text: str) -> list[str]:
    words = _re.findall(r'\b[a-zA-Z]{3,}\b', task_text.lower())
    seen: set[str] = set()
    out = []
    for w in words:
        if w not in _STOP and w not in seen:
            seen.add(w)
            out.append(w)
        if len(out) >= 12:
            break
    return out


def build_menu_tree(elements: list[dict]) -> dict[str, str]:
    MENU_ROLES = {
        "menu", "menuitem", "menu-item", "menubar",
        "push-button", "toggle-button", "button",
        "check-box", "radio-button", "combo-box",
    }
    menu_els = [e for e in elements
                if e.get("role", "") in MENU_ROLES and e.get("name", "").strip()]

    root_menus = [e for e in menu_els if e.get("y", 999) < 100]
    other_els   = [e for e in menu_els if e.get("y", 999) >= 100]

    paths: dict[str, str] = {}
    for e in root_menus:
        paths[e["name"].lower()] = e["name"]

    for e in other_els:
        name = e["name"]
        if not name:
            continue
        if root_menus:
            closest = min(root_menus, key=lambda r: abs(r.get("x", 0) - e.get("x", 0)))
            paths[name.lower()] = f"{closest['name']} → {name}"
        else:
            paths[name.lower()] = name

    return paths


def build_dynamic_context(elements: list[dict], task_text: str) -> str:
    if not elements:
        return ""

    keywords = extract_keywords(task_text)
    if not keywords:
        return ""

    tree = build_menu_tree(elements)

    relevant: list[str] = []
    seen: set[str] = set()
    for kw in keywords:
        for el_name, path in tree.items():
            if kw in el_name and path not in seen:
                relevant.append(path)
                seen.add(path)

    if not relevant:
        return ""

    lines = ["Relevant UI paths found for this task:"]
    for path in relevant[:8]:
        lines.append(f"  - {path}")
    lines.append("")
    lines.append("Use these paths. Click menu items in order.")
    return "\n".join(lines)


def detect_preconditions(elements: list[dict], task_keywords: list[str]) -> str:
    disabled_relevant = [
        e for e in elements
        if not e.get("enabled", True)
        and any(kw in e["name"].lower() for kw in task_keywords)
    ]

    if not disabled_relevant:
        return ""

    enabled_actions = [
        e for e in elements
        if e.get("enabled", True)
        and e.get("role") in ("button", "push-button", "menuitem", "menu", "toggle-button")
        and e.get("name", "").strip()
    ]

    disabled_names = [e["name"] for e in disabled_relevant[:3]]
    available_names = [e["name"] for e in enabled_actions[:5]]

    msg = (
        f"NOTE: {', '.join(disabled_names)} are currently disabled. "
        f"Something may need to happen first. "
    )
    if available_names:
        msg += f"Currently available: {', '.join(available_names)}."
    return msg


def _safe_env_step(env, action: str, pause: float = 0.5, timeout: int = 20):
    """env.step with a hard timeout — returns None on timeout or error."""
    result = [None]
    def _handler(signum, frame):
        raise TimeoutError(f"env.step timed out ({timeout}s): {action[:40]}")
    old = signal.signal(signal.SIGALRM, _handler)
    signal.alarm(timeout)
    try:
        result[0] = env.step(action, pause=pause)
    except TimeoutError:
        pass
    except Exception:
        pass
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, old)
    return result[0]


def explore_and_build_context(
    a11y_elements: list[dict], task_instr: str, env, needs_a11y: bool
) -> str:
    root_menus = [
        e for e in a11y_elements
        if e.get("role") in ("menu", "menuitem", "menu-item", "menubar")
        and e.get("y", 999) < 80
        and e.get("name", "").strip()
    ]

    full_vocab: dict[str, str] = {}

    for menu in root_menus[:5]:
        try:
            res = _safe_env_step(env, f"pyautogui.click({menu['x']}, {menu['y']})", pause=0.4)
            if res is None:
                continue
            obs_m = res[0]
            a11y_xml = obs_m.get("accessibility_tree") if (obs_m and needs_a11y) else None
            if a11y_xml:
                sub_els = parse_a11y_tree(a11y_xml)
                for item in sub_els:
                    ix, iy = item.get("x", 0), item.get("y", 0)
                    if iy > 80 and item.get("name", "").strip() and not (ix > 1800 and iy < 50):
                        name = item["name"].lower()
                        full_vocab[name] = (
                            f"{menu['name']} → {item['name']} "
                            f"→ click({ix}, {iy})"
                        )
            _safe_env_step(env, "pyautogui.press('escape')", pause=0.2)
        except Exception:
            continue

    if not full_vocab:
        return build_dynamic_context(a11y_elements, task_instr)

    keywords = extract_keywords(task_instr)
    matches: list[str] = []
    seen: set[str] = set()
    for kw in keywords:
        for vname, path in full_vocab.items():
            if kw in vname and path not in seen:
                matches.append(path)
                seen.add(path)

    if not matches:
        return ""

    lines = ["Relevant paths found by exploring menus:"]
    for path in matches[:6]:
        lines.append(f"  - {path}")
    lines.append("")
    lines.append("Use these paths. Click menu items in order.")
    return "\n".join(lines)


def format_a11y_context(elements: list[dict],
                        with_app_context: bool = False,
                        with_dynamic_context: bool = False,
                        task_text: str = "") -> str:
    if not elements:
        return ""
    lines = []
    if with_dynamic_context and task_text:
        dyn = build_dynamic_context(elements, task_text)
        if dyn:
            lines.append(dyn)
            lines.append("")
    elif with_app_context:
        app_ctx = get_app_context(elements)
        if app_ctx:
            lines.append("App procedures (HOW to use this app):")
            lines.append(app_ctx)
            lines.append("")
    lines.append("UI elements (USE THESE EXACT COORDINATES):")
    for e in elements:
        lines.append(
            f"  {e['role']:12s} \"{e['name']}\" → pyautogui.click({e['x']}, {e['y']})"
        )
    return "\n".join(lines)


def encode(path: str) -> str:
    with open(path, "rb") as f:
        return base64.b64encode(f.read()).decode()


def vl_checkpoint(screenshot_path: str, task: str, temperature: float = 0.0) -> bool:
    # UI-TARS format: image first
    messages = [
        {"role": "user", "content": [
            {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{encode(screenshot_path)}"}},
            {"type": "text", "text": f"Task: {task}\n\nHas this task been completed? Answer only YES or NO."},
        ]},
    ]
    try:
        r = requests.post(f"{VL_SERVER}/v1/chat/completions",
                          json={"model": VL_MODEL, "messages": messages,
                                "max_tokens": 5, "temperature": temperature},
                          timeout=(10, 60))
        r.raise_for_status()
        answer = r.json()["choices"][0]["message"]["content"].strip().upper()
        return answer.startswith("YES")
    except Exception:
        return False


def annotate_screenshot(
    shot_path: str,
    tried_coords: list[tuple[int, int]],
    untried_elements: list[dict],
) -> str:
    """Draw visual feedback on screenshot:
    - Red circle where model has been clicking (stuck)
    - Green circles for untried elements
    Returns path to annotated image."""
    try:
        from PIL import Image, ImageDraw
        img = Image.open(shot_path).convert("RGB")
        draw = ImageDraw.Draw(img, "RGBA")

        for x, y in tried_coords[-6:]:
            r = 22
            draw.ellipse([x-r, y-r, x+r, y+r], outline=(220, 30, 30, 255), width=4)
            draw.line([x-r, y-r, x+r, y+r], fill=(220, 30, 30, 220), width=3)
            draw.line([x+r, y-r, x-r, y+r], fill=(220, 30, 30, 220), width=3)

        for e in untried_elements[:4]:
            x, y = e["x"], e["y"]
            r = 18
            draw.ellipse([x-r, y-r, x+r, y+r], outline=(30, 200, 60, 255), width=4)
            draw.ellipse([x-4, y-4, x+4, y+4], fill=(30, 200, 60, 255))

        out = shot_path.replace(".png", "_annotated.png")
        img.save(out)
        return out
    except Exception:
        return shot_path


def vl_call_text_only(task: str, a11y_context: str, stuck_hint: str = "") -> str:
    """VL call WITHOUT screenshot — used when visual loop detected.
    Forces model to reason from a11y structure only, breaking visual anchor."""
    parts = [f"Task: {task}"]
    if stuck_hint:
        parts.append(stuck_hint)
    if a11y_context:
        parts.append(a11y_context)

    text = "\n".join(parts) + "\nAction:"
    messages = [{"role": "user", "content": text}]
    try:
        r = requests.post(f"{VL_SERVER}/v1/chat/completions",
                          json={"model": VL_MODEL, "messages": messages,
                                "max_tokens": 128, "temperature": 0.3},
                          timeout=(10, 60))
        r.raise_for_status()
        return r.json()["choices"][0]["message"]["content"].strip()
    except Exception:
        return ""


def vl_call(screenshot_path: str, task: str, history: list,
            a11y_context: str = "", sf_feedback: str = "",
            temperature: float = 0.0,
            no_image: bool = False) -> str:
    parts = [f"Task: {task}"]
    if sf_feedback:
        parts.append(sf_feedback)
    if a11y_context:
        parts.append(a11y_context)

    text_content = "\n".join(parts)

    if no_image or not screenshot_path or not os.path.exists(screenshot_path):
        # Text-only: model copies coords from a11y_context
        if VL_MODEL_TYPE == "qwen25vl":
            messages = [
                {"role": "system", "content": SYSTEM_AUG},
                {"role": "user", "content": text_content},
            ]
        else:
            # UI-TARS: prefill "Action:"
            messages = [
                {"role": "user", "content": text_content},
                {"role": "assistant", "content": "Action:"},
            ]
    elif VL_MODEL_TYPE == "qwen25vl":
        # Qwen2.5-VL: system prompt + image in user message
        # Model outputs pyautogui statements directly (no start_box format)
        messages = [
            {"role": "system", "content": SYSTEM_AUG if a11y_context else SYSTEM_BASE},
            {"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{encode(screenshot_path)}"}},
                {"type": "text", "text": text_content},
            ]},
        ]
    else:
        # UI-TARS: image BEFORE text, then "Thought:" prefill
        messages = [
            {"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{encode(screenshot_path)}"}},
                {"type": "text", "text": text_content},
            ]},
            {"role": "assistant", "content": "Thought:"},
        ]

    def _call(msgs, max_tok=256):
        resp = requests.post(f"{VL_SERVER}/v1/chat/completions",
                             json={"model": VL_MODEL, "messages": msgs,
                                   "max_tokens": max_tok, "temperature": temperature},
                             timeout=(10, 60))
        resp.raise_for_status()
        return resp.json()["choices"][0]["message"]["content"].strip()

    def _extract(text):
        if "Action:" in text:
            for line in text.split("\n"):
                if line.strip().startswith("Action:"):
                    return line.strip()[len("Action:"):].strip()
        if "start_box=" in text:
            for line in text.split("\n"):
                if "start_box=" in line:
                    return line.strip()
        return text.splitlines()[0].strip() if text else text

    raw = _call(messages)
    result = _extract(raw)

    # Qwen2.5-VL outputs pyautogui directly — no retry needed if it's there
    if VL_MODEL_TYPE == "qwen25vl" and not no_image:
        return result

    # UI-TARS: if visual mode returned prose without an action, retry
    if not no_image and "start_box=" not in result and "pyautogui." not in result:
        retry_msgs = [
            {"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{encode(screenshot_path)}"}},
                {"type": "text", "text": text_content + "\nRespond with ONLY the action, no explanation."},
            ]},
            {"role": "assistant", "content": "Action:"},
        ]
        raw2 = _call(retry_msgs, max_tok=64)
        result2 = _extract(raw2)
        if "start_box=" in result2 or "pyautogui." in result2:
            return result2

    return result


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


def action_to_pyautogui(raw: str, no_image: bool = False) -> str | None:
    raw = raw.strip().splitlines()[0].strip()
    # Normalize malformed pyautogui prefixes: "pyautogui hotkey(...)" → "hotkey(...)"
    raw = _re.sub(r'^pyautogui\s+', '', raw)
    if raw.upper().startswith("DONE") or raw.upper().startswith("FAIL"):
        return None
    # UI-TARS native formats: click/rightClick/doubleClick/type/scroll/hotkey/drag
    if "start_box=" in raw:
        m = _re.search(r"start_box=.?\(?(\d+)[,\s]+(\d+)", raw)
        if m:
            rx, ry = int(m.group(1)), int(m.group(2))
            # no_image=True: model copied absolute coords from a11y_context → use directly
            # no_image=False (visual): coords are normalized [0-1000] → scale to screen
            # Fallback heuristic: >1000 is always absolute regardless of mode
            if no_image or rx > 1000 or ry > 1000:
                x, y = rx, ry
            else:
                x = round(rx / 1000 * VL_SCREEN_W)
                y = round(ry / 1000 * VL_SCREEN_H)
            raw_l = raw.lower()
            if any(raw_l.startswith(p) for p in ("rightclick", "right_click", "right click", "right-click")):
                return f"pyautogui.rightClick({x}, {y})"
            if any(raw_l.startswith(p) for p in ("doubleclick", "double_click", "double click", "double-click")):
                return f"pyautogui.doubleClick({x}, {y})"
            return f"pyautogui.click({x}, {y})"
        return None
    # UI-TARS type action: type(content='...')
    if raw.startswith("type(") or raw.startswith("Type("):
        m = _re.search(r"type\(content=['\"](.+?)['\"]\)", raw, _re.IGNORECASE)
        if m:
            text = m.group(1).replace("'", "\\'")
            return f"pyautogui.typewrite('{text}', interval=0.05)"
        return None
    # UI-TARS hotkey: hotkey(key='ctrl c') or hotkey(key='ctrl', modifier='shift')
    if raw.startswith("hotkey(") or raw.startswith("Hotkey("):
        m = _re.search(r"key=['\"]([^'\"]+)['\"]", raw)
        if m:
            keys = m.group(1).split()
            keys_str = ", ".join(f"'{k}'" for k in keys)
            return f"pyautogui.hotkey({keys_str})"
        return None
    # UI-TARS scroll action
    if raw.startswith("scroll("):
        m = _re.search(r"start_box=.?\(?(\d+)[,\s]+(\d+).+?direction=['\"](\w+)['\"]", raw)
        if m:
            x = round(int(m.group(1)) / 1000 * VL_SCREEN_W)
            y = round(int(m.group(2)) / 1000 * VL_SCREEN_H)
            direction = m.group(3).lower()
            clicks = -3 if direction == "down" else 3
            return f"pyautogui.scroll({x}, {y}, clicks={clicks})"
        return None
    # Legacy formats
    if raw.startswith("click(") and "pyautogui." not in raw:
        m = _re.search(r"click\((\d+),\s*(\d+)\)", raw)
        if m:
            return f"pyautogui.click({m.group(1)}, {m.group(2)})"
    if "pyautogui." not in raw:
        return None
    return raw


ESCAPE_LADDER = [
    "pyautogui.press('escape')",
    "pyautogui.hotkey('ctrl', 'z')",
    "pyautogui.press('escape')",
    "pyautogui.hotkey('ctrl', 'z')",
    "pyautogui.click(960, 540)",
]


def _parse_click_coords(action_str: str) -> tuple[int, int] | None:
    m = _re.search(r'click\((\d+),\s*(\d+)\)', action_str)
    if m:
        return int(m.group(1)), int(m.group(2))
    return None


def _phash_hamming(id1: str, id2: str) -> int:
    try:
        h1 = int(id1.replace("phash:", "").replace("0x", ""), 16)
        h2 = int(id2.replace("phash:", "").replace("0x", ""), 16)
        return bin(h1 ^ h2).count("1")
    except Exception:
        return 0


def explore_dialogs(a11y_elements: list[dict], env, needs_a11y: bool) -> dict[str, str]:
    tab_roles = {"page-tab", "tab", "page-tab-list", "pagetab", "tabitem"}
    tabs = [
        e for e in a11y_elements
        if e.get("role", "") in tab_roles and e.get("name", "").strip()
    ]
    if not tabs:
        return {}

    vocab: dict[str, str] = {}
    for tab in tabs[:5]:
        try:
            res = _safe_env_step(env, f"pyautogui.click({tab['x']}, {tab['y']})", pause=0.3)
            if res is None:
                continue
            obs_t = res[0]
            a11y_xml = obs_t.get("accessibility_tree") if (obs_t and needs_a11y) else None
            if a11y_xml:
                sub_els = parse_a11y_tree(a11y_xml)
                for e in sub_els:
                    name = e.get("name", "").lower().strip()
                    if name:
                        vocab[name] = (
                            f"{tab['name']} tab → {e['name']} "
                            f"→ click({e['x']}, {e['y']})"
                        )
        except Exception:
            continue
    return vocab


def run_task(env, task_id: str, task_instr: str, task_config: dict,
             max_steps: int, augmented: bool, out_dir: Path,
             a11y_only: bool = False, with_context: bool = False,
             with_dynamic: bool = False, with_submenu: bool = False,
             needs_a11y: bool = False) -> dict:
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
    action_coord_history: list[tuple[int, int]] = []
    action_str_history: list[str] = []   # tracks all actions for non-coord loops
    micro_loop_count = 0
    submenu_context: str = ""
    tried_names: set[str] = set()  # init here to avoid NameError in augmented mode

    obs = env.reset(task_config=task_config)

    initial_sem_names: set[str] = set()
    ever_seen_names:   set[str] = set()
    if a11y_only and obs and needs_a11y:
        init_xml = obs.get("accessibility_tree")
        if init_xml:
            init_sem = extract_semantic_state(init_xml)
            initial_sem_names = {
                e.get("name", "").lower()
                for e in init_sem["interactive"] + init_sem["content"]
            }
            ever_seen_names = set(initial_sem_names)

    if with_submenu and obs:
        a11y_xml = obs.get("accessibility_tree")
        if a11y_xml:
            init_els = parse_a11y_tree(a11y_xml)
            submenu_context = explore_and_build_context(init_els, task_instr, env, needs_a11y)
            if submenu_context:
                print(f"  [submenu] built context: {len(submenu_context)} chars")

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
            task_kw  = extract_keywords(task_instr)

            elements  = parse_a11y_tree(a11y_xml) if a11y_xml else []
            sem_state = extract_semantic_state(a11y_xml) if a11y_xml else \
                        {"interactive": [], "content": [], "values": [], "actions": {}}

            focused_els = [e for e in elements if e.get("focused")]
            if focused_els:
                print(f"  [s{step:02d}] FOCUSED: {[(e['role'], e['name'][:30]) for e in focused_els]}")

            live_ctx  = semantic_state_to_context(sem_state, task_kw)
            precond   = detect_preconditions(elements, task_kw)

            dyn_paths = build_dynamic_context(elements, task_instr)
            if dyn_paths and dyn_paths != submenu_context:
                submenu_context = dyn_paths
                print(f"  [s{step:02d}] CTX_LIVE {len(elements)}el")

            # SINAL 1 — novos elementos desde o step anterior
            current_names = {
                e.get("name", "").lower()
                for e in elements + sem_state["content"]
                if e.get("name", "").strip()
            }
            truly_new = current_names - ever_seen_names
            ever_seen_names |= current_names
            appeared_signal = ""
            if truly_new and len(truly_new) >= 2:
                new_labels = [n for n in truly_new if len(n) > 2][:6]
                if new_labels:
                    # Build appeared signal WITH coordinates so model can click directly
                    appeared_lines = []
                    for n in new_labels:
                        # Find matching element in current elements list
                        el = next((e for e in elements if e["name"].lower() == n.lower()), None)
                        if el:
                            appeared_lines.append(
                                f"  - \"{el['name']}\" → pyautogui.click({el['x']}, {el['y']})"
                            )
                        else:
                            appeared_lines.append(f"  - \"{n}\"")
                    appeared_signal = (
                        "⚡ NEW elements appeared — click one of these:\n"
                        + "\n".join(appeared_lines)
                    )
                    print(f"  [s{step:02d}] APPEARED: {new_labels[:3]}")

            # SINAL 2 — elementos que o modelo ainda não tentou
            tried_names: set[str] = set()
            for coord in action_coord_history[-10:]:
                cx, cy = coord
                for e in elements:
                    if abs(e["x"] - cx) < 20 and abs(e["y"] - cy) < 20:
                        tried_names.add(e["name"].lower())
            untried = [
                e for e in elements
                if e.get("enabled", True)
                and e["name"].lower() not in tried_names
                and e["name"].strip()
            ]
            untried_signal = ""
            if untried and len(tried_names) >= 2:
                untried_signal = (
                    "Elements you have NOT yet tried:\n"
                    + "\n".join(
                        f"  - {e['role']} \"{e['name']}\" → pyautogui.click({e['x']}, {e['y']})"
                        for e in untried[:5]
                    )
                )
                print(f"  [s{step:02d}] UNTRIED: {[e['name'] for e in untried[:3]]}")

            if precond:
                print(f"  [s{step:02d}] precond: {precond[:70]}")

            # Active text field detection — tell the model, let it decide what to type
            # Only real text inputs: deep in the UI (y>100), not toolbar/taskbar elements
            # Text input detection: entry/text/textfield roles
            # Exclude: known buttons/widgets by name, toolbar height, single chars
            _SHELL_EXCL = {"activities", "applications", "overview",
                           "new document", "open windows", "close window",
                           "quit", "close", "cancel", "ok", "yes", "no",
                           "search", "find"}
            entry_fields = [
                e for e in elements
                if e.get("role") in ("entry", "text", "textfield")
                and e.get("enabled", True)
                and e.get("name", "").lower() not in _SHELL_EXCL
                and len(e.get("name", "")) > 2
                and e.get("y", 0) > 100
                # Extra: exclude names that look like menu/button labels
                and not e.get("name", "").lower().startswith(("open", "new ", "save", "view"))
            ]
            text_input_hint = ""
            if entry_fields:
                field = entry_fields[0]
                last_act_h = action_str_history[-1] if action_str_history else ""
                if "typewrite" in last_act_h:
                    # typewrite done → press return
                    text_input_hint = (
                        f"pyautogui.press('return') — confirm what was typed."
                    )
                else:
                    # Entry field open — hint must NOT contain the text to type
                    # (model will literally typewrite the hint text otherwise)
                    text_input_hint = (
                        f"editable field '{field['name']}' is open at "
                        f"({field['x']}, {field['y']}). "
                        f"use pyautogui.typewrite() with the value from the task."
                    )
                print(f"  [s{step:02d}] ENTRY_FIELD: '{field['name']}' → {text_input_hint[:50]}")

            # Direct scan: find task-keyword elements in current AT-SPI tree
            # This catches cases where APPEARED missed them (timing/AT-SPI latency)
            direct_hits = [
                e for e in elements
                if any(k in e["name"].lower() for k in task_kw)
                and e.get("enabled", True)
                and e["name"].strip()
            ]
            direct_hint = ""
            if direct_hits and not appeared_signal and not text_input_hint:
                direct_hint = (
                    "Relevant elements found — use one of these:\n"
                    + "\n".join(
                        f"  - \"{e['name']}\" → pyautogui.click({e['x']}, {e['y']})"
                        for e in direct_hits[:5]
                    )
                )

            ctx_parts = []
            # text_input_hint has highest priority — overrides everything
            if text_input_hint:
                ctx_parts.append(text_input_hint)
            elif appeared_signal:
                ctx_parts.append(appeared_signal)
            elif direct_hint:
                ctx_parts.append(direct_hint)
            elif untried_signal and len(tried_names) >= 3:
                ctx_parts.append(untried_signal)
            if precond:
                ctx_parts.append(precond)
            if submenu_context and not appeared_signal and not direct_hint and not text_input_hint:
                ctx_parts.append(submenu_context)
            if live_ctx and not text_input_hint:
                ctx_parts.append(live_ctx)
            a11y_context = "\n\n".join(ctx_parts)

            n_content = len(sem_state["content"])
            n_vals    = len(sem_state["values"])
            matched   = sum(1 for k in task_kw if any(
                k in e["name"].lower() for e in elements + sem_state["content"]
            ))
            # Text input is always a clear signal — force no_image mode
            if text_input_hint:
                matched = max(matched, 1)
            print(f"  [ctx] inter={len(elements)}  content={n_content}  vals={n_vals}  matched={matched}")

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
                _r = _safe_env_step(env, escape_action, pause=1.0)
                obs = _r[0] if _r else obs
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

        temp = min(0.1 * total_escapes, 0.7) if total_escapes > 0 else 0.0
        try:
            n_elements = len(elements) if (augmented or a11y_only) else 0
        except NameError:
            n_elements = 0
        print(f"  [s{step:02d}] a11y={n_elements}el  csf={consecutive_sf}  esc={total_escapes}  temp={temp:.1f}")
        # no_image=True when >=1 matched keyword: model copies absolute coords from a11y_context.
        # no_image=False (visual) only when matched=0: model uses screenshot freely.
        use_no_image = a11y_only and matched >= 1
        if use_no_image:
            print(f"  [s{step:02d}] NO-IMAGE mode (matched={matched} ≥ 1)")
        # When matched=0: visual mode, no a11y_context (sidebar elements distract model).
        # When matched>=1: no_image mode with a11y_context as the only signal.
        effective_ctx = a11y_context if use_no_image else ""

        # Auto-actions: deterministic state machine for text field interactions
        # Bypasses model uncertainty for well-defined sequences
        last_act = action_str_history[-1] if action_str_history else ""
        prev_act = action_str_history[-2] if len(action_str_history) >= 2 else ""
        auto_clean = None
        auto_done = False
        if entry_fields:
            if "typewrite" in last_act:
                auto_clean = "pyautogui.press('return')"
                m_typed = _re.search(r"typewrite\('([^']+)'", last_act)
                typed = m_typed.group(1) if m_typed else ""
                field = entry_fields[0]
                field_sem = next(
                    (e for e in sem_state["interactive"]
                     if abs(e["x"] - field["x"]) < 15 and abs(e["y"] - field["y"]) < 15),
                    None
                )
                field_value = (field_sem.get("value", "") if field_sem else "") or field.get("name", "")
                value_match = bool(typed) and typed in field_value
                if value_match:
                    auto_done = True
                    print(f"  [s{step:02d}] AUTO: typewrite done → press return + DONE ('{typed}'~'{field_value[:25]}')")
                else:
                    print(f"  [s{step:02d}] AUTO: press return (no match: '{typed[:20]}' not in '{field_value[:25]}')")
            # Note: no auto ctrl+a — GNOME rename dialog auto-selects text on open

        if auto_clean:
            clean = auto_clean
        else:
            raw = vl_call(shot_path, task_instr, history,
                          a11y_context=effective_ctx, sf_feedback=sf_feedback,
                          temperature=temp,
                          no_image=use_no_image)
            clean = raw.strip().splitlines()[0].strip()
        print(f"  [s{step:02d}] model → {clean[:80]}")
        history.append({"role": "assistant", "content": clean})

        if clean.upper().startswith("DONE"):
            env.step("DONE", pause=0.5)
            break
        if clean.upper().startswith("FAIL"):
            env.step("FAIL", pause=0.5)
            break

        action_str = action_to_pyautogui(clean, no_image=use_no_image)
        if not action_str:
            break

        # After press(return) on entry_field: wait for async state change then check done
        if "press('return')" in action_str and entry_fields:
            time.sleep(1.5)

        # Timeout guard: env.step can block indefinitely if VM freezes
        def _timeout_handler(signum, frame):
            raise TimeoutError("env.step timed out after 45s")
        signal.signal(signal.SIGALRM, _timeout_handler)
        signal.alarm(45)
        try:
            obs, reward, done, info = env.step(action_str, pause=0.5)
        except TimeoutError:
            print(f"  [s{step:02d}] TIMEOUT — env.step blocked 45s, breaking")
            break
        finally:
            signal.alarm(0)
        agent_step += 1

        # If auto_done: entry field confirmed — stop and let evaluator judge
        if auto_done:
            print(f"  [s{step:02d}] AUTO_DONE: entry field confirmed, stopping")
            done = True

        # Track all actions (for non-coord loops like repeated hotkeys)
        action_str_history.append(action_str)
        if len(action_str_history) >= 5:
            recent_strs = action_str_history[-5:]
            if len(set(recent_strs)) == 1:  # same action 5x in a row
                micro_loop_count += 1
                total_escapes += 1
                print(f"  [s{step:02d}] ACTION-LOOP: '{action_str[:40]}' repeated 5x → escape")
                # Clear history so next escape ladder step is different
                action_str_history.clear()

        coords = _parse_click_coords(action_str)
        if coords:
            action_coord_history.append(coords)
            if len(action_coord_history) >= 6:
                recent = action_coord_history[-6:]
                buckets = {(round(x / 10) * 10, round(y / 10) * 10) for x, y in recent}
                if len(buckets) <= 3:
                    micro_loop_count += 1
                    if micro_loop_count >= 2:
                        total_escapes += 1
                        a11y_xml_now = obs.get("accessibility_tree") if obs and needs_a11y else None
                        focused_els = parse_focused_subtree(a11y_xml_now) if a11y_xml_now else []
                        focused_untried = [
                            e for e in focused_els
                            if e.get("enabled", True)
                            and e["name"].lower() not in tried_names
                        ] if focused_els else [
                            e for e in elements
                            if e.get("enabled", True)
                            and e["name"].lower() not in tried_names
                        ]
                        annotated = annotate_screenshot(
                            shot_path,
                            tried_coords=list(action_coord_history[-6:]),
                            untried_elements=focused_untried[:4],
                        )
                        src = "focused container" if focused_els else "fallback all"
                        dialog_hint = detect_dialog_next_action(focused_els, tried_names, task_instr)
                        if dialog_hint:
                            print(f"  [s{step:02d}] DIALOG PATTERN: {dialog_hint[:80]}")
                        print(f"  [s{step:02d}] annotated: {len(focused_untried)} untried from {src}")
                        print(f"  [s{step:02d}] MICRO-LOOP → annotated screenshot (🔴 tried, 🟢 untried)")
                        sf_feedback = (
                            (dialog_hint + "\n\n" if dialog_hint else "")
                            + "RED X marks = where you have been clicking (not working).\n"
                            + "GREEN circles = elements you have NOT tried yet.\n"
                            + "Click a GREEN circle or follow the instruction above."
                        )
                        shot_path = annotated
                else:
                    micro_loop_count = 0

        shot_after = str(out_dir / f"{session_id}-s{step:02d}b.png")
        save_screenshot(obs, shot_after)
        # Timeout guard for entire post-step analysis block (farscry + vl_checkpoint)
        signal.signal(signal.SIGALRM, lambda s, f: (_ for _ in ()).throw(TimeoutError("post-step timeout")))
        signal.alarm(70)
        try:
          if os.path.exists(shot_after) and (a11y_only or augmented):
            state_after, _ = farscry_state(shot_after)
            if a11y_only and obs and needs_a11y:
                new_xml = obs.get("accessibility_tree")
                if new_xml:
                    new_sem = extract_semantic_state(new_xml)
                    if semantic_task_done(new_sem, task_kw, task_instr, initial_sem_names):
                        print(f"  [s{step:02d}] SEMANTIC_DONE: task keywords found in UI state")
                        done = True
            if not done and state_before and state_after and state_before != state_after:
                hamming = _phash_hamming(state_before, state_after)
                if hamming > 5:
                    if vl_checkpoint(shot_after, task_instr):
                        print(f"  [s{step:02d}] CHECKPOINT (Δ={hamming}): done → stopping early")
                        done = True
                    elif a11y_only and obs and needs_a11y:
                        new_a11y_xml = obs.get("accessibility_tree")
                        if new_a11y_xml:
                            new_els = parse_a11y_tree(new_a11y_xml)
                            prev_count = len(elements) if "elements" in dir() else 0
                            new_dyn = build_dynamic_context(new_els, task_instr)
                            new_precond = detect_preconditions(
                                new_els, extract_keywords(task_instr)
                            )
                            updated = new_dyn or new_precond
                            if updated:
                                submenu_context = new_dyn
                                if new_precond:
                                    sf_feedback = new_precond
                                print(
                                    f"  [s{step:02d}] CTX_UPDATE Δ={hamming} "
                                    f"{prev_count}→{len(new_els)}el"
                                    + (f" precond={new_precond[:40]}" if new_precond else "")
                                    + (f" paths={new_dyn[:40]}" if new_dyn else "")
                                )
                            dialog_vocab = explore_dialogs(new_els, env, needs_a11y)
                            if dialog_vocab:
                                kw = extract_keywords(task_instr)
                                matches = [
                                    path for vname, path in dialog_vocab.items()
                                    if any(k in vname for k in kw)
                                ]
                                if matches:
                                    extra = "\nDialog contents:\n" + "\n".join(
                                        f"  - {m}" for m in matches[:5]
                                    )
                                    submenu_context = (submenu_context or "") + extra
                                    print(f"  [s{step:02d}] DIALOG {len(matches)} matches")

        except TimeoutError as _te:
            print(f"  [s{step:02d}] POST-STEP TIMEOUT: {_te}")
        finally:
            signal.alarm(0)

        if augmented:
            if os.path.exists(shot_after):
                state_after, vasp_after = farscry_state(shot_after)
                sf_frames.append((state_after, "state", vasp_after))
                if state_before and state_after and state_before == state_after:
                    consecutive_sf += 1
                    max_csf = max(max_csf, consecutive_sf)
                    print(f"  [s{step:02d}] SF! state_unchanged csf={consecutive_sf}")
                else:
                    if consecutive_sf > 0:
                        print(f"  [s{step:02d}] state_changed (was csf={consecutive_sf})")
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
    p.add_argument("--mode", choices=["run_a", "run_b_a11y", "run_b_context", "run_b_dynamic",
                                       "run_b_submenu", "run_b_full", "run_b_smart", "run_b",
                                       "run_c_vision"], required=True)
    p.add_argument("--tasks", type=Path, required=True)
    p.add_argument("--n", type=int, default=30)
    p.add_argument("--max-steps", type=int, default=15)
    p.add_argument("--result-dir", type=Path, default=Path("./osworld_results"))
    p.add_argument("--vm-path", type=str, default=None)
    p.add_argument("--provider", type=str, default="docker")
    p.add_argument("--output", type=Path, default=None)
    args = p.parse_args()

    from desktop_env.desktop_env import DesktopEnv

    augmented     = args.mode in ("run_b", "run_c_vision")
    a11y_only     = args.mode in ("run_b_a11y", "run_b_context", "run_b_dynamic",
                                   "run_b_submenu", "run_b_full", "run_b_smart")
    with_context  = args.mode == "run_b_context"
    with_dynamic  = args.mode in ("run_b_dynamic", "run_b_full", "run_b_smart", "run_c_vision")
    with_submenu  = args.mode in ("run_b_submenu", "run_b_full", "run_b_smart", "run_c_vision")
    # run_c_vision: always uses screenshot (a11y_only=False) + AT-SPI as text context
    needs_a11y    = augmented or a11y_only
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
                             with_context=with_context,
                             with_dynamic=with_dynamic,
                             with_submenu=with_submenu,
                             needs_a11y=needs_a11y,
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
