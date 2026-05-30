# ARLI Roadmap — что делать дальше

## Что уже есть (v0.1.3)

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
- TUI (ratatui, word-wrapping)
- Install: `curl | bash` + `arli setup`
- 44 теста
- CI/CD (GitHub Actions, release binaries)

---

## TIER 1 — База (production readiness)

То без чего ARLI не может считаться законченным продуктом.

| # | Что | Зачем | Аналог в Hermes |
|---|-----|-------|-----------------|
| 1 | **soul.md** — файл идентичности агента | Системный промпт из файла `~/.arli/soul.md`, а не только из кода. Пользователь описывает кто агент и как себя вести | soul.md |
| 2 | **`arli config`** — управление конфигом из CLI | `arli config show/edit/set/path` — не лезть в файл руками | `hermes config` |
| 3 | **`arli model`** — смена модели на лету | Интерактивный выбор модели/провайдера без правки config.toml | `hermes model` |
| 4 | **Session resume** — продолжение сессии | `arli --resume SESSION_ID` или `arli --continue` — вернуться к диалогу | `hermes --resume`, `hermes --continue` |
| 5 | **Cron jobs** — работа по расписанию | Stub есть в коде, надо подключить CLI: `arli cron list/create/pause/remove` | `hermes cron` |
| 6 | **`arli doctor`** — проверка здоровья | Проверить что Rust собран, API ключ работает, SQLite ок, провайдер отвечает | `hermes doctor` |
| 7 | **TUI slash commands** | `/help`, `/clear`, `/model`, `/status`, `/save` — полноценное управление из TUI | `/help`, `/model`, etc |
| 8 | **Token usage / cost tracking** | Показывать сколько токенов и денег потрачено за сессию | `/usage`, `show_cost` |

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

## Приоритет по дням

### День 1-2 (Tier 1: production-ready)
1. soul.md — чтение из ~/.arli/soul.md при старте
2. `arli config show/set/path` — CLI управление конфигом
3. `arli model` — интерактивная смена провайдера/модели
4. `arli doctor` — проверка здоровья

### День 3-4 (Tier 1: state management)
5. Session resume — `arli --resume`, `arli --continue`
6. Cron jobs — подключить stub, CLI управление
7. TUI slash commands — `/model`, `/usage`, `/save`, `/status`
8. Token usage tracking — показывать в TUI и по `/usage`

### День 5-7 (Tier 2: догоняем экосистему)
9. OpenRouter provider
10. Web search tool
11. `arli completion` для bash/zsh
12. Профили — `arli profile create/use/list`

### День 8-10 (Tier 2: платформы)
13. Discord gateway
14. MCP server
15. Vision tool

### Неделя 3-4 (Tier 3: отрыв)
16. Trading live (Hyperliquid WebSocket)
17. Web UI
18. Python alpha sandbox
