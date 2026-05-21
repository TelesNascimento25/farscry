
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
