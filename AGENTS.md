
## Regras de Monitoramento de Experimentos

REGRA ABSOLUTA para monitorar processos longos (OSWorld, SSH, benchmarks):

1. **Poll a cada 8s** — nunca sleep > 30s entre checks
2. **Para IMEDIATAMENTE** se `stuck >= 6` (48s) E processo já iniciou (`LINES > 10`)
3. **Ao parar**: verificar wchan + ss -tp + tail do log ANTES de qualquer ação
4. **Limpar containers velhos** antes de cada run: `docker ps --format "{{.Names}}" | grep -v open-webui | xargs -r docker stop`
5. **Limpar log antigo**: `> /tmp/run.log` antes de lançar novo run
6. **Timeouts em TODA chamada bloqueante**: env.step(45s), vl_call(60s), post-step(70s)

Skill de referência: `~/.config/devin/skills/monitor-experiments/SKILL.md`

---

## Registro Obrigatorio de Spikes e Experimentos

REGRA ABSOLUTA para qualquer spike, experimento ou ablation com logs (OSWorld, benchmarks, testes de modelo, alteracoes no agente):

**Ao terminar o spike, criar imediatamente um arquivo de log na pasta:**
`/Users/teles/Documents/Obsidian Vault (iCloud)/FARSCRY/logs/`

### Formato do nome do arquivo

```
YYYYMMDD_[o-que-mudou]_[o-que-foi-testado]_[resultado].md
```

Exemplos corretos:
- `20260522_auto-press-return_entry-field-rename_TCR20pct.md`
- `20260521_uitars-nf4-vs-qwen-gptq_osworld-n10_TCR0-vs-20.md`
- `20260523_thunderbird-excl-shell_email-task_false-positive-fix.md`

Regras do nome:
- Kebab-case, sem espacos, sem acentos, sem caracteres especiais
- `[o-que-mudou]`: a mudanca principal feita no codigo ou config (ex: `auto-press-return`, `qwen-gptq-int4`, `entry-field-filter`)
- `[o-que-foi-testado]`: o cenario ou task testada (ex: `rename-task`, `osworld-n10`, `gimp-tasks`)
- `[resultado]`: TCR, PASS/FAIL, ou descricao breve do outcome (ex: `TCR20pct`, `PASS`, `loop-fixed`)

### Conteudo obrigatorio do arquivo

```markdown
# [Titulo descritivo: o que mudou | o que foi testado]

**Data:** YYYY-MM-DD
**Branch:** nome-do-branch
**Commit(s):** hash curto(s)

## O que mudou

[Descricao da alteracao feita ANTES do teste — codigo, config, prompt, modelo]
Incluir: arquivo alterado, funcao/trecho afetado, motivacao

## Estado anterior (antes da mudanca)

[Como estava antes — comportamento, bug, resultado anterior]

## O que foi testado

[Descricao do experimento — task, n_runs, modelo, parametros]

## Resultado

[TCR, PASS/FAIL por task, observacoes, surpresas]

## Conclusao

[O que aprendemos — o que a mudanca provou ou refutou]

## Proximo passo

[O que fazer a seguir com base nesse resultado]
```

### Quando criar o arquivo

- Ao terminar qualquer run com `run_b_smart` ou equivalente
- Ao mudar modelo (ex: UI-TARS para Qwen)
- Ao alterar logica do agente e testar resultado
- Ao confirmar um bug resolvido via run

**Nao esperar ate o fim da sessao.** Criar o arquivo imediatamente apos o resultado estar disponivel.

---

## Historico de Logs Brutos por Sessao

REGRA ABSOLUTA: ao final de TODA sessao de experimento (OSWorld, benchmark, ablation),
salvar o log bruto completo da sessao em pasta separada por modelo.

### Estrutura de pastas

```
/Users/teles/Documents/Obsidian Vault (iCloud)/FARSCRY/logs/raw/
  {model-slug}/
    YYYYMMDD_HHMMSS_{descricao}.log    <- log bruto do run
    YYYYMMDD_session-context.md        <- contexto da sessao (uma vez por dia/sessao)
```

### Slugs de modelo (usar exatamente esses nomes)

