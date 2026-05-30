# ARLI Roadmap — что делать дальше

## Что уже есть (v0.2.3)

- Agent actor (mailbox, pause/resume/kill/redirect)
- 3 провайдера (OpenAI, DeepSeek, Anthropic + prompt caching)
- 10 инструментов (read/write/patch/shell/search/http/browser/session_search/memory/delegate)
- Context manager (tiktoken-rs, pressure, авто-компакция)
- Policy engine (allow/deny/approve + rate limiting + budget + grace period)
- Session store (SQLite WAL + FTS5)
- Memory store (add/replace/remove/search, FTS5)
- Skills (SKILL.md + TOML, загрузка из ~/.arli/skills/)
- AGENTS.md / CLAUDE.md авто-инжект
- Swarm (spawn/steer/kill/redirect/restart policy)
- Telegram gateway (long-poll)
- TUI (ratatui, word-wrapping, slash commands)
- Install: `curl | bash` + `arli setup`
- 44 теста
- CI/CD (GitHub Actions, release binaries)

---

## ✅ TIER 1 — База (production readiness) — DONE

| # | Что | Статус | Версия |
|---|-----|--------|--------|
| 1 | **soul.md** — файл идентичности агента | ✅ | v0.2.0 |
| 2 | **`arli config`** — управление конфигом | ✅ | v0.2.0 |
| 3 | **`arli model`** — смена модели на лету | ✅ | v0.2.0 |
| 4 | **Session resume** — `arli --resume` / `--continue` | ✅ | v0.2.1 |
| 5 | **Cron jobs** — `arli cron add/list/start/run` | ✅ | v0.2.2 |
| 6 | **`arli doctor`** — проверка здоровья | ✅ | v0.2.0 |
| 7 | **TUI slash commands** — `/help` `/stats` `/model` ... | ✅ | v0.2.3 |
| 8 | **Token usage / stats** — `/stats` в TUI | ✅ | v0.2.3 |

---

## TIER 2 — Экосистема (догнать Hermes)

| # | Что | Зачем | Аналог в Hermes |
|---|-----|-------|-----------------|
| 9 | **Больше провайдеров** | OpenRouter, Google Gemini, xAI/Grok, local (llama.cpp, ollama) | 20+ providers |
| 10 | **Web search tool** | Поиск в интернете через API (Tavily, Brave, SerpAPI) | web search |
| 11 | **Browser automation** | Не просто http_get, а полноценный браузер (CDP/Playwright) | browser (Browserbase/Camofox) |
| 12 | **Vision** — анализ картинок | Отправить скриншот/фото → модель описывает | vision toolset |
| 13 | **Voice (TTS/STT)** | Голосовые сообщения в Telegram, TTS ответы | voice, stt, tts |
| 14 | **Больше платформ** | Discord, Slack, WhatsApp — не только Telegram | 15+ платформ |
| 15 | **MCP сервер** | ARLI как MCP server — чтобы IDE подключались | `hermes mcp serve` |
| 16 | **Плагины** | Python/Rust расширения без правки ядра | plugins |
| 17 | **Профили** | Изолированные контексты: trading, coding, personal | `hermes profile` |
| 18 | **Webhook subscriptions** | Агент просыпается по HTTP/webhook | `hermes webhook` |
| 19 | **Checkpoints** | Снапшоты файловой системы с возможностью отката | `hermes --checkpoints`, `/rollback` |
| 20 | **Skill hub** | Поиск и установка скиллов из реестра | `hermes skills search/install` |
| 21 | **Credential pools** | Ротация API ключей, multiple keys per provider | `hermes auth` |
| 22 | **Shell completions** | Автодополнение для bash/zsh/fish | `hermes completion` |

---

## TIER 3 — Уникальные фичи (отрыв от конкурентов)

| # | Что | Зачем |
|---|-----|-------|
| 23 | **Trading live** | Hyperliquid реальные ордера, не stubs. WebSocket, стаканы, позиции |
| 24 | **Multi-agent work queue (Kanban)** | Очередь задач между агентами, как у Hermes Kanban |
| 25 | **Python alpha sandbox** | Безопасный Python-рантайм для трейдинговых расчётов (аналог execute_code) |
| 26 | **Real-time metrics dashboard** | Prometheus/Grafana — метрики агентов, токенов, задержек |
| 27 | **Auto-optimization (DSPy-like)** | Авто-подбор промптов и параметров под лучший результат |
| 28 | **Web UI** | Не только TUI — веб-интерфейс для мониторинга и управления агентами |

---

## Следующий шаг: Tier 2 #9 — OpenRouter provider
