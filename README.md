<p align="center">
  <img src="https://img.shields.io/badge/language-Rust-orange?style=flat-square" alt="Rust">
  <img src="https://img.shields.io/badge/binary-12MB-22d3ee?style=flat-square" alt="12MB">
  <img src="https://img.shields.io/badge/cold_start-50ms-34d399?style=flat-square" alt="50ms">
  <img src="https://img.shields.io/badge/sandbox-Landlock%2Bseccomp-ef4444?style=flat-square" alt="Landlock+seccomp">
  <img src="https://img.shields.io/badge/audit-OCSF-blue?style=flat-square" alt="OCSF">
  <img src="https://img.shields.io/badge/tests-254-green?style=flat-square" alt="254 tests">
  <img src="https://img.shields.io/badge/providers-36-fbbf24?style=flat-square" alt="36 providers">
  <img src="https://img.shields.io/badge/platforms-20-a78bfa?style=flat-square" alt="20 platforms">
  <img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="MIT">
</p>

<h1 align="center">ARLI</h1>
<h3 align="center">Rust-native AI Agent Harness — Production-grade agent infrastructure</h3>

<p align="center">
Single binary. Zero runtime dependencies. Kernel-level sandbox.<br>
Actor-based agent loop, swarm orchestration, 20-platform messaging gateway,<br>
Hyperliquid live trading, ENSO/ICP on-chain settlements.
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
                              │  Prometheus /metrics          │
                              └──────┬──────────────┬────────┘
                                     │              │
                    ┌────────────────▼──┐   ┌───────▼──────────────┐
                    │  Agent Actor      │   │  Swarm Orchestrator  │
                    │  Mailbox-driven   │   │  spawn/steer/kill    │
                    │  Context pressure │   │  redirect/restart    │
                    │  Auto-compaction  │   │  TaskRouter          │
                    │  Policy engine    │   │  fan-out/round-robin │
                    │  Hook system      │   └──────────────────────┘
                    └────────┬──────────┘
                             │              ┌──────────────────────┐
                             │              │  Cron Scheduler      │
                             │              │  cron · intervals    │
                             │              │  skill attachments   │
                             │              └──────────────────────┘
                    ┌────────▼──────────────────────────────┐
                    │  18 BUILT-IN TOOLS                    │
                    │  terminal  read  write  patch  search │
                    │  hashedit  ast_edit  resolve  browser │
                    │  web_search  vision  voice  memory    │
                    │  image_generate  video_generate       │
                    │  delegate  execute_code               │
                    └────────┬──────────────────────────────┘
                             │
          ┌──────────────────┼───────────────────┬─────────────────┐
          │                  │                   │                 │
  ┌───────▼──────┐  ┌────────▼───────┐  ┌───────▼──────────┐  ┌──▼─────────────┐
  │ 36 PROVIDERS │  │   STORAGE      │  │  LIVE TRADING    │  │  ENSO / ICP     │
  │ 3 adapters   │  │   SQLite+WAL   │  │  Hyperliquid SDK │  │  Attestation    │
  │ OpenAI-compat│  │   FTS5 search  │  │  Perps · Spot    │  │  Payments·Escrow│
  │ Anthropic    │  │   12 memory    │  │  WS fusion feed  │  │  Marketplace    │
  │ OpenRouter   │  │   backends     │  │  Metrics·Alerts  │  │  Oracle loop    │
  └──────────────┘  └────────────────┘  └──────────────────┘  └─────────────────┘
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

One daemon. Set env vars, start `arli gateway start`. Prometheus metrics at `/metrics`.

```
Telegram  Discord  Slack  WhatsApp  Matrix  Microsoft Teams  Email
Signal  SMS/Twilio  Google Chat  Feishu  DingTalk  LINE  IRC
WeCom  QQ  ntfy  SimpleX  Yuanbao  BlueBubbles/iMessage
```

### Tools — 18

