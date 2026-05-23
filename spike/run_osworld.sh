#!/bin/bash
# run_osworld.sh — wrapper que garante log unico por run + auto-save + GPU health check
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

# ── GPU health check ────────────────────────────────────────────────────────
# Historico: crash Mai 22 2026 — NV_ERR_GPU_NOT_FULL_POWER apos docker stop
# forcou reboot. Sempre verificar antes de iniciar.
echo "=== GPU HEALTH CHECK ==="

# 1. Verificar se nvidia-smi responde
if ! nvidia-smi --query-gpu=name --format=csv,noheader &>/dev/null; then
    echo "ERRO: nvidia-smi nao responde. GPU pode estar em estado invalido."
    echo "Acao: reiniciar o servidor antes de continuar."
    exit 1
fi

# 2. Verificar power state
PSTATE=$(nvidia-smi --query-gpu=pstate --format=csv,noheader 2>/dev/null | head -1 | tr -d ' ')
VRAM_FREE=$(nvidia-smi --query-gpu=memory.free --format=csv,noheader 2>/dev/null | head -1 | tr -d ' MiB')
echo "  Power state: $PSTATE"
echo "  VRAM livre: ${VRAM_FREE}MB"

# 3. Verificar vLLM respondendo ANTES do check de VRAM
# (vLLM carregado usa VRAM — P8+VRAM ocupada por vLLM e estado NORMAL de idle)
echo "  Verificando vLLM..."
VLLM_HTTP=$(curl -s --max-time 10 -o /dev/null -w "%{http_code}" http://localhost:8083/health 2>/dev/null || echo "0")
if [[ "$VLLM_HTTP" == "200" ]]; then
    echo "  vLLM OK (HTTP 200)"
else
    echo "  ERRO: vLLM nao responde (HTTP $VLLM_HTTP)"
    echo "  Iniciar com: source /home/teles/vllm-env/bin/activate && nohup vllm serve ..."
    # Sem vLLM: verificar se VRAM livre (se nao, contexto CUDA preso = crash pendente)
    if [[ "$PSTATE" == "P8" || "$PSTATE" == "P12" ]] && (( VRAM_FREE < 8000 )); then
        echo ""
        echo "  CRITICO: GPU em P8 COM VRAM ocupada e sem vLLM rodando."
        echo "  Causa: docker stop forcado deixou contexto CUDA preso."
        echo "  Acao: reiniciar o servidor (sudo reboot)."
        nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader 2>/dev/null || true
    fi
    exit 1
fi

# 5. Parar containers OSWorld restantes — graceful
CONTAINERS=$(docker ps --format "{{.Names}}" 2>/dev/null | grep -v open-webui || true)
if [[ -n "$CONTAINERS" ]]; then
    echo "  Parando containers ativos: $CONTAINERS"
    echo "$CONTAINERS" | xargs -r docker stop
    echo "  Aguardando 5s para GPU liberar contextos..."
    sleep 5
fi

echo ""
echo "=== GPU OK — iniciando run ==="
echo ""
# ────────────────────────────────────────────────────────────────────────────

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
