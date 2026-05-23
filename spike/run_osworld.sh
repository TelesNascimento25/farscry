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

if [[ "$PSTATE" == "P8" || "$PSTATE" == "P12" || "$PSTATE" == "P16" ]]; then
    if (( VRAM_FREE < 8000 )); then
        # P8 + VRAM ocupada = contexto CUDA preso (estado de crash)
        echo "  ERRO: GPU em low power ($PSTATE) COM VRAM ocupada — possivel contexto CUDA preso."
        echo "  Causa conhecida: docker stop forcado sem limpar contexto GPU."
        echo "  Acao necessaria: reiniciar o servidor (sudo reboot)."
        nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader 2>/dev/null || true
        exit 1
    fi
    # P8 + VRAM livre = GPU em idle normal (vai acordar quando vLLM iniciar — ok)
    echo "  GPU em idle (P8 + VRAM livre) — normal, vai acordar com vLLM."
fi

# 3. Verificar VRAM disponivel (precisa de pelo menos 8GB livres para Qwen GPTQ)
if (( VRAM_FREE < 8000 )); then
    echo "  AVISO: VRAM insuficiente (${VRAM_FREE}MB < 8000MB)"
    echo "  Verificar se outro processo esta usando a GPU:"
    nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader 2>/dev/null || true
    echo ""
    echo "  Aguardando 20s para VRAM liberar..."
    sleep 20
    VRAM_FREE=$(nvidia-smi --query-gpu=memory.free --format=csv,noheader | head -1 | tr -d ' MiB')
    echo "  VRAM apos espera: ${VRAM_FREE}MB"
    if (( VRAM_FREE < 8000 )); then
        echo "  ERRO: VRAM ainda insuficiente. Parar outros processos primeiro."
        exit 1
    fi
fi

# 4. Verificar vLLM respondendo (timeout curto — se travar aqui e problema)
echo "  Verificando vLLM..."
if ! curl -s --max-time 10 -o /dev/null -w "%{http_code}" http://localhost:8083/health | grep -qE "^200$"; then
    echo "  ERRO: vLLM nao responde em http://localhost:8083"
    echo "  Iniciar com: source /home/teles/vllm-env/bin/activate && nohup vllm serve ..."
    exit 1
fi
echo "  vLLM OK"

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