| Modelo | Slug da pasta |
|--------|---------------|
| UI-TARS 7B llama.cpp | `uitars-llamacpp` |
| UI-TARS 7B vLLM NF4 bitsandbytes | `uitars-vllm-nf4` |
| UI-TARS 7B vLLM int8 bitsandbytes | `uitars-vllm-int8` |
| Qwen2.5-VL-7B GPTQ-Int4 | `qwen25vl-gptq-int4` |
| Qwen2.5-VL-7B fp16 | `qwen25vl-fp16` |
| Claude CUA | `claude-cua` |
| Novo modelo | `{nome}-{quant}` em kebab-case sem acentos |

### Nome do arquivo de log

```
YYYYMMDD_HHMMSS_{descricao-curta}.log
```

Descricao curta (kebab-case, max 40 chars):
- `osworld-n10` para run de 10 tasks
- `osworld-n1_rename-task` para run focado numa task
- `osworld-n10_TCR20pct-OS-GIMP` para run com resultado notavel
- `interrompido` se o run foi cancelado antes do fim

### Conteudo do session-context.md (criar uma vez por sessao/dia)

```markdown
# Session Context: {YYYYMMDD}

**Modelo:** {nome completo}
**vLLM / backend:** {versao e flags usadas}
**Branch:** {nome do branch}
**Commits desta sessao:** {hashes}

## O que foi investigado

[Objetivo da sessao — qual bug ou hipotese estava sendo testada]

## Runs desta sessao

| Arquivo | n | TCR | Nota |
|---------|---|-----|------|
| YYYYMMDD_HHMMSS_descricao.log | 10 | 20% | primeiro PASS OS task |

## Descobertas

[O que aprendemos de relevante]
```

### Comando para salvar log no fim do run (NullPointer -> Mac via scp)

```bash
# Rodar no Mac IMEDIATAMENTE apos o run terminar — ANTES de comecar o proximo run
MODELO="qwen25vl-gptq-int4"
DEST="/Users/teles/Documents/Obsidian Vault (iCloud)/FARSCRY/logs/raw/$MODELO"
mkdir -p "$DEST"
scp kali:/tmp/run.log "$DEST/$(date +%Y%m%d_%H%M%S)_osworld-n10.log"
```

### REGRA CRITICA: salvar ANTES de apagar

O comando `> /tmp/run.log` ou `>> /tmp/run.log` que precede cada run **DESTROI o log anterior**.
Sequencia obrigatoria:

```
1. Run termina
2. SCP -> Mac (SALVAR AGORA)
3. Criar .md de spike
4. Somente entao: > /tmp/run.log (limpar para proximo run)
```

NUNCA inverter a ordem. Violar isso causa perda permanente de dados de experimento.
O que aconteceu nos runs 38a, 38b, 38c desta sessao (Mai 22): logs perdidos por nao seguir esta ordem.

### Espaco em disco

- Mac: ~43 GB livres (90% uso) — suficiente para centenas de sessoes
- NullPointer: ~66 GB livres — manter logs so em /tmp (ephemeral) durante run
- Logs brutos ficam permanentemente apenas no Mac (Obsidian Vault iCloud)

---

## Publication Standard

REGRA ABSOLUTA: só publicar quando estivermos em 70%+ de chance de impacto real.

### O que isso exige

1. **Corpus OSWorld** — não Qwen no terminal, não sessões sintéticas.
   OSWorld é o benchmark reconhecido. Claude CUA falha ~85% das tasks.
   Farscry precisa mostrar o que acontece nesse 85%.

2. **Números AER e VLR reais** de um agente CUA real (Claude CUA ou
   Qwen2.5-VL) em OSWorld. Sem placeholders.

3. **Comparação com ferramentas existentes** (Langfuse, Braintrust, Arize).
   Elas não fazem visual state. Isso precisa estar no paper.

4. **Demo funcional de 60 segundos** mostrando silent failure sendo detectado.

### Estado atual (Mai 2026) — o que está pronto

- farscry extract: 15.5x token reduction, N=223, ScreenSpot-Pro — MEDIDO, REAL
- StateId: primitiva nova cross-session, ninguém publicou antes
- AER/VLR: definições formais, implementação funcionando
- farscry serve --mcp: headless Linux, SIGTERM limpo, Xvfb auto-start
- farscry_mark_action: MCP tool para AER explícito
- 117 testes passando, CI verde 3 plataformas

### O que FALTA para 70%

1. OSWorld corpus com Claude CUA (~$20-50 de API)
2. Paper escrito (zero palavras)
3. Demo video 60s
4. Seção comparação vs Langfuse/Braintrust/Arize

### Janela

4-6 semanas. Depois os labs publicam algo similar.
NÃO publicar com corpus sintético, Qwen terminal, ou números placeholder.
