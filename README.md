# 🔥 ARLI — Rust-native AI Agent Harness

> *"ARLI stole fire from the gods and gave it to humanity."*

**Universal agent harness. Swarm-first. Trading-grade. ~20MB binary, ~50ms cold start.**

```
prom chat                    # TUI chat with any LLM
prom chat -q "..."          # Single-shot query
arli-gateway                # Telegram bot
```

---

## Why ARLI

| | Hermes | Claude Code | ARLI |
|---|--------|-------------|------------|
| Language | Python | TypeScript | **Rust** |
| Binary size | ~200MB | ~150MB | **~20MB** |
| Cold start | 2-5s | 1-3s | **~50ms** |
| Swarm | ★★ | ★★ | **★★★★★** |
| Trading | ❌ | ❌ | **✅** |
| Sandbox | ★★ | ★★ | **★★★★** |
| Typed skills | ★★ | ★★ | **★★★★★** |

## Quick Start

```bash
# Clone and build
git clone https://github.com/ARLI-Research/arli
cd arli
cargo build --release

# Configure
export DEEPSEEK_API_KEY="sk-..."
export PROMETHEUS_MODEL="deepseek-chat"

# Chat
./target/release/prom chat

# Single query
./target/release/prom chat -q "What is Rust?"

# Telegram bot
export TELEGRAM_BOT_TOKEN="123:abc"
./target/release/arli-gateway
```

## Architecture

```
┌──────────────────────────────────────────────────────┐
│                  arli-core                     │
│                                                      │
│  Agent Actor ← Mailbox ← User/System messages       │
│       │                                              │
│       ├── Context Manager (tiktoken-rs, pressure)    │
│       ├── Compaction (LLM summarization)             │
│       ├── Policy Engine (allow/deny/needs_approval)  │
│       ├── Hook System (lifecycle callbacks)          │
│       │                                              │
│       ├── Tools (10 built-in)                        │
│       │   read_file, write_file, patch, shell,       │
│       │   search_files, http_get, browser,           │
│       │   session_search, memory, delegate_task      │
│       │                                              │
│       ├── Providers (3 adapters)                     │
│       │   OpenAI, DeepSeek, Anthropic                │
│       │                                              │
│       ├── Session Store (SQLite + WAL + FTS5)        │
│       ├── Memory Store (persistent cross-session)    │
│       ├── Skill Loader (SKILL.md / .toml from disk)  │
│       │                                              │
│       ├── Swarm Orchestrator                         │
│       │   spawn/steer/kill/recovery                  │
│       │                                              │
│       ├── Cron Scheduler                             │
│       │   cron expressions + human intervals         │
│       │                                              │
│       └── Sandbox (Linux namespaces)                 │
│                                                      │
├──────────────────────────────────────────────────────┤
│  arli-cli         │  arli-gateway        │
│  CLI + TUI (ratatui)    │  Telegram long-poll bot    │
├──────────────────────────────────────────────────────┤
│  arli-trading                                   │
│  Hyperliquid integration (hypersdk-ready)             │
└──────────────────────────────────────────────────────┘
```

## Features

### Agent Loop
- **Actor model** — mailbox-driven, externally steerable
- **Context pressure** — automatic token counting with tiktoken-rs
- **Auto-compaction** — LLM summarization at critical pressure
- **Prompt injection** — auto-loads AGENTS.md / CLAUDE.md from workdir
- **Streaming** — token-by-token via `chat_stream()`

### 10 Built-in Tools

| Tool | Description |
|------|-------------|
| `read_file` | Read files with offset/limit pagination |
| `write_file` | Create/overwrite files |
| `patch` | Targeted find-and-replace edits with diffs |
| `shell` | Execute shell commands |
| `search_files` | Ripgrep-based file search |
| `http_get` | HTTP GET with auto-truncation |
| `browser` | Fetch web pages, HTML→text extraction |
| `session_search` | FTS5 full-text search |
| `memory` | Persistent memory (add/replace/remove/search) |
| `delegate_task` | Spawn child agents (spawn_and_wait/list/kill) |

### 3 LLM Providers
- **OpenAI** — GPT-4o, GPT-4-turbo, o1
- **DeepSeek** — deepseek-chat, deepseek-reasoner
- **Anthropic** — Claude 3.5 Sonnet, Claude 3 Opus

### Swarm Orchestration
```rust
let swarm = Swarm::new(provider_factory, policy, tools_factory);
let child_id = swarm.spawn(SwarmAgentConfig {
    name: "research-agent",
    initial_message: Some("Analyze cointegration for top-50 perps"),
    max_iterations: 20,
    restart_policy: Some(3), // restart up to 3 times on failure
}).await?;

// Steer: pause, resume, kill
swarm.get(&child_id).await?.pause().await;
swarm.get(&child_id).await?.resume().await;
swarm.kill_all().await;
```

### Cron Jobs
```rust
let scheduler = CronScheduler::new();
scheduler.add_job(CronJob {
    id: "market-check",
    schedule_str: "0 */5 * * * *", // every 5 min
    prompt: "Check funding rates on top perps",
    ...
}).await;
```

### Memory (persistent across sessions)
```
Agent: [memory action='add' target='memory' content='Project uses Rust 1.95']
Agent: [memory action='search' query='Rust toolchain']
Agent: [memory action='get' target='user']
```

### Skills from Disk
```
~/.arli/skills/
├── execute-trade/
│   └── SKILL.md          # YAML frontmatter + markdown body
├── code-review/
│   └── SKILL.md
└── data-analysis.toml     # Alternative TOML format
```

### Policy Engine
```toml
[policy.default]
trade_execution = "needs_approval"
file_delete = "deny"
shell = "needs_approval"

[policy.agent.live_trading_agent]
trade_execution = "needs_approval"
max_position_size = "1000 USDC"
max_daily_trades = 50
```

## Project Structure

```
arli/
├── Cargo.toml                 # Workspace root
├── arli-core/           # Core library (17 modules)
│   └── src/
│       ├── agent.rs           # Agent actor loop
│       ├── swarm.rs           # Swarm orchestrator
│       ├── cron.rs            # Cron scheduler
│       ├── hooks.rs           # Lifecycle hooks
│       ├── memory.rs          # Persistent memory store
│       ├── session.rs         # SQLite session store
│       ├── compaction.rs      # LLM conversation compaction
│       ├── context.rs         # Token counting + pressure
│       ├── skill_loader.rs    # Skills from disk
│       ├── telemetry.rs       # JSON structured logging
│       ├── policy.rs          # Approval engine
│       ├── sandbox.rs         # Linux namespace isolation
│       ├── providers/         # OpenAI + Anthropic adapters
│       └── tools/             # 10 tool implementations
├── arli-cli/            # CLI + TUI binary
├── arli-gateway/        # Telegram bot binary
└── arli-trading/        # Trading integration (WIP)
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `DEEPSEEK_API_KEY` | — | DeepSeek API key |
| `OPENAI_API_KEY` | — | OpenAI API key |
| `ANTHROPIC_API_KEY` | — | Anthropic API key |
| `PROMETHEUS_MODEL` | `deepseek-chat` | Default model |
| `PROMETHEUS_PROVIDER` | `deepseek` | Provider name |
| `PROMETHEUS_BASE_URL` | — | Custom API base URL |
| `PROMETHEUS_HOME` | `~/.arli` | Data directory |
| `PROMETHEUS_LOG` | `info` | Log level filter |
| `TELEGRAM_BOT_TOKEN` | — | Telegram bot token |

## Tests

```bash
cargo test -p arli-core    # 44 tests
cargo test --workspace           # all crates
```

## License

MIT
