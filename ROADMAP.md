# ARLI Roadmap

## Что уже есть (v0.2.9)

- 13 инструментов (read/write/patch/shell/search/http/browser/web_search/vision/voice/session_search/memory/delegate)
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
- Plugin system (subprocess JSON-RPC, plugin.toml)
- Shell completions (bash/zsh/fish)
- TUI (ratatui, slash commands, /stats)
- Install: `curl | bash` + `arli setup`
- 46 тестов
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

## ⏳ TIER 2 — REMAINING

| # | Что |
|---|-----|
| 14 | Discord/Slack/WhatsApp gateways | ✅ |
| 15 | MCP server | ✅ |
| 16 | Plugins | ✅ |
| 17 | Profiles | ✅ |
| 18 | Webhook subscriptions |
| 19 | Checkpoints |
| 20 | Skill hub |
| 21 | Credential pools |

## 📋 TIER 3 (уникальные фичи)

23–28: Trading live, Kanban, Python sandbox, Metrics, Auto-optimization, Web UI
