# ARLI — Rust-native AI Agent Harness

Universal agent infrastructure. Actor-based, swarm-first. ~20MB binary, ~50ms cold start.

## Quick Start

### 1. Install

```bash
curl -fsSL https://raw.githubusercontent.com/ARLI-Research/arli/main/install.sh | bash
```

Requires: Linux or macOS. No Rust toolchain needed — downloads pre-built binary.
Falls back to building from source if no binary for your platform.

### 2. Configure

```bash
arli setup
```

Or manually:

```bash
export DEEPSEEK_API_KEY="sk-..."
# or: OPENAI_API_KEY, ANTHROPIC_API_KEY
```

### 3. Chat

```bash
arli chat                  # Interactive TUI
arli chat -q "What is Rust?"  # Single query
```

### Build from source

```bash
git clone https://github.com/ARLI-Research/arli
cd arli
cargo build --release
./target/release/arli setup  # configure API keys
./target/release/arli chat   # start chatting
```
./target/release/arli-gateway
```

## Architecture

```
arli-core
  Agent Actor <- Mailbox <- User/System messages
       |
       ├── Context Manager (tiktoken-rs, pressure detection)
       ├── Compaction (LLM summarization)
       ├── Policy Engine (allow/deny/needs_approval, rate limiting, budget)
       ├── Hook System (lifecycle callbacks)
       |
       ├── Tools (10 built-in)
       │   read_file, write_file, patch, shell,
       │   search_files, http_get, browser,
       │   session_search, memory, delegate_task
       |
       ├── Providers (3 adapters)
       │   OpenAI, DeepSeek, Anthropic (w/ prompt caching)
       |
       ├── Session Store (SQLite + WAL + FTS5)
       ├── Memory Store (persistent cross-session)
       ├── Skill Loader (SKILL.md from disk)
       |
       ├── Swarm (spawn/steer/kill/redirect/restart)
       ├── Cron Scheduler (cron expressions + human intervals)
       └── Sandbox (Linux namespaces)

arli-cli              CLI + TUI (ratatui)
arli-gateway          Telegram long-poll bot
arli-trading          Hyperliquid integration (hypersdk, WIP)
```

## Features

### Agent Loop
- Actor model: mailbox-driven, externally steerable
- Context pressure: automatic token counting via tiktoken-rs
- Auto-compaction: LLM summarization at critical pressure
- Prompt injection: auto-loads AGENTS.md / CLAUDE.md from workdir
- Streaming: token-by-token via `chat_stream()`
- Budget tracking: token/time/dollar limits with grace period

### Tools

| Tool | Description |
|------|-------------|
| `read_file` | Read files with offset/limit pagination |
| `write_file` | Create/overwrite files |
| `patch` | Targeted find-and-replace edits with diffs |
| `shell` | Execute shell commands |
| `search_files` | Ripgrep-based file search |
| `http_get` | HTTP GET with auto-truncation |
| `browser` | Fetch web pages, HTML-to-text extraction |
| `session_search` | FTS5 full-text search across sessions |
| `memory` | Persistent memory (add/replace/remove/search) |
| `delegate_task` | Spawn child agents |

### LLM Providers
- OpenAI: GPT-4o, GPT-4-turbo
- DeepSeek: deepseek-chat, deepseek-reasoner
- Anthropic: Claude 3.5 Sonnet, Claude 3 Opus (w/ ephemeral prompt caching)

### Swarm

```rust
let swarm = Swarm::new(provider_factory, policy, tools_factory);

// Spawn with restart policy
let child_id = swarm.spawn(SwarmAgentConfig {
    name: "research-agent",
    initial_message: Some("Analyze cointegration for top-50 perps"),
    max_iterations: 20,
    restart_policy: Some(3),  // restart up to 3 times on failure
}).await?;

// Steer: pause, resume, kill, redirect
swarm.get(&child_id).await?.pause().await;
swarm.get(&child_id).await?.resume().await;
swarm.get(&child_id).await?.send_message(
    AgentMessage::Redirect("New goal: check BTC orderbook".into())
).await;
swarm.kill_all().await;
```

### Policy Engine

```toml
[policy.default]
trade_execution = "needs_approval"
file_delete = "deny"
shell = "allow"

[policy.agent.live_trading_agent]
trade_execution = "needs_approval"
max_position_size = "1000 USDC"
max_daily_trades = 50

# Rate limiting per tool
[rate_limits.shell]
max_calls = 30
window_secs = 60
```

### Cron Jobs

```rust
let scheduler = CronScheduler::new();
scheduler.add_job(CronJob {
    id: "market-check",
    schedule_str: "0 */5 * * * *",  // every 5 min
    prompt: "Check funding rates on top perps",
}).await;
```

### Skills (disk-based)

```
~/.arli/skills/
  execute-trade/SKILL.md    # YAML frontmatter + markdown body
  code-review/SKILL.md
  data-analysis.toml        # Alternative TOML format
```

### Memory (persistent across sessions)

```
Agent: [memory action='add'    target='memory' content='Project uses Rust 1.95']
Agent: [memory action='search' query='Rust toolchain']
Agent: [memory action='get'    target='user']
```

## Comparison

| | Hermes | Claude Code | ARLI |
|---|--------|-------------|------|
| Language | Python | TypeScript | Rust |
| Binary size | ~200MB | ~150MB | ~20MB |
| Cold start | 2-5s | 1-3s | ~50ms |
| Swarm | Partial | Partial | Native |
| Trading | No | No | Native |
| Sandbox | Partial | Partial | Namespaces |
| Typed skills | Partial | Partial | JSON Schema |

## Project Structure

```
arli/
  Cargo.toml                 # Workspace root
  arli-core/                 # Core library (17 modules)
    src/
      agent.rs               # Agent actor loop
      swarm.rs               # Swarm orchestrator
      cron.rs                # Cron scheduler
      hooks.rs               # Lifecycle hooks
      memory.rs              # Persistent memory store
      session.rs             # SQLite session store
      compaction.rs          # LLM conversation compaction
      context.rs             # Token counting + pressure
      skill_loader.rs        # Skills from disk
      telemetry.rs           # JSON structured logging
      policy.rs              # Approval engine + rate limiting
      sandbox.rs             # Linux namespace isolation
      providers/             # OpenAI + DeepSeek + Anthropic adapters
      tools/                 # 10 tool implementations
  arli-cli/                  # CLI + TUI binary
  arli-gateway/              # Telegram bot binary
  arli-trading/              # Trading integration (WIP)
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `DEEPSEEK_API_KEY` | — | DeepSeek API key |
| `OPENAI_API_KEY` | — | OpenAI API key |
| `ANTHROPIC_API_KEY` | — | Anthropic API key |
| `ARLI_MODEL` | `deepseek-chat` | Default model |
| `ARLI_PROVIDER` | `deepseek` | Provider name |
| `ARLI_BASE_URL` | — | Custom API base URL |
| `ARLI_HOME` | `~/.arli` | Data directory |
| `ARLI_LOG` | `info` | Log level filter |
| `TELEGRAM_BOT_TOKEN` | — | Telegram bot token |

## Tests

```bash
cargo test -p arli-core      # 44 tests
cargo test --workspace       # all crates
```

## License

MIT
