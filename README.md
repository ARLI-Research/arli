# ARLI — Rust-native AI Agent Harness

Production-grade agent infrastructure. Single ~20MB binary, ~50ms cold start, zero runtime dependencies. Actor-based agent loop with swarm orchestration, multi-platform messaging gateway, cron scheduler, and self-updating CLI.

## Quick Start

```bash
# Install (Linux / macOS, no Rust toolchain needed)
curl -fsSL https://raw.githubusercontent.com/ARLI-Research/arli/main/install.sh | bash

# Configure
arli setup

# Chat
arli chat                    # Interactive TUI
arli chat -q "Explain Rust"  # Single query
```

## What It Does

ARLI is a universal agent runtime — the engine that powers AI assistants. It handles:

- **Agent loop** — mailbox-driven actor, auto-compaction, streaming, budget tracking
- **LLM routing** — 36 providers behind a unified interface
- **Messaging** — 20 platforms, one gateway daemon
- **Scheduling** — cron jobs with skill attachments
- **Self-update** — `arli update` from GitHub Releases

## Features

### LLM Providers (36)

DeepSeek, OpenAI, Anthropic, OpenRouter, Google AI Studio, xAI/Grok, GitHub Copilot, Nous Portal, NovitaAI, Qwen/DashScope, Xiaomi MiMo, Tencent TokenHub, NVIDIA NIM, HuggingFace, Z.AI/GLM, Kimi/Moonshot, StepFun, MiniMax (global + China), LM Studio, Ollama, AWS Bedrock, Azure Foundry, Arcee AI, GMI Cloud, Kilo Code, OpenCode Zen/Go, Alibaba Cloud, Custom endpoint — full list in `arli setup`.

All routed through a single provider trait. OpenAI-compatible providers share one adapter. Anthropic has native prompt caching.

### Messaging Gateway (20 platforms)

Telegram, Discord, Slack, WhatsApp, Matrix, Microsoft Teams, Email (IMAP/SMTP), Signal, SMS/Twilio, Google Chat, Feishu/Lark, DingTalk, LINE, IRC, WeCom, QQ Bot, ntfy, SimpleX, Yuanbao, BlueBubbles/iMessage.

One daemon, zero-config per platform — set env vars, start `arli gateway start`.

```bash
# Set platform tokens
export TELEGRAM_BOT_TOKEN="..."
export DISCORD_BOT_TOKEN="..."
export MATRIX_USER="..." MATRIX_PASSWORD="..."

# Start all platforms at once
arli gateway start
```

### Agent Settings

- **Max iterations** — 90 default (configurable)
- **Tool progress** — off / new / all / verbose
- **Context compression** — threshold 0.5–0.95
- **Session reset** — inactivity + daily, inactivity-only, daily-only, never

### Tools

Terminal, file read/write/patch/search, HTTP fetch, web search, browser automation, vision/image analysis, text-to-speech, image generation, video generation, session search, persistent memory, task delegation, code execution.

### TTS (Edge TTS default, free)

Edge TTS (Microsoft, free cloud), OpenAI TTS, local engines (espeak-ng, flite, macOS say). Auto-fallback — tries Edge first, then OpenAI, then local.

### Image Generation

FAL.ai (Flux), OpenAI DALL-E 3. Auto-fallback — tries FAL first, then OpenAI.

### Search Providers

DuckDuckGo (free, default), Brave, SearXNG (self-hosted), Tavily, Firecrawl, Exa, Parallel, xAI Web Search.

### Memory Backends

Built-in (SQLite, default), mem0, ChromaDB, Qdrant, Byterover, Hindsight, Holographic, Honcho, OpenViking, RetainDB, Supermemory, AgentMemory.

### Terminal Backends

Local (default), Docker, SSH, Modal, Daytona, Singularity/Apptainer.

### Browser Providers

Local Chromium (default), Camofox (Firefox anti-detection), Browserbase, Firecrawl, Browser Use.

## Architecture

```
arli-core
  Agent Actor ← Mailbox ← User/System messages
       │
       ├── Context Manager (token counting, pressure detection)
       ├── Compaction (LLM summarization)
       ├── Policy Engine (allow/deny/needs_approval, rate limiting)
       ├── Hook System (lifecycle callbacks)
       │
       ├── Tools
       │   terminal, read_file, write_file, patch, search_files,
       │   http_get, web_search, browser, vision, voice/TTS,
       │   image_generate, video_generate, session_search,
       │   memory, delegate_task, execute_code
       │
       ├── Providers (36 via 3 adapters)
       │   OpenAI-compatible, Anthropic (prompt caching), OpenRouter
       │
       ├── Session Store (SQLite + WAL + FTS5)
       ├── Memory Store (persistent cross-session, 12 backends)
       ├── Skill Loader (SKILL.md from disk)
       │
       ├── Swarm (spawn/steer/kill/redirect/restart)
       ├── Cron Scheduler (cron expressions + human intervals)
       └── Sandbox (Linux namespaces)

arli-cli              CLI + TUI (ratatui)
arli-gateway          20-platform messaging daemon
arli-trading          Hyperliquid integration (hypersdk)
```

## Commands

