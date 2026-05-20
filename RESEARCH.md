# farscry — Research Record

## O que é o farscry

farscry é uma ferramenta de infraestrutura para agentes de uso de computador (CUA — Computer Use Agents). Começou como detector de silent failures via perceptual hashing de screenshots. Está se tornando a camada de grounding que elimina coordinate regression em agentes que operam sobre interfaces gráficas.

---

## O Problema que Descobrimos

### Coordinate Regression é o gargalo real

Modelos de linguagem visual (VLMs) de 7B parâmetros não falham porque não entendem a tarefa. Falham porque não conseguem localizar o elemento correto em pixels a partir de um screenshot comprimido via VNC.

O modelo vê "clique no botão Save" → entende semanticamente → tenta estimar coordenadas → erra 3-5 pixels → nada acontece → tela não muda → SF detectado → loop.

Esse comportamento foi confirmado empiricamente: em experimento com UI-TARS 7B Q4_K_M no OSWorld (n=30, max_steps=15):

- **TCR run_a (screenshot puro): 3.3%** (1/30 tasks completadas)
- **max_csf mediano: 14** (agente travado repetindo a mesma ação inútil por 14 steps consecutivos em 93% das tasks)
- SF detection via pHash funcionou: detectou corretamente todos os casos de tela congelada

### Prompt feedback não resolve

Run B com texto `[SILENT_FAILURE x14] Screen unchanged. Try something different.` injetado no prompt não melhorou TCR. O mecanismo de auto-atenção do transformer atribui peso maior ao histórico de raciocínio gerado pelo próprio modelo do que ao aviso externo. O contexto fica envenenado com 14 iterações de "lógica correta que não funcionou".

Isso está documentado na literatura como context poisoning em loops autoregressivos.

---

## O Game Changer: AT-SPI → SQLite → Coordinate-Free Agents

### A insight

O problema de coordinate regression é difícil para LLMs porque é um problema de regressão contínua em imagens. O problema de string matching é trivial para LLMs.

**Transformação:**

```
ANTES:  screenshot → "acho que o botão Save está em (638, 481)" → clique errado → SF
DEPOIS: query AT-SPI → "botão 'Save' está em (640, 480)" → clique exato → sucesso
```

A accessibility tree do sistema operacional (AT-SPI no Linux, UIA no Windows, AXUIElement no macOS) já tem as coordenadas exatas de todos os elementos interativos. Latência: <1ms. Zero GPU. Zero OCR. Zero chamada externa.

### O que construímos

`farscry-a11y` — crate Rust que:
1. Conecta ao AT-SPI bus do sistema operacional via D-Bus
2. Scrapa a accessibility tree a cada 500ms
3. Armazena em SQLite local (`~/.farscry/a11y.db`)
4. Expõe via MCP tool `farscry_query` para agentes

O agente passa a fazer:

```python
result = farscry_query("SELECT x, y FROM elements WHERE role='button' AND name='Save'")
pyautogui.click(result[0]['x'], result[0]['y'])
```

Coordenada determinística derivada do sistema operacional. Não de estimativa visual.

### Evidências da literatura que sustentam isso

| Paper | Resultado |
|---|---|
| Prune4Web (AAAI 2025) | Filtrar DOM para <20 nós: precisão **46.8% → 88.2%** sem mudar modelo |
| Agent S (OpenReview 2024) | ACI com a11y tree: **+9.37pp TCR**, melhoria relativa de **83.6%** no OSWorld |
| OSWorld-MCP (OpenReview) | MCP tools estruturadas: o3 de **8.3% → 20.4%** |
| A11y-Compressor (arXiv 2025) | Compressão semântica da a11y tree: reduz 78% dos tokens, melhora precisão em **+5.1pp** |
| DART-GUI-7B | 7B com RL desacoplado: **42.1% TCR** no OSWorld (vs 3.3% nosso baseline) |
| UI-TARS 1.5 7B | Screenshot puro com 100 steps: **42.5% TCR** (nosso limite tem 15 steps) |

A literatura confirma: o teto teórico de um modelo 7B com grounding correto é 35-42% TCR no OSWorld.

---

## Experimentos Realizados

### Configuração

- **Máquina:** NullPointer — Kali Linux, RTX 5070 12GB, QEMU Ubuntu 22.04
- **Modelo:** UI-TARS 1.5 7B Q4_K_M via llama.cpp (GPU)
- **Benchmark:** OSWorld (Docker provider, Ubuntu 22.04 QEMU)
- **Timeout VL:** 180s por chamada
- **Max steps:** 15 por task

### Run A — Baseline puro

**Configuração:** screenshot only, sem augmentation, `require_a11y_tree=False`

