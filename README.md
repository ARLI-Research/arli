<p align="center">
  <img src="https://img.shields.io/badge/language-Rust-orange?style=flat-square" alt="Rust">
  <img src="https://img.shields.io/badge/binary-12MB-22d3ee?style=flat-square" alt="12MB">
  <img src="https://img.shields.io/badge/cold_start-50ms-34d399?style=flat-square" alt="50ms">
  <img src="https://img.shields.io/badge/sandbox-Landlock%2Bseccomp-ef4444?style=flat-square" alt="Landlock+seccomp">
  <img src="https://img.shields.io/badge/audit-OCSF-blue?style=flat-square" alt="OCSF">
  <img src="https://img.shields.io/badge/providers-36-fbbf24?style=flat-square" alt="36 providers">
  <img src="https://img.shields.io/badge/platforms-20-a78bfa?style=flat-square" alt="20 platforms">
  <img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="MIT">
</p>

<h1 align="center">ARLI</h1>
<h3 align="center">Rust-native AI Agent Harness</h3>

<p align="center">
Production-grade agent infrastructure. Single binary, zero runtime dependencies.<br>
Actor-based agent loop, swarm orchestration, 20-platform messaging gateway.
</p>

---

## Architecture

```
                             ┌─────────────────────────────────────────────┐
                             │  EXTERNAL — 20 Messaging Platforms           │
                             │  Telegram  Discord  Slack  WhatsApp  Matrix  │
                             │  Teams  Email  Signal  LINE  Feishu  WeCom   │
                             │  QQ  SMS  Google Chat  DingTalk  IRC  ntfy   │
                             │  SimpleX  Yuanbao  BlueBubbles  Webhooks     │
                             └──────────────┬──────────────────────────────┘
                                            │
                              ┌─────────────▼────────────────┐
                              │  arli-gateway Daemon          │
                              │  systemd · auto-restart       │
                              │  per-chat agent sessions      │
                              └──────┬──────────────┬────────┘
                                     │              │
                    ┌────────────────▼──┐   ┌───────▼──────────────┐
                    │  Agent Actor      │   │  Swarm Orchestrator  │
                    │  Mailbox-driven   │   │  spawn/steer/kill    │
                    │  Context pressure │   │  redirect/restart    │
                    │  Auto-compaction  │   └──────────────────────┘
                    │  Policy engine    │
                    │  Hook system      │   ┌──────────────────────┐
                    └────────┬──────────┘   │  Cron Scheduler      │
                             │              │  cron · intervals    │
                             │              │  skill attachments   │
                             │              └──────────────────────┘
                    ┌────────▼──────────────────────────────┐
                    │  15 BUILT-IN TOOLS                    │
                    │  terminal  read  write  patch  search │
                    │  browser  web_search  vision  voice   │
                    │  image_generate  video_generate       │
                    │  memory  delegate  execute_code       │
                    └────────┬──────────────────────────────┘
                             │
          ┌──────────────────┼───────────────────┐
          │                  │                   │
  ┌───────▼──────┐  ┌────────▼───────┐  ┌───────▼──────────┐
  │ 36 PROVIDERS │  │   STORAGE      │  │  TRADING (WIP)   │
  │ 3 adapters   │  │   SQLite+WAL   │  │  Hyperliquid SDK │
  │ OpenAI-compat│  │   FTS5 search  │  │  Perps · Spot    │
  │ Anthropic    │  │   12 memory    │  │  WebSocket live  │
  │ OpenRouter   │  │   backends     │  └──────────────────┘
  └──────────────┘  └────────────────┘
```

[Full interactive diagram — dark-themed SVG](/docs/architecture.html)

---

## Quick Start

```bash
# Install (Linux / macOS, no Rust toolchain required)
curl -fsSL https://raw.githubusercontent.com/ARLI-Research/arli/main/install.sh | bash

# Configure — interactive setup for providers, platforms, settings
arli setup

# Start chatting
arli chat                     # Interactive TUI
arli chat -q "Explain Rust"   # Single query

# Start the messaging gateway (20 platforms)
arli gateway start

# Self-update
arli update
```

---

## Feature Matrix

### LLM Providers — 36

