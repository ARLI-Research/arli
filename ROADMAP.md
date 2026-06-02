# ARLI Roadmap

## v0.5.0 — LEARNING & TRUST (current)

Архитектурный сдвиг: ARLI теперь не просто executor, а self-improving runtime с cryptographic trust.

### Что нового в v0.5.0

**Skills as Directories**
- SKILL.md (YAML frontmatter + markdown body) + `references/` + `scripts/`
- Lazy load: references/scripts грузятся только при активации (~500 токенов экономии на старте)
- `create_skill_from_template()` — создаёт структуру из кода
- `SkillHub`: discover, search, enable/disable, manifest.toml

**Auto-Skill Creation**
- `ToolSequenceTracker` — rolling window последних 5 тулзов → подсчёт повторяющихся цепочек
- `suggest_skill()` — когда цепочка повторяется 3+ раза → предлагает создать skill
- `record_sequence()` вызывается после каждого tool execution в agent.rs

**Memory: Reflect vs Recall**
- `recall` — быстрый FTS5 lookup (для cron, time-sensitive)
- `reflect` — глубокая LLM-синтез всех memories для target (240s+, использовать редко)
- Tool handler: `memory` tool принимает `action: "reflect"` / `action: "recall"`

**Feedback Loop (Corrections)**
- `add_correction(original, correction)` — сохраняет user corrections как `target="correction"`
- `get_corrections()` — извлечение всех сохранённых исправлений
- `FeedbackConfig`: enabled, auto_learn, max_corrections (100)

**x402 Agentic Wallet (infrastructure)**
- `X402Config`: wallet_address, private_key (NEVER logged), rpc_url, max_spend_per_call (50¢), total_budget ($10)
- `can_afford()`, `pay()`, `remaining_budget()`
- `x402_pay` tool — зарегистрирован условно (только если x402.enabled)
- USDC transfer logic: TODO

**ENSO Attestation (v0.6.0 readiness)**
- ed25519 key generation + signing
- `ArliAttestation` с replay protection (SHA-256 over все поля)
- ENSO Contracts canister integration (ICP mainnet)
- End-to-end: sandbox → attestation → settlement — verified на mainnet

**OpenShell 6-Phase Integration**
- Phase 1: Kernel sandbox (Landlock + seccomp + privdrop)
- Phase 2: Policy engine (host globs, per-agent profiles)
- Phase 3: Provider registry (11 YAML-defined providers)
- Phase 4: Gateway production patterns (/healthz, /readyz, /metrics)
- Phase 5: Inference routing (round-robin, fallback chains)
- Phase 6: OCSF audit logging (SIEM-compatible, class_uid 6007)

### Что уже было (v0.2.x–v0.4.x)

- 18 builtin tools (read_file, write_file, search_files, patch, shell, http_get, web_search, vision, voice, browser, execute_code, session_search, memory, delegate_task, process, image_generate, video_generate, text_to_speech)
- 36 LLM providers (3 adapter traits: OpenAI-compatible, Anthropic, OpenRouter)
- Agent actor model (mailbox, pause/resume/kill/redirect)
- Context compaction (tiktoken + LLM)
- Policy engine (allow/deny/approve + rate limiting + budget)
- Cron scheduler (persistent, CLI management)
- 20-platform gateway (Telegram, Discord, Slack, WhatsApp, Matrix, Teams, Email, Signal, SMS, Google Chat, Feishu, DingTalk, LINE, IRC, WeCom, ntfy, QQ, SimpleX, Yuanbao, BlueBubbles)
- MCP server (JSON-RPC stdio, Claude Desktop compatible)
- Plugin system (subprocess JSON-RPC)
- Skill hub + Credential pools + Profiles + Webhooks + Checkpoints
- Safety guardrail (AgentDoG 1.5 — 3D taxonomy, Pre-Reply checkpoint, Hybrid judge)
- Process manager (spawn/poll/wait/kill/list)
- Execute code sandbox (namespace isolation)
- TUI (ratatui, slash commands, /stats) + Shell completions (bash/zsh/fish)
- Self-update (`arli update`) + Install (`curl | bash` + `arli setup`)
- 148 тестов, CI/CD (GitHub Actions, multi-platform releases)

---

## 📋 TIER 3 — PRODUCTION HARDENING

| # | Что | Статус |
|---|-----|--------|
| 1 | Trading live execution (hypersdk) | ⏳ сейчас |
| 2 | Metrics/telemetry (Prometheus endpoint) | ⏳ |
| 3 | Python sandbox hardening (wasm isolation, pkg mgmt) | ⏳ |
| 4 | Auto-optimization (DSPy-like prompt tuning) | ⏳ |
| 5 | Kanban task boards | ⏳ |
| 6 | Web UI dashboard (axum + htmx) | ⏳ |

## 📋 TIER 4 — ECOSYSTEM

| # | Что | Статус |
|---|-----|--------|
| 7 | ENSO marketplace (RFQ → Quote → Contract flow, frontend) | ⏳ |
| 8 | Inference brokering (ARLI API keys, wholesale margin) | ⏳ |
| 9 | x402 USDC settlement (actual on-chain transfers) | ⏳ |
| 10 | Multi-agent swarm coordination | ⏳ |
