#!/usr/bin/env bash
set -euo pipefail

MODEL_DIR="$HOME/llm-setup/models/uitars15_7b"
mkdir -p "$MODEL_DIR"

echo "Stopping Qwen VL service..."
systemctl --user stop vl7b.service 2>/dev/null || true
sleep 5
nvidia-smi --query-gpu=memory.used,memory.free --format=csv,noheader

echo "Downloading UI-TARS-1.5-7B Q4_K_M (~4.8GB)..."
cd "$MODEL_DIR"
wget -c --progress=dot:giga \
  "https://huggingface.co/mradermacher/UI-TARS-1.5-7B-GGUF/resolve/main/UI-TARS-1.5-7B.Q4_K_M.gguf" \
  -O model.gguf

echo "Downloading mmproj (~1.0GB)..."
wget -c --progress=dot:giga \
  "https://huggingface.co/mradermacher/UI-TARS-1.5-7B-GGUF/resolve/main/UI-TARS-1.5-7B.mmproj-Q8_0.gguf" \
  -O mmproj.gguf

ls -lh "$MODEL_DIR"

cat > "$HOME/.config/systemd/user/uitars.service" << 'SVC'
[Unit]
Description=UI-TARS-1.5-7B llama-server
After=default.target

[Service]
Type=simple
Environment=HOME=/home/teles
Environment=CUDA_VISIBLE_DEVICES=0
ExecStart=/home/teles/llama.cpp/build/bin/llama-server \
  -m /home/teles/llm-setup/models/uitars15_7b/model.gguf \
  --mmproj /home/teles/llm-setup/models/uitars15_7b/mmproj.gguf \
  -ngl 99 --ctx-size 4096 -fa on \
  --host 0.0.0.0 --port 8083
Restart=on-failure
RestartSec=10
KillSignal=SIGTERM
TimeoutStopSec=30

[Install]
WantedBy=default.target
SVC

systemctl --user daemon-reload
systemctl --user disable vl7b.service 2>/dev/null || true
systemctl --user enable uitars.service
systemctl --user start uitars.service

echo "Waiting 25s for model to load..."
sleep 25
nvidia-smi --query-gpu=memory.used,memory.free --format=csv,noheader

curl -s --max-time 5 http://localhost:8083/v1/models | python3 -c \
  'import json,sys; d=json.load(sys.stdin); print("OK:", d["data"][0]["id"])' 2>/dev/null \
  || echo "Server not ready yet, check: systemctl --user status uitars.service"