All routed through 3 adapter traits. OpenAI-compatible providers share one adapter. Anthropic has native ephemeral prompt caching.

```
DeepSeek  OpenAI  Anthropic  OpenRouter  Google AI  xAI/Grok  Copilot
Nous  Novita  Qwen  MiMo  Tencent  NVIDIA  HuggingFace  GLM
Kimi/Moonshot  StepFun  MiniMax (global + China)  LM Studio  Ollama
Bedrock  Azure  Arcee  GMI Cloud  Kilo Code  OpenCode Zen/Go
Alibaba Cloud  Custom endpoint
```

### Messaging Gateway — 20 Platforms

One daemon. Set env vars, start `arli gateway start`. All platforms run in parallel.

```
Telegram  Discord  Slack  WhatsApp  Matrix  Microsoft Teams  Email
Signal  SMS/Twilio  Google Chat  Feishu  DingTalk  LINE  IRC
WeCom  QQ  ntfy  SimpleX  Yuanbao  BlueBubbles/iMessage
```

### Tools — 15

| Tool | Description |
|---|---|
| `terminal` | Execute shell commands |
| `read_file` | Read with offset/limit pagination |
| `write_file` | Create/overwrite files |
| `patch` | Targeted find-and-replace edits |
| `search_files` | Ripgrep-backed file search |
| `web_search` | Search via 8 providers (DuckDuckGo, Brave, etc.) |
| `browser` | Browser automation (5 backends) |
| `vision` | Image analysis / OCR |
| `text_to_speech` | Edge TTS (free) + OpenAI TTS + local engines |
| `image_generate` | FAL.ai + DALL-E 3 |
| `video_generate` | Premium feature stub |
| `session_search` | FTS5 search across past sessions |
| `memory` | Persistent memory (add/replace/remove/search) |
| `delegate_task` | Spawn sub-agents in parallel |
| `execute_code` | Run Python with tool access |

### Agent Settings

| Setting | Default | Options |
|---|---|---|
| Max iterations | 90 | 1–200 |
| Tool progress | all | off / new / all / verbose |
| Compression threshold | 0.5 | 0.5–0.95 |
| Session reset | inactivity_daily | inactivity_daily / inactivity / daily / never |

### Backend Configuration

| Category | Default | Options |
|---|---|---|
| **Search** | DuckDuckGo | Brave, SearXNG, Tavily, Firecrawl, Exa, Parallel, xAI |
| **Memory** | builtin | mem0, ChromaDB, Qdrant, Byterover, Hindsight, Holographic, Honcho, OpenViking, RetainDB, Supermemory, AgentMemory |
| **Terminal** | local | Docker, SSH, Modal, Daytona, Singularity |
| **Browser** | local Chromium | Camofox, Browserbase, Firecrawl, Browser Use |
| **TTS** | Edge (free) | OpenAI, local (espeak/flite/say) |

---

## Commands

```
arli chat              Interactive TUI chat
arli chat -q "..."     Single query
arli setup             Interactive configuration wizard
arli model             Change model/provider interactively
arli doctor            System health check
arli update            Self-update from GitHub Releases

arli gateway start     Start messaging daemon
arli gateway stop      Stop daemon
arli gateway status    Daemon status
arli gateway log -l N  View last N log lines

arli config show       Display current configuration
arli config set k v    Set config value
arli config path       Print config file location

arli sessions          List recent sessions
arli cron add          Schedule recurring task
arli cron list         List all cron jobs
arli cron start        Start cron scheduler
arli serve -p PORT     Health check HTTP server

arli mcp               MCP server on stdio
arli profile ...       Manage named profiles
arli webhook ...       Manage webhook subscriptions
arli checkpoint ...    Session checkpoint management
arli plugins list      List discovered plugins
arli completion bash   Generate shell completions
```

---

## Swarm API

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
    AgentMessage::Redirect("New goal: check BTC orderbook".into())
).await;
swarm.kill_all().await;
```

## Policy Engine

Policy-driven security rules with host glob matching. Define allowed filesystem paths, network targets, and process capabilities in YAML.

```yaml
# default_policy.yaml (embedded in binary)
policies:
  - name: "restricted-agent"
    filesystem:
      read_only: ["/workspace", "/tmp"]
      read_write: ["/tmp/output"]
      allow_network: false
    process:
      disallow_exec: true
  - name: "network-agent"
    network:
      allowed_hosts: ["api.example.com", "*.github.com"]
      allowed_ports: [443, 80]
    rate_limits:
      max_requests_per_minute: 60