```
arli chat              Interactive TUI chat
arli chat -q "..."     Single query
arli setup             Configure providers, platforms, settings
arli model             Change model/provider interactively
arli doctor            Check system health and configuration
arli update            Self-update from GitHub Releases
arli gateway start     Start messaging daemon
arli gateway stop      Stop daemon
arli gateway status    Daemon status
arli gateway log       View daemon logs
arli config show       Display current configuration
arli config set ...    Set config values
arli sessions          List recent sessions
arli cron add ...      Schedule a recurring task
arli cron list         List cron jobs
arli cron start        Start cron scheduler
arli serve             Health check HTTP server (port 3001)
arli mcp               MCP server on stdio
arli profile ...       Manage named profiles
arli webhook ...       Manage webhook subscriptions
arli checkpoint ...    Session checkpoint management
arli plugins list      List discovered plugins
arli completion        Shell completions (bash/zsh/fish)
```

## Swarm

```rust
let swarm = Swarm::new(provider_factory, policy, tools_factory);

let child_id = swarm.spawn(SwarmAgentConfig {
    name: "research-agent",
    initial_message: Some("Analyze cointegration for top-50 perps"),
    max_iterations: 20,
    restart_policy: Some(3),
}).await?;

swarm.get(&child_id).await?.pause().await;
swarm.get(&child_id).await?.send_message(
    AgentMessage::Redirect("Check BTC orderbook".into())
).await;
swarm.kill_all().await;
```

## Policy Engine

```toml
[policy.default]
trade_execution = "needs_approval"
file_delete = "deny"
shell = "allow"

[policy.agent.live_trading_agent]
trade_execution = "needs_approval"
max_position_size = "1000 USDC"
max_daily_trades = 50

[rate_limits.shell]
max_calls = 30
window_secs = 60
```

## Cron Jobs

```rust
let scheduler = CronScheduler::new();
scheduler.add_job(CronJob {
    id: "market-check",
    schedule_str: "0 */5 * * * *",
    prompt: "Check funding rates on top perps",
}).await;
```

## Configuration

```toml
# ~/.arli/config.toml
model = "deepseek-chat"
max_iterations = 90
tool_progress = "all"
compression_threshold = 0.5

[provider]
name = "deepseek"
api_key = "sk-..."

[session_reset]
mode = "inactivity_daily"
inactivity_minutes = 1440
daily_reset_hour = 4

[search]
provider = "duckduckgo"

[memory]
provider = "builtin"

[terminal]
backend = "local"

[browser]
provider = "local"

[gateway]
telegram_token = "..."
discord_bot_token = "..."
```

## Environment Variables

| Variable | Description |
|---|---|
| `DEEPSEEK_API_KEY` | DeepSeek API key |
| `OPENAI_API_KEY` | OpenAI API key |
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `GOOGLE_API_KEY` | Google AI Studio API key |
| `XAI_API_KEY` | xAI/Grok API key |
| `GITHUB_TOKEN` | GitHub Copilot token |
| `OPENROUTER_API_KEY` | OpenRouter API key |
| `ARLI_MODEL` | Override model name |
| `ARLI_MAX_ITERATIONS` | Override max iterations |
| `ARLI_HOME` | Data directory (default `~/.arli`) |
| `ARLI_LOG` | Log level filter |
| `TELEGRAM_BOT_TOKEN` | Telegram bot token |
| `DISCORD_BOT_TOKEN` | Discord bot token |
| `SLACK_BOT_TOKEN` | Slack bot token |
| `MATRIX_USER` / `MATRIX_PASSWORD` | Matrix credentials |
| `MS_TEAMS_APP_ID` / `MS_TEAMS_APP_PASSWORD` | Teams credentials |
| `EMAIL_IMAP_SERVER` / `EMAIL_USER` / `EMAIL_PASSWORD` | Email credentials |

Full env var list for all 36 providers and 20 platforms — run `arli setup` for guided configuration.

## Comparison

| | Hermes | Claude Code | ARLI |
|---|---|---|---|
| Language | Python | TypeScript | Rust |
| Binary size | ~200MB | ~150MB | ~20MB |
| Cold start | 2–5s | 1–3s | ~50ms |
| LLM providers | 37 | 3 | 36 |
| Messaging platforms | 21 | — | 20 |
| Swarm | Partial | Partial | Native |
| Trading | No | No | Native (Hyperliquid) |
| Self-update | No | Yes | Yes |
| Sandbox | Partial | Partial | Linux namespaces |
| Cron scheduler | Yes | No | Yes |
| MCP server | Yes | No | Yes |
| TTS | 16 providers | — | 3 providers |

## Project Structure

```
arli/
  Cargo.toml                 # Workspace root
  arli-core/                 # Core library
    src/
      agent.rs               # Agent actor loop
      swarm.rs               # Swarm orchestrator
      cron.rs                # Cron scheduler
      hooks.rs               # Lifecycle hooks
      memory.rs              # 12-backend memory store
      session.rs             # SQLite session store (FTS5)
      compaction.rs          # LLM conversation compaction
      context.rs             # Token counting + pressure
      policy.rs              # Approval engine + rate limiting
      sandbox.rs             # Linux namespace isolation
      telemetry.rs           # JSON structured logging
      providers/             # 36 LLM providers, 3 adapters
      tools/                 # 15 tool implementations
  arli-cli/                  # CLI + TUI binary
  arli-gateway/              # 20-platform messaging daemon
  arli-trading/              # Hyperliquid trading (hypersdk)
```

## Install

```bash
# One-liner (Linux / macOS)
curl -fsSL https://raw.githubusercontent.com/ARLI-Research/arli/main/install.sh | bash

# Or build from source
git clone https://github.com/ARLI-Research/arli
cd arli && cargo build --release
./target/release/arli setup
```

## Tests

```bash
cargo test -p arli-core      # 86 tests
cargo test --workspace       # all crates
```

## License

MIT