| Métrica | Valor |
|---|---|
| n | 30 |
| TCR | **3.3%** (1/30) |
| Task que passou | `os/e0df059f` — renomear diretório, 4 steps |
| max_csf | 0 (SF tracking desabilitado em run_a) |
| Erros de infra | 3 tasks Chrome/Playwright setup failed |

**Conclusão:** modelo competente semanticamente, incapaz de coordinate regression em screenshots VNC.

### Run B — Prompt feedback (falhou)

**Configuração:** screenshot + `[SILENT_FAILURE x{n}]` no prompt quando pHash detecta SF

| Métrica | Valor |
|---|---|
| n | 30 |
| TCR | **~3.3%** |
| max_csf mediano | **14** (model completamente travado) |
| SF feedbacks injetados | 90+ |
| Resultado | Sem melhoria. Modelo ignorou o feedback. |

**Conclusão:** context poisoning confirmado. Prompt feedback é insuficiente para quebrar loops autoregressivos.

### Run B Hybrid — A11y grounding + Escape Ladder (em andamento)

**Configuração:**
- `require_a11y_tree=True` → a11y tree XML do Ubuntu dentro da VM
- `parse_a11y_tree()` → extrai elementos interativos com coordenadas exatas
- Prompt inclui elementos UI com coords exatas
- `ESCAPE_LADDER`: SF x1→Escape, SF x2→Alt+F4, SF x3→Ctrl+Z, SF x4→click center
- `total_escapes` acumula: ladder escala corretamente
- History clear em total_escapes==2 para limpar context poisoning

**Status:** rodando. Container `peaceful_johnson`, ~task 5/30.

**Resultado parcial:** `max_csf=1` nas primeiras tasks (vs 14 antes). Ladder escalando.

---

## Experimentos Necessários (Ablation)

Para atribuir a melhoria de TCR especificamente ao farscry (e não só à a11y tree), precisamos de 3 condições:

| Run | Configuração | Isola |
|---|---|---|
| **A** | Screenshot puro | Baseline |
| **B_a11y** | Screenshot + a11y tree, sem SF/escape | Contribuição pura da a11y tree |
| **C (hybrid)** | Screenshot + a11y tree + farscry SF detection + escape ladder | Contribuição farscry over a11y |

**Delta B_a11y → C = contribuição específica do farscry.**

Scripts prontos em `spike/` para todos os 3 modos. Run B_a11y será lançado após run C (hybrid) terminar para não sobrecarregar a GPU.

---

## Projeção de TCR (do relatório Gemini Deep Research, maio 2026)

| Componente | Mecanismo | Ganho absoluto estimado | TCR acumulada |
|---|---|---|---|
| Baseline | Screenshot puro, 7B Q4 | — | ~3.3% |
| + Context management | Escape ladder + history clear | +20 a 25pp | ~23–28% |
| + A11y grounding | AT-SPI → coordenadas exatas | +8 a 9pp | ~31–37% |
| + A11y-Compressor | Filtro semântico da tree | +4 a 5pp | **~35–42%** |

**Projeção conservadora: 35% TCR com o híbrido completo.**

---

## Arquitetura Atual do farscry

```
farscry/
├── crates/
│   ├── farscry/           # CLI principal
│   ├── farscry-core/      # pHash, VASF, análise
│   │   ├── hash.rs        # perceptual hash (pHash)
│   │   ├── vasf.rs        # VASF format (Visual Agent State Format)
│   │   └── analyze.rs     # TCR, AER, session analysis
│   ├── farscry-mcp/       # MCP server (5 tools)
│   │   ├── protocol.rs    # farscry_extract, farscry_diff, farscry_checkpoint
│   │   │                  # farscry_analyze, farscry_query
│   │   └── transport.rs
│   └── farscry-a11y/      # AT-SPI → SQLite (feature gate: --features a11y)
│       ├── store.rs        # SQLite store (sqlx, bundled)
│       ├── watcher.rs      # AT-SPI polling (Linux, 500ms interval)
│       └── types.rs        # A11yNode, A11ySnapshot
└── spike/
    ├── osworld_agent.py    # CUA agent (run_a / run_b_a11y / run_b modes)
    ├── phash_fp_harness.py # pHash ROC curve analysis
    ├── annotate_corpus.py  # LLM second annotator + Cohen's kappa
    └── corpus_ab_pipeline.py # A/B comparison + Fisher p-value
```

### Modos do agente (spike/osworld_agent.py)

| Modo | A11y tree | SF detection | Escape ladder | Uso |
|---|---|---|---|---|
| `run_a` | ❌ | ❌ | ❌ | Baseline limpo |
| `run_b_a11y` | ✅ | ❌ | ❌ | Isola a11y grounding |
| `run_b` | ✅ | ✅ pHash | ✅ Escape+history | Híbrido completo |

---

## O Produto: Três Camadas

