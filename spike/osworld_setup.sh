#!/usr/bin/env bash
set -euo pipefail

OSWORLD_DIR="$HOME/OSWorld"

check_deps() {
    command -v docker &>/dev/null || { echo "docker not found"; exit 1; }
    command -v python3 &>/dev/null || { echo "python3 not found"; exit 1; }
    command -v git &>/dev/null || { echo "git not found"; exit 1; }
}

install_osworld() {
    if [ -d "$OSWORLD_DIR" ]; then
        cd "$OSWORLD_DIR" && git pull --quiet
    else
        git clone --depth=1 https://github.com/xlang-ai/OSWorld "$OSWORLD_DIR"
    fi
    cd "$OSWORLD_DIR"
    grep -v 'agp-client' requirements.txt > /tmp/req_clean.txt
    pip3 install -r /tmp/req_clean.txt --break-system-packages --quiet 2>&1 | tail -2
    pip3 install requests zstandard pillow pydrive requests-toolbelt fastdtw pymupdf imagehash pdfplumber borb --break-system-packages --quiet 2>&1 | tail -2
    python3 -c "
import sys, types
for pkg in ['acoustid','librosa','fastdtw','PyPDF2','borb','borb.pdf','mutagen','pdfplumber','ag2','agp_client','easyocr','torch','cv2']:
    sys.modules[pkg] = types.ModuleType(pkg)
sys.path.insert(0, '.')
from desktop_env.desktop_env import DesktopEnv
print('DesktopEnv ok')
"
}

patch_metrics_init() {
    python3 - <<'PYEOF'
path = 'desktop_env/evaluators/metrics/__init__.py'
with open(path) as f:
    content = f.read()
lines = content.splitlines()
new_lines = []
i = 0
while i < len(lines):
    line = lines[i]
    if line.startswith('from .') and line.strip().endswith('('):
        block = [line]
        i += 1
        while i < len(lines):
            block.append(lines[i])
            if lines[i].strip() == ')':
                i += 1
                break
            i += 1
        new_lines.append('try:')
        for b in block:
            new_lines.append('    ' + b)
        new_lines.append('except Exception:')
        new_lines.append('    pass')
    elif line.startswith('from .') or line.startswith('import '):
        new_lines.append('try:')
        new_lines.append('    ' + line)
        new_lines.append('except Exception:')
        new_lines.append('    pass')
        i += 1
    else:
        new_lines.append(line)
        i += 1
with open(path, 'w') as f:
    f.write('\n'.join(new_lines))
PYEOF
}

install_farscry() {
    if ! command -v farscry &>/dev/null; then
        cd "$HOME/farscry" && cargo build --release --quiet
        cp target/release/farscry "$HOME/bin/farscry"
    fi
    echo "farscry: $(farscry --version)"
}

gen_tasks() {
    local out="${1:-$HOME/osworld_tasks_30.json}"
    python3 "$HOME/farscry/spike/osworld_gen_tasks.py" \
        --osworld-dir "$OSWORLD_DIR" \
        --n 30 --seed 42 --output "$out"
}

main() {
    check_deps
    install_osworld
    cd "$OSWORLD_DIR" && patch_metrics_init
    install_farscry
    gen_tasks "$HOME/osworld_tasks_30.json"
    echo "setup done"
    echo "run_a: python3 ~/farscry/spike/osworld_agent.py --mode run_a --tasks ~/osworld_tasks_30.json --n 30 --output ~/osworld_results/run_a.json"
    echo "run_b: python3 ~/farscry/spike/osworld_agent.py --mode run_b --tasks ~/osworld_tasks_30.json --n 30 --output ~/osworld_results/run_b.json"
}

main
