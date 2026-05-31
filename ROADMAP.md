# ARLI Roadmap

## Что уже есть (v0.2.19)

- 13+ инструментов (read/write/patch/shell/search/http/browser/web_search/vision/voice/session_search/memory/delegate/execute_code/process)
- 4 провайдера (OpenAI, DeepSeek, Anthropic, OpenRouter)
- Agent actor (mailbox, pause/resume/kill/redirect)
- Context manager (tiktoken-rs, pressure, авто-компакция)
- Policy engine (allow/deny/approve + rate limiting + budget + grace period)
- Session store (SQLite WAL + FTS5, resume с lineage)
- Memory store (add/replace/remove/search, FTS5)
- Skills (SKILL.md + TOML)
- Cron scheduler (persistent, CLI management)
- Telegram gateway (long-poll)
- Multi-platform gateway (Discord, Slack, WhatsApp)
- MCP server (JSON-RPC stdio, Claude Desktop)
- Skill hub + Credential pools
- Safety guardrail (AgentDoG 1.5 — Pre-Reply checkpoint, 3D taxonomy, Hybrid judge)
- Background process management
- Execute code sandbox (namespace isolation)
- Shell completions (bash/zsh/fish)
- TUI (ratatui, slash commands, /stats)
- Install: `curl | bash` + `arli setup`
- 86 тестов
- CI/CD (GitHub Actions, release binaries)

---

## ✅ TIER 1 — DONE (v0.2.0–v0.2.3)

| # | Что | Статус |
|---|-----|--------|
| 1 | soul.md | ✅ |
| 2 | arli config | ✅ |
| 3 | arli model | ✅ |
| 4 | --resume / --continue | ✅ |
| 5 | Cron jobs | ✅ |
| 6 | arli doctor | ✅ |
| 7 | TUI slash commands | ✅ |
| 8 | Token usage /stats | ✅ |

## ✅ TIER 2 — DONE (v0.2.4–v0.2.9)

| # | Что | Статус |
|---|-----|--------|
| 9 | OpenRouter provider | ✅ |
| 10 | Web search (Tavily) | ✅ |
| 11 | Browser CDP (scraper) | ✅ |
| 12 | Vision (image analysis) | ✅ |
| 13 | Voice (TTS/STT) | ✅ |
| 22 | Shell completions | ✅ |

## ✅ TIER 2 — DONE (v0.2.10–v0.2.16)

| # | Что |
|---|-----|
| 14 | Discord/Slack/WhatsApp gateways | ✅ |
| 15 | MCP server | ✅ |
| 16 | Plugins | ✅ |
| 17 | Profiles | ✅ |
| 18 | Webhook subscriptions | ✅ |
| 19 | Checkpoints | ✅ |
| 20 | Skill hub | ✅ |
| 21 | Credential pools | ✅ |

## ✅ TIER 2.5 — DONE (v0.2.17–v0.2.19)

| # | Что | Статус |
|---|-----|--------|
| — | execute_code sandbox | ✅ v0.2.17 |
| — | Background process management | ✅ v0.2.17 |
| — | AgentDoG safety guardrail | ✅ v0.2.18 |
| — | Чистка: 0 warnings, 86 тестов | ✅ v0.2.19 |

## 📋 TIER 3 — PRODUCTION

| # | Что | Статус |
|---|-----|--------|
| 23 | Trading live (hypersdk integration) | ⏳ |
| 24 | Kanban — task boards | ⏳ |
| 25 | Python sandbox improvements | ⏳ |
| 26 | Metrics / telemetry | ⏳ |
| 27 | Auto-optimization (DSPy-like) | ⏳ |
| 28 | Web UI | ⏳ |
