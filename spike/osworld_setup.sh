#!/usr/bin/env bash
set -euo pipefail

OSWORLD_DIR="$HOME/OSWorld"
CONDA_ENV="osworld"

check_deps() {
    command -v docker &>/dev/null || { echo "docker not found"; exit 1; }
    command -v python3 &>/dev/null || { echo "python3 not found"; exit 1; }
    command -v git &>/dev/null || { echo "git not found"; exit 1; }
}

install_osworld() {
    if [ -d "$OSWORLD_DIR" ]; then
        echo "OSWorld already cloned at $OSWORLD_DIR"
        cd "$OSWORLD_DIR" && git pull --quiet
    else
        git clone --depth=1 https://github.com/xlang-ai/OSWorld "$OSWORLD_DIR"
    fi
    cd "$OSWORLD_DIR"
    pip3 install -e . --quiet
    pip3 install requests zstandard --quiet
}

pull_docker_image() {
    echo "Pulling OSWorld Docker image..."
    docker pull xlangai/ubuntu_osworld:latest 2>&1 | tail -3
}

install_farscry() {
    if ! command -v farscry &>/dev/null; then
        echo "Building farscry from source..."
        cd "$HOME/farscry" && cargo build --release --quiet
        cp target/release/farscry "$HOME/bin/farscry"
    fi
    echo "farscry: $(farscry --version)"
}

generate_task_list() {
    local out_file="${1:-$HOME/osworld_tasks_30.json}"
    python3 "$OSWORLD_DIR/evaluation_examples/generate_task_list.py" \
        --test_all_meta "$OSWORLD_DIR/evaluation_examples/test_all.json" \
        --n 30 \
        --seed 42 \
        --output "$out_file" 2>/dev/null || \
    python3 - <<'PYEOF'
import json, random, pathlib, sys

meta_path = pathlib.Path("$OSWORLD_DIR/evaluation_examples/test_all.json")
if not meta_path.exists():
    print("test_all.json not found", file=sys.stderr)
    sys.exit(1)

meta = json.loads(meta_path.read_text())
rng = random.Random(42)
tasks = []
for domain, task_ids in meta.items():
    for task_id in task_ids:
        cfg = pathlib.Path(f"$OSWORLD_DIR/evaluation_examples/examples/{domain}/{task_id}.json")
        if not cfg.exists():
            continue
        task_data = json.loads(cfg.read_text())
        tasks.append({
            "id": f"{domain}/{task_id}",
            "instruction": task_data.get("instruction", ""),
            "config_path": str(cfg),
        })

rng.shuffle(tasks)
selected = tasks[:30]
out = pathlib.Path("${1:-$HOME/osworld_tasks_30.json}")
out.write_text(json.dumps(selected, indent=2))
print(f"wrote {len(selected)} tasks to {out}")
PYEOF
}

main() {
    echo "=== OSWorld setup on NullPointer ==="
    check_deps
    install_osworld
    pull_docker_image
    install_farscry
    generate_task_list "$HOME/osworld_tasks_30.json"
    echo ""
    echo "Setup complete. Run:"
    echo "  python3 ~/farscry/spike/osworld_agent.py --mode run_a --tasks ~/osworld_tasks_30.json --n 30 --output ~/run_a.json"
    echo "  python3 ~/farscry/spike/osworld_agent.py --mode run_b --tasks ~/osworld_tasks_30.json --n 30 --output ~/run_b.json"
    echo "  python3 ~/farscry/spike/corpus_ab_pipeline.py --run-a ~/run_a_sessions/ --run-b ~/run_b_sessions/"
}

main
