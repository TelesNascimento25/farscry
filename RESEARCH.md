# farscry — Research Log

## Branch: `research/osworld-spike`

---

## Sessão 21 Mai 2026 — vLLM + UI-TARS 7B no OSWorld

### Contexto
Migração de llama.cpp (visão quebrada) para vLLM com UI-TARS-1.5-7B em bitsandbytes 4-bit
no NullPointer (RTX 5070 Blackwell sm_120, 12GB VRAM).

### Infraestrutura resolvida

**RTX 5070 Blackwell (sm_120) + vLLM:**
```bash
VLLM_USE_FLASHINFER_SAMPLER=0 \
VLLM_ATTENTION_BACKEND=TORCH_SDPA \
python3 -m vllm.entrypoints.openai.api_server \
  --model /home/teles/llm-setup/models/uitars_hf \
  --quantization bitsandbytes --load-format bitsandbytes \
  --max-model-len 4096 --gpu-memory-utilization 0.90 \
  --enforce-eager --port 8083
```
- 5.84 GB VRAM, bitsandbytes NF4 mixed precision
- FlashInfer incompatível com sm_120 → desabilitar

**Imagem Docker OSWorld customizada (`osworld-fast:latest`):**
```dockerfile
FROM happysixd/osworld-docker
RUN sed -i "s/migratable=no/migratable=yes/g" /run/proc.sh
```
- `migratable=yes` necessário para futura implementação de `savevm`
- Soft reset via `/execute` API: mata apps, mantém VM (~3-5s vs 3-4min boot)

### Formato correto do UI-TARS

O UI-TARS-1.5 usa formato de chat específico. **Errado** coloca texto antes da imagem.

**Formato correto:**
```python
messages = [
    {"role": "user", "content": [
        {"type": "image_url", "image_url": {"url": f"data:image/png;base64,{b64}"}},
        {"type": "text", "text": f"Task: {task}"},
    ]},
    {"role": "assistant", "content": "Thought:"},  # prefill
]
# Response: "Thought: ... reasoning ...\nAction: click(start_box='(x,y)')"
```

**Coordenadas:**
- Visual mode: `start_box='(x,y)'` onde x,y ∈ [0,1000] → converter: `round(x/1000 * 1920)`
- No_image mode (a11y_context): modelo copia coords absolutas → NÃO converter
- Flag no_image passada ao parser para distinguir os casos

### Parser de actions UI-TARS

```python
def action_to_pyautogui(raw: str, no_image: bool = False) -> str | None:
    # Normaliza "pyautogui hotkey(...)" → "hotkey(...)"
    raw = re.sub(r'^pyautogui\s+', '', raw)
    
    if "start_box=" in raw:
        # no_image ou >1000 = absoluto; senão normaliza [0-1000]→pixels
        ...
    
    # Formatos suportados:
    # click(start_box='(x,y)'), right click(...), doubleClick(...)
    # type(content='...') → pyautogui.typewrite(...)
    # hotkey(key='ctrl a') → pyautogui.hotkey('ctrl', 'a')
    # scroll(start_box=..., direction='down') → pyautogui.scroll(...)
```

### Lógica do no_image mode

```
matched=0 → visual mode, sem a11y_context
           modelo usa screenshot livre (sem distração da sidebar)

matched≥1 → no_image mode, a11y_context como único sinal
           modelo copia coords absolutas do contexto
```

**Razão:** Quando `matched=0`, enviar `a11y_context` com elementos da sidebar (Files, Chrome)
faz o modelo ir para o file manager em vez do ícone do desktop. Confirmado experimentalmente.

### Sinais no a11y_context (run_b_smart)

Em ordem de prioridade:
1. `text_input_hint` — campo `entry` visível (excluindo Activities/GNOME shell)
2. `appeared_signal` — novos elementos com coords absolutas
3. `direct_hint` — scan direto por keywords mesmo sem APPEARED
4. `untried_signal` — elementos não tentados
5. `live_ctx` — `semantic_state_to_context` completo

**Bug crítico corrigido:** `Activities` (botão GNOME) tem `role=entry`, y~14px.
Filtro: `role == "entry" AND name not in {"activities","applications"} AND y > 30`.

### Resultados OSWorld

| Run | Modo | n | TCR | Observação |
|-----|------|---|-----|------------|
| run_b_smart (llama.cpp) | a11y_only | 10 | **20%** | Prior acidental — visão quebrada gerava rightClick(1862,936) |
| run_c_vision (vLLM 4-bit) | augmented | 10 | 0% | Modelo vê imagem mas 4-bit degrada raciocínio sequencial |
| run_b_smart (vLLM 4-bit) | a11y_only | 10 | 0%* | *PASS visto nos logs mas run interrompido antes de terminar |

**Insight crítico:** Os 20% anteriores com llama.cpp eram acidentais — o modelo usava prior
`rightClick(1862,936)` porque a visão estava quebrada. Com vLLM funcionando, o modelo vê
o file manager icon e vai para lá. Fix: não enviar a11y_context quando matched=0.

**Progresso real observado nos logs:**
- step 0: `right click(start_box='(964,822)')` — rightClick correto no ícone do desktop ✓
- step 2: `APPEARED: ['rename', 'folder name', 'todo_list_jan_1']` — rename dialog abriu ✓
- step 3+: modelo clica em vez de typewrite → task não completa

### Problemas de infraestrutura

**Boot time por task:** 3-4 minutos (QEMU Ubuntu VM do zero cada task)
- QEMU `savevm` impossível: `migratable=yes` + `+invtsc` ainda bloqueia savevm
- Sem invtsc: boot fica 3x mais lento
- Solução implementada: soft reset via `/execute` mantém container vivo

**Hang do processo:**
- `env.step()` pode bloquear indefinidamente se VM freezes
- `vl_checkpoint()` pode bloquear com imagem grande + 4-bit model
- Fix: `signal.alarm(45)` em `env.step`, `signal.alarm(70)` no bloco post-step

**Docker container accumulation:**
- Filtro `docker ps --filter "name!=open-webui"` é inválido
- Correto: `docker ps --format "{{.Names}}" | grep -v "open-webui" | xargs -r docker stop`

### Estado final dos arquivos no NullPointer

```
/home/teles/farscry/spike/osworld_agent.py    — agente principal (synced)
/home/teles/run_vllm_experiment.sh             — script de run com cleanup
/home/teles/OSWorld/desktop_env/providers/docker/provider.py — soft reset
```

### Próximos passos

1. Verificar se timeouts (45s/70s) funcionam na prática
2. Confirmar soft reset não deixa estado residual
3. Investigar por que `typewrite('todo_list_Jan_2')` não acontece no rename dialog
4. Se TCR > 0%: rodar N=10 completo