| Tool | Description |
|---|---|
| `terminal` | Execute shell commands |
| `read_file` | Read with offset/limit pagination |
| `write_file` | Create/overwrite files |
| `patch` | Targeted find-and-replace edits |
| `hashedit` | Content-hash-anchored edits — zero whitespace battles |
| `ast_edit` | Structural code edits via AST pattern matching (ast-grep) |
| `resolve` | Accept/reject staged edits (preview-then-accept workflow) |
| `search_files` | Ripgrep-backed in-process file search |
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

### Edit Engine — 5 Layers

ARLI's editing pipeline eliminates the "string not found" problem entirely through layered precision:

| Layer | Tool | What it does |
|---|---|---|
| **1. In-process search** | `search_files` | Ripgrep linked into the binary — no fork/exec, no external dependency. ~10–100ms saved per search. |
| **2. Hash-anchored edits** | `hashedit` | Model identifies lines by SHA-256 content hash (8 chars), not by retyping text. Whitespace battles just stop happening. |
| **3. AST structural edits** | `ast_edit` | tree-sitter grammars for Rust/Python/TypeScript/JavaScript. Pattern `console.log($X)` matches the real AST node — not comments, not strings, not lookalikes. |
| **4. Preview-then-accept** | `resolve` | Edits are staged, not written. Agent shows a unified diff. You accept or reject. File untouched until approved. |
| **5. Stream rules (TTSR)** | `stream_rules` | Regex policies sit dormant until violated. No context tax on every turn. Match triggers injection + retry — max 3 retries per turn. |

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
arli dashboard -p PORT Web UI dashboard (metrics + logs)

arli key generate      Ed25519 keypair for ENSO attestation
arli key show          Show public key

arli enso setup        ENSO ICP integration setup
arli enso status       Key, config, active contracts
arli enso pay <id>     Build + sign attestation

arli marketplace rfq-create  Create RFQ for ENSO marketplace
arli marketplace rfq-list    List open RFQs
arli marketplace stats       Marketplace statistics

arli kanban create     Create task board
arli kanban add        Add card (backlog…)
arli kanban show       View board
arli kanban move       Move card between columns

arli mcp               MCP server on stdio
arli profile ...       Manage named profiles
arli webhook ...       Manage webhook subscriptions
arli checkpoint ...    Session checkpoint management
arli plugins list      List discovered plugins
arli completion bash   Generate shell completions
```

---

## Live Trading — Hyperliquid

Native Rust trading via `hypersdk`. Perpetuals + spot, WebSocket fusion feed, cointegration signals, Prometheus metrics.

```rust
// Live trading with OCSF attestation
use arli_trading::fusion_live::FusionLiveTrader;

let trader = FusionLiveTrader::new(config, metrics).await?;
trader.run().await; // WebSocket loop + signals + execution
```

**Capabilities:**
- 229 perpetual + 19 spot markets on Hyperliquid mainnet
- Cointegration signal generation (10 min candles)
- Fusion backtest integration (Sharpe 3.83 on historical)
- OCSF event hash embedded in every order for attestation
- Prometheus metrics: PnL, positions, signal count, execution latency

---

## ENSO / ICP Integration

On-chain agent settlements via Internet Computer Protocol. Full attestation loop: contract → execution → attestation → payment release. All atomic in one ICP call.

### Architecture

```
ARLI Agent                     ENSO Contracts (ICP)             ICP Ledger
─────                          ────────────────────             ──────────
│                              │
│ 1. Run job in sandbox        │
│    Landlock + seccomp        │
│                              │
│ 2. Build OCSF attestation    │
│    + ed25519 signature       │
│                              │
│ 3. submit_arli_payment ────→ │
│    (one ICP call)            │  verify attestation
│                              ├─ ed25519 verify
│                              ├─ binary hash match
│                              ├─ sandbox config match
│                              ├─ Landlock enforced
│                              ├─ Seccomp enforced
│                              │
│                              │  settlement triggered
│                              │  escrow released ────→ USDC/ICP
│                              │
│  ←─── ArliPaymentResult      │
│       status + tx_id + amount│
```

### CLI

```bash
# One-shot setup: keygen + config + registration
arli enso setup --contracts 7yv6j-ryaaa-aaaaa-qhheq-cai