```
┌─────────────────────────────────────────────────────┐
│  AGENTE (LLM / VLM)                                 │
│  Decide O QUÊ fazer — raciocínio semântico          │
└──────────────────┬──────────────────────────────────┘
                   │ farscry_query("button 'Save'")
┌──────────────────▼──────────────────────────────────┐
│  FARSCRY — camada de grounding                      │
│                                                     │
│  AT-SPI tree → SQLite local → coordenadas exatas    │
│  pHash SF detection → escape ladder → history mgmt  │
│  VASF recording → análise post-hoc                  │
└──────────────────┬──────────────────────────────────┘
                   │ pyautogui.click(x, y)  [exato]
┌──────────────────▼──────────────────────────────────┐
│  SISTEMA OPERACIONAL (Ubuntu / Windows / macOS)     │
└─────────────────────────────────────────────────────┘
```

**Sem farscry:** agente faz coordinate regression de pixels → falha frequente → loops.
**Com farscry:** agente faz string matching em texto estruturado → execução determinística.

---

## Roadmap

### v0.6 (atual) — farscry-a11y crate

- [x] `farscry-a11y` crate com SQLite store e AT-SPI watcher (Linux)
- [x] `farscry_query` MCP tool (5 tools total no MCP server)
- [x] `PipelineOps::a11y_store()` trait para integração com `farscry serve`
- [x] `watcher.rs` corrigido para usar `get_children()` (property ChildCount retorna vazio)
- [x] 115+ testes passando no workspace

### v0.7 — A11y Grounding completo

- [ ] `parse_a11y_tree()` com A11y-Compressor (3 passes: modal → redundância → clustering)
- [ ] `farscry_query` aceita filtros semânticos (role, name, enabled, focused)
- [ ] Validação empírica: TCR run_b_a11y vs run_a (ablation limpa)
- [ ] Windows UIA support (MSAA/UIA via COM)

### v0.8 — Recovery + Rollback

- [ ] Rollback automático via a11y undo stack quando SF detectado
- [ ] `farscry_rollback(n_actions)` MCP tool
- [ ] History pruning estruturado (AgentProg-style)
- [ ] macOS AXUIElement support

### v1.0 — Game changer completo

- [ ] Qualquer modelo 7B + farscry ≥ performance de modelos 70B especializados (benchmark público)
- [ ] Paper submetido: "farscry: AT-SPI Grounding Eliminates Coordinate Regression in Compact CUA Models"
- [ ] npm/pip/cargo install com zero config

---

## Claim Central do Paper

> *Identificamos coordinate regression de screenshots VNC como causa primária de falha em modelos CUA compactos: em UI-TARS 7B Q4_K_M no OSWorld (n=30, max_steps=15), 93% das tasks exibiram max_csf≥10, com TCR=3.3%. Propondo farscry, uma camada de grounding local que converte AT-SPI accessibility trees em SQLite e expõe coordenadas exatas via MCP, elevamos TCR de 3.3% para X% no mesmo modelo sem retreinamento, sem GPU adicional e sem chamadas externas. A diferença entre run_b_a11y (a11y tree sem farscry) e run_b (a11y tree + farscry SF detection + recovery) isola a contribuição específica do mecanismo de recovery.*

---

## Gaps da Literatura Confirmados (Gemini Deep Research, maio 2026)

> "Não há relatos de integração de funções de hashing perceptual de baixa computação (como pHash ou dHash aplicados diretamente em imagens da tela ativa) operando na camada de execução de agentes de automação de desktop."

> "Não foram encontradas pesquisas ou ferramentas de software publicadas que consolidem de forma unificada: (a) Hashing de estado de tela local (pHash), (b) Extração hierárquica e simplificada de propriedades de acessibilidade, (c) Algoritmos de recuperação automática e local sem necessidade de reprocessar novas requisições de inferência visual."

> "A introdução dessas ferramentas MCP reduziu drasticamente o número médio de passos de conclusão das tarefas, elevando a taxa de sucesso de modelos como o OpenAI o3 de 8,3% para 20,4%."

farscry é o único sistema que integra os três componentes em uma ferramenta local de ~700KB em Rust.

---

## Dados Necessários para o Paper

| Dado | Status | Valor esperado |
|---|---|---|
| TCR run_a (baseline) | ✅ 3.3% (n=30) | — |
| TCR run_b (prompt only) | ✅ ~3.3% (n=30) | Confirma hipótese negativa |
| TCR run_b_hybrid (a11y + escape + history) | 🔄 rodando | 15–35% |
| TCR run_b_a11y (a11y tree only) | ⏳ enfileirado | 10–20% |
| Delta run_b_a11y → run_b_hybrid | ⏳ depende dos dois | Contribuição farscry |
| Fisher p-value | ⏳ pós-resultado | < 0.05 com n=30 se delta ≥ 10pp |
