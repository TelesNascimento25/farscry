#!/bin/bash
# gpu_safe.sh — garante que GPU esta em full power antes de qualquer run
# Uso: source gpu_safe.sh ou ./gpu_safe.sh check

set -euo pipefail

GPU_WAIT_SECS=${GPU_WAIT_SECS:-10}

check_gpu() {
    local power_state
    power_state=$(nvidia-smi --query-gpu=pstate --format=csv,noheader 2>/dev/null | head -1 | tr -d ' ')
    echo "GPU power state: $power_state"
    
    # P0-P3 = high perf, P8-P12 = low power (bad para CUDA init apos crash)
    if [[ "$power_state" == "P8" || "$power_state" == "P12" || "$power_state" == "P16" ]]; then
        echo "AVISO: GPU em low power state ($power_state) — aguardando ativar..."
        return 1
    fi
    return 0
}

wake_gpu() {
    echo "Ativando GPU com workload minimo..."
    # Forcar GPU para P0 fazendo uma operacao minima
    python3 -c "
import subprocess, time
try:
    import torch
    if torch.cuda.is_available():
        x = torch.zeros(1).cuda()
        del x
        torch.cuda.synchronize()
        print('GPU acordada via torch')
except:
    pass
" 2>/dev/null || true
    sleep 3
}

kill_vllm_gracefully() {
    echo "Parando vLLM graciosamente..."
    # Tentar shutdown graceful via API primeiro
    curl -s --max-time 5 -X POST http://localhost:8083/shutdown 2>/dev/null || true
    sleep 2
    # SIGTERM (graceful)
    pkill -TERM -f "vllm.entrypoints" 2>/dev/null || true
    sleep 5
    # SIGKILL so se ainda rodando
    pkill -KILL -f "vllm.entrypoints" 2>/dev/null || true
    sleep $GPU_WAIT_SECS
    echo "vLLM parado. Aguardando GPU liberar VRAM..."
}

wait_vram_free() {
    local target_free=${1:-8000}  # MB — default 8GB livres
    local max_wait=60
    local waited=0
    while (( waited < max_wait )); do
        local free_mb
        free_mb=$(nvidia-smi --query-gpu=memory.free --format=csv,noheader 2>/dev/null | tr -d ' MiB')
        echo "VRAM livre: ${free_mb}MB (target: ${target_free}MB)"
        if (( free_mb >= target_free )); then
            echo "VRAM OK"
            return 0
        fi
        sleep 5
        (( waited += 5 ))
    done
    echo "AVISO: VRAM nao liberou apos ${max_wait}s"
    return 1
}

case "${1:-check}" in
    check)
        check_gpu || (wake_gpu && check_gpu)
        ;;
    kill-vllm)
        kill_vllm_gracefully
        ;;
    wait-vram)
        wait_vram_free "${2:-8000}"
        ;;
    *)
        echo "Uso: $0 [check|kill-vllm|wait-vram <MB>]"
        ;;
esac
