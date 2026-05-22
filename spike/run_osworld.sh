#!/bin/bash
# run_osworld.sh — wrapper que garante log unico por run + auto-save
# Uso: ./run_osworld.sh [args para osworld_agent.py]

set -euo pipefail

MODELO="qwen25vl-gptq-int4"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOG_REMOTE="/tmp/osworld_${TIMESTAMP}.log"
LOG_LOCAL="/Users/teles/Documents/Obsidian Vault (iCloud)/FARSCRY/logs/raw/${MODELO}/${TIMESTAMP}_osworld.log"

echo "=== RUN OSWORLD ==="
echo "Log: $LOG_REMOTE"
echo "Destino: $LOG_LOCAL"
echo ""

cd /home/teles/OSWorld

PYTHONPATH=/home/teles/OSWorld \
VL_SERVER=http://localhost:8083 \
VL_MODEL=/home/teles/llm-setup/models/qwen25vl_gptq \
VL_MODEL_TYPE=qwen25vl \
VL_SCREEN_W=1920 VL_SCREEN_H=1080 \
python3 -u /home/teles/farscry/spike/osworld_agent.py "$@" \
  > "$LOG_REMOTE" 2>&1

EXIT_CODE=$?

echo ""
echo "=== RUN TERMINADO (exit=$EXIT_CODE) ==="
echo "Log salvo em: $LOG_REMOTE"
echo "Linhas: $(wc -l < "$LOG_REMOTE")"
tail -5 "$LOG_REMOTE"

echo ""
echo "Para copiar para o Mac:"
echo "  scp kali:$LOG_REMOTE \"$LOG_LOCAL\""

exit $EXIT_CODE