# Host glob matching
# *.example.com    → matches api.example.com, www.example.com
# 10.0.*           → matches any host in 10.0.0.0/16
# *                → matches everything
```

## Sandbox

Kernel-level isolation via Linux Landlock + seccomp BPF. Agent commands run in a restricted environment that physically cannot escape.

```rust
let sandbox = Sandbox::from_policy(&policy)?;

// Execute a command inside the sandbox
// Chain: seccomp → Landlock → privilege drop → exec
let output = sandbox.execute_isolated("ls /workspace")?;
```

**Enforcement layers (applied in order):**

1. **Seccomp BPF** — syscall whitelist. Blocks dangerous calls (`ptrace`, `mount`, `reboot`, `kexec_load`, etc.)
2. **Landlock** — filesystem access control at kernel level. White-list directories, deny everything else
3. **Privilege drop** — `initgroups` → `setgid` → `setuid` before `exec`. Process runs as unprivileged user

All layers activate via `Command::pre_exec()` — attack surface is closed before the child process starts.

## Inference Routing

Smart multi-provider routing with automatic failover:

```rust
let registry = ProviderRegistry::from_embedded()?;

// Round-robin across providers
registry.route(RouteStrategy::RoundRobin, &request).await?;

// Fallback chain: primary → secondary → tertiary
registry.route(RouteStrategy::Fallback, &request).await?;

// Affinity: same provider for same user/session
registry.route(RouteStrategy::Affinity("user-123"), &request).await?;
```

11 providers: DeepSeek, OpenAI, Anthropic, Groq, Together, Fireworks, xAI, Google, Mistral, OpenRouter, Perplexity. All defined in YAML, switchable at runtime.

## Audit Logging

All agent actions logged in OCSF (Open Cybersecurity Schema Framework) format — the industry standard for security telemetry.

```json
{
  "class_uid": 6007,
  "class_name": "Agent Activity",
  "activity_name": "Execute",
  "activity_id": 1,
  "time": 1717286400,
  "agent": {
    "name": "arli-core",
    "policy": "restricted-agent"
  },
  "command": "ls /workspace",
  "result": "success",
  "sandbox": {
    "landlock_enforced": true,
    "seccomp_enforced": true,
    "uid": 65534
  }
}
```

Compatible with SIEM systems, log aggregators, and security monitoring pipelines.

## Cron Jobs

```rust
scheduler.add_job(CronJob {
    id: "market-check",
    schedule_str: "0 */5 * * * *",
    prompt: "Check funding rates on top perps",
}).await;
```

---

## Comparison

| | Hermes | Claude Code | ARLI |
|---|---|---|---|
| Language | Python | TypeScript | Rust |
| Binary size | ~200MB | ~150MB | ~20MB |
| Cold start | 2–5s | 1–3s | ~50ms |
| LLM providers | 37 | 3 | 36 |
| Messaging platforms | 21 | — | 20 |
| Swarm orchestration | Partial | Partial | Native |
| Cron scheduler | Native | — | Native |
| MCP server | Native | — | Native |
| Self-update | — | Native | Native |
| Trading | — | — | Native (Hyperliquid) |
| Sandbox | Partial | Partial | Landlock + seccomp |
| Audit logging | — | — | OCSF |
| Inference routing | — | — | Round-robin, fallback, affinity |
| TTS | 16 providers | — | 3 providers |
| Image generation | — | — | 2 providers |

---

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
```

---

## Install

```bash
# One-liner (Linux / macOS)
curl -fsSL https://raw.githubusercontent.com/ARLI-Research/arli/main/install.sh | bash

# From source
git clone https://github.com/ARLI-Research/arli
cd arli && cargo build --release
./target/release/arli setup
```

## Tests

```bash
cargo test -p arli-core      # 134 tests
cargo test --workspace       # all crates
```

## License

MIT