# Check status
arli enso status

# Build and sign attestation for a contract
arli enso pay contract_1780372735456935314_4
```

### Oracle Mode

```bash
# Daemon that polls ENSO contracts and auto-attests
ENSO_CONTRACTS=contract_xxx,contract_yyy arli enso oracle
```

**ENS Contract endpoints used:**
- `submit_arli_payment(contract_id, attestation_json)` — atomic verify + settle + release
- `register_arli_agent(pubkey, binary_hash, name, capabilities)` — agent registration
- `get_canister_metadata()` — DID version, supported protocols

**Deployed canisters (ICP mainnet):**
- Contracts: `7yv6j-ryaaa-aaaaa-qhheq-cai`
- Frontend:  `7rwvv-hqaaa-aaaaa-qhhfa-cai`
- Bridge UI: `https://7rwvv-hqaaa-aaaaa-qhhfa-cai.icp0.io/#/app`

---

## Kanban Task Boards

SQLite-backed kanban boards with WIP limits, priorities, and agent assignment.

```bash
arli kanban create "Sprint 1" -d "ENSO integration"
arli kanban add <board_id> backlog "Fix attestation bug" -p critical
arli kanban show
arli kanban move <card_id> in_progress
arli kanban stats
```

**Features:**
- 4-column workflow: backlog → in_progress → review → done
- WIP limits per column (max 3 in_progress)
- Card priorities: low, medium, high, critical
- Agent assignment per card
- Board statistics: counts, blocked cards, cycle time

---

## Auto-Optimization (DSPy-like)

Pure Rust prompt and strategy optimization. No Python dependency.

```rust
use arli_core::optimization::{PromptOptimizer, StrategyOptimizer, AutoFewShot};

// Optimize a prompt against a metric
let optimizer = PromptOptimizer::new(evaluator);
let improved = optimizer.optimize(&base_prompt, &metric, 10)?;

// Auto-select few-shot examples from history
let few_shot = AutoFewShot::from_history(&session_store, "trading analysis", 5)?;
```

**Optimizers:**
- PromptOptimizer — iterative refinement against evaluation metric
- StrategyOptimizer — tool selection and execution order optimization
- AutoFewShot — automatic example selection from session history

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

**Coordination primitives:**
- TaskRouter — fan-out tasks to available agents
- AgentRole — researcher, executor, reviewer
- Round-robin load distribution
- Task queue with priority

---

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
| Binary size | ~200MB | ~150MB | ~12MB |
| Cold start | 2–5s | 1–3s | ~50ms |
| LLM providers | 37 | 3 | 36 |
| Messaging platforms | 21 | — | 20 |
| Swarm orchestration | Partial | Partial | Native (TaskRouter, fan-out, round-robin) |
| Cron scheduler | Native | — | Native |
| MCP server | Native | — | Native |
| Self-update | — | Native | Native |
| Live trading | — | — | Native (Hyperliquid, fusion, WS) |
| On-chain settlement | — | — | Native (ICP, ENSO, attestation loop) |
| Kanban boards | — | — | Native (SQLite, WIP limits) |
| Web dashboard | Native | — | Native (axum + htmx) |
| Auto-optimization | — | — | Native (DSPy-like, pure Rust) |
| Sandbox | Partial | Partial | Landlock + seccomp (kernel-level) |
| Audit logging | — | — | OCSF (SIEM-compatible) |
| Inference routing | — | — | Round-robin, fallback, affinity |
| TTS | 16 providers | — | 3 providers |
| Image generation | — | — | 2 providers |
| Tests | — | — | 254 (0 fail) |

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

[enso]
icp_gateway = "https://icp0.io"
registry_canister_id = "ENSO_REGISTRY_CANISTER_ID"
contracts_canister_id = "7yv6j-ryaaa-aaaaa-qhheq-cai"
arli_public_key = "..."

[trading]
hyperliquid_wallet = "..."
hyperliquid_rpc = "https://api.hyperliquid.xyz"
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
cargo test -p arli-core       # 213 tests, 0 fail
cargo test --workspace        # all crates
```

## License

MIT
