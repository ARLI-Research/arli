<p align="center">
  <img src="https://img.shields.io/badge/language-Rust-orange?style=flat-square" alt="Rust">
  <img src="https://img.shields.io/badge/binary-12MB-22d3ee?style=flat-square" alt="12MB">
  <img src="https://img.shields.io/badge/cold_start-50ms-34d399?style=flat-square" alt="50ms">
  <img src="https://img.shields.io/badge/sandbox-Landlock%2Bseccomp-ef4444?style=flat-square" alt="Landlock+seccomp">
  <img src="https://img.shields.io/badge/attestation-ENSO%2FICP-8b5cf6?style=flat-square" alt="ENSO">
  <img src="https://img.shields.io/badge/audit-OCSF-blue?style=flat-square" alt="OCSF">
  <img src="https://img.shields.io/badge/tests-448-green?style=flat-square" alt="448 tests">
  <img src="https://img.shields.io/badge/providers-36-fbbf24?style=flat-square" alt="36 providers">
  <img src="https://img.shields.io/badge/platforms-20-a78bfa?style=flat-square" alt="20 platforms">
  <img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="MIT">
</p>

<h1 align="center">ARLI</h1>
<h3 align="center">Production-Grade AI Agent Harness — Rust. Kernel Sandbox. On-Chain Settlement.</h3>

<p align="center">
Single 12MB binary. Zero runtime dependencies beyond the kernel.<br>
Actor-based agent loop. Swarm orchestration. 20-platform messaging gateway.<br>
Landlock + seccomp sandbox. OCSF audit trail. ENSO/ICP on-chain attestation.<br>
Hyperliquid live trading. Self-healing experiential memory. Autonomous oracle loop.
</p>

---

## What ARLI Is

ARLI is a **complete agent harness** — the infrastructure layer between an LLM and the real world. It doesn't just call models. It:

- **Executes agent actions** inside a kernel-level sandbox (Landlock + seccomp)
- **Verifies work** before claiming it's done (compile → lint → test → fuzz pipeline)
- **Attests on-chain** via ENSO/ICP — cryptographic proof of what ran, how, and under what policy
- **Learns from failures** — experiential memory that remembers fixes and auto-applies them
- **Survives faults** — workspace snapshots with rollback on failure, clean retries every time
- **Explains failures** — classifies why something broke (sandbox killed it? agent wrote bad code? ENSO rejected?)
- **Governs itself** — risk scoring, approval queues, audit trail for every action
- **Self-analyzes** — `arli harness analyze` reads telemetry and tells you what to fix

Everything is Rust. Everything is local. No Python. No Docker. No cloud dependency.

---

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│  EXTERNAL — 20 Messaging Platforms                            │
│  Telegram  Discord  Slack  WhatsApp  Matrix  Teams  Email     │
│  Signal  LINE  Feishu  WeCom  QQ  SMS  Google Chat  DingTalk  │
│  IRC  ntfy  SimpleX  Yuanbao  BlueBubbles  Webhooks           │
└──────────────────────┬───────────────────────────────────────┘
                       │
       ┌───────────────▼──────────────────────────┐
       │  arli-gateway Daemon                      │
       │  Built-in daemon (no systemd required)    │
       │  Per-chat agent sessions                  │
       │  Prometheus /metrics  /healthz  /readyz   │
       └──────┬──────────────┬─────────────────────┘
              │              │
┌─────────────▼──┐  ┌────────▼─────────────────────────┐
│  Agent Actor   │  │  Swarm Orchestrator               │
│  Mailbox-driven│  │  spawn/steer/kill/restart         │
│  Context press.│  │  TaskRouter · fan-out · affinity  │
│  Auto-compact. │  │  Shared Memory (agent blackboard) │
│  Hook system   │  │  Agent registry (SQLite)          │
└───────┬────────┘  └───────────────────────────────────┘
        │
        │  ┌──────────────────────────────────────────────┐
        │  │  HARNESS LAYER — executes BEFORE attestation  │
        │  │                                               │
        │  │  ┌──────────────┐  ┌───────────────────────┐ │
        │  │  │ Verification │  │ Fault-Tolerant         │ │
        │  │  │ Pipeline     │  │ Sandbox                │ │
        │  │  │ compile→lint │  │ snapshot→execute→      │ │
        │  │  │ →test→fuzz   │  │ success=commit         │ │
        │  │  └──────────────┘  │ failure=rollback        │ │
        │  │                    └───────────────────────┘ │
        │  │  ┌──────────────┐  ┌───────────────────────┐ │
        │  │  │ Governance   │  │ Failure Attribution    │ │
        │  │  │ Engine       │  │                        │ │
        │  │  │ risk score→  │  │ SandboxKilled?         │ │
        │  │  │ approve/deny │  │ EnsoRejected?          │ │
        │  │  │ /queue       │  │ VerificationFailed?    │ │
        │  │  └──────────────┘  └───────────────────────┘ │
        │  └──────────────────────────────────────────────┘
        │
┌───────▼──────────────────────────────────────────────────┐
│  18 BUILT-IN TOOLS                                        │
│  terminal  read  write  patch  search  hashedit  ast_edit │
│  resolve  browser  web_search  vision  voice  memory      │
│  image_generate  video_generate  delegate  execute_code   │
└───────┬──────────────────────────────────────────────────┘
        │
┌───────┼──────────────┬──────────────────┬─────────────────┐
│       │              │                  │                 │
│  36 PROVIDERS   STORAGE         LIVE TRADING      ENSO/ICP│
│  3 adapters     SQLite+WAL      Hyperliquid SDK   Attest. │
│  OpenAI-compat  FTS5 search     Perps · Spot      Payment │
│  Anthropic      12 backends     WS fusion feed    Oracle  │
│  OpenRouter     Skill loader    Cointegration     loop    │
└───────────────────────────────────────────────────────────┘

LEARNING LAYER (persisted to ~/.arli/)
┌───────────────────────────────────────────────────────────┐
│  Experiential Memory    Harness Telemetry    Task Contracts│
│  failure→fix lessons    per-tool metrics     upfront work │
│  auto-apply on retry    failure rates        declarations │
│  verified/unverified    memory hit rate      SHA-256 hash │
│  stale pruning          policy violations    in attest.   │
│                                                           │
│  Task State             Quality Critic       Evolution CLI│
│  phased execution       heuristic + LLM     harness       │
│  trail for ENSO         response review     analyze       │
└───────────────────────────────────────────────────────────┘
```

---

## Quick Start

```bash
# Install (Linux / macOS, no Rust toolchain required)
curl -fsSL https://raw.githubusercontent.com/ARLI-Research/arli/main/install.sh | bash

# Configure
arli setup

# Chat
arli chat                     # Interactive TUI
arli chat -q "Fix this bug"   # Single query

# Gateway
arli gateway start            # 20-platform messaging daemon

# Self-update
arli update
```

---

## The Harness — What Makes ARLI Different

A harness is not just a tool-calling loop. It's the infrastructure that makes agent execution **verifiable, fault-tolerant, and auditable**. ARLI implements the full stack from the "Code as Agent Harness" survey (458 papers, 2025–2026).

### Fault-Tolerant Sandbox

Before executing a contract, ARLI snapshots the workspace. If anything goes wrong — verification fails, ENSO disputes, sandbox kills the process — the workspace rolls back to a clean state. The next retry starts from the same baseline, not from garbage.

```
Contract received → snapshot workspace → execute → ✓ commit / ✗ rollback
```

Git-based when available, tar.gz fallback for non-git workspaces.

### Hierarchical Sandbox Profiles

Not one sandbox — four. Each contract declares what level it needs:

| Profile | Network | Memory | Timeout | Use |
|---------|---------|--------|---------|-----|
| **Build** | No | 1 GB | 5 min | Compile/lint only |
| **Test** | Yes | 2 GB | 10 min | Build + test fixtures |
| **Deploy** | Full | 4 GB | 20 min | Integration, staging |
| **Unsafe** | Full | None | None | Trusted code only |

The harness enforces that the actual sandbox is **at least as restrictive** as the contract demands. A Build contract cannot run under Unsafe.

### Verification Pipeline

Attestation doesn't happen on faith. Before submitting to ENSO, ARLI runs:

```
compile → lint → test → fuzz
```

Auto-detects commands from the workspace (`Cargo.toml` → `cargo check`, `package.json` → `npm test`, etc.). If compile or test fails — attestation is **blocked**. ENSO never sees a "Verified" on broken code.

### Task Contracts

Upfront declaration of what the agent will do. Before execution, the contract specifies expected artifacts and success checks. After execution, the contract hash is included in the ENSO attestation — cryptographic proof that the agent delivered exactly what it promised.

```rust
let contract = TaskContract {
    goal: "Fix compilation error in src/main.rs",
    expected_artifacts: vec!["src/main.rs"],
    success_checks: vec!["cargo check", "cargo test"],
    sandbox_policy: Some("build"),
};
// Contract hash: sha256("goal=...&artifacts=...&checks=...")
// Included in ArliAttestation → ENSO verifies it
```

### Experiential Memory

The agent learns from failures. When `cargo build` fails with "borrow checker", ARLI records the fix ("clone before move"). Next time the same error appears, it auto-applies the fix without wasting a retry.

- **MemGovern filter**: fixes must be verified to persist. Unverified fixes older than 7 days are auto-pruned.
- **Memory health**: `arli harness analyze` shows hit rate — is the memory actually helping or just noise?

### Harness Telemetry

Per-tool, per-policy, per-task metrics. Persistent to `~/.arli/telemetry.json`.

```
$ arli harness analyze

═══ ARLI Harness Analytics ═══
─── Tools ───
  Total calls: 1247 | Failures: 89 | Rate: 7.1%
  Worst: terminal (23% failure, 156 calls)

─── Sandbox Policies ───
  policy-abc: 12 violations — may be too restrictive

─── Experiential Memory ───
  Lessons: 47 total | 31 verified | 16 unverified
  Hit rate: 68% — effective

─── Recommendations ───
  1. Fix tool 'terminal': fails 23% — check timeout config
  2. 16 unverified lessons — run verification or prune stale
```

### Failure Attribution

When a contract fails, ARLI doesn't just say "Disputed". It classifies **why**:

| Category | Example | Retry? |
|----------|---------|--------|
| VerificationFailed | `cargo test` returned exit code 1 | No — fix the code |
| SandboxKilled | Process killed by OOM, SIGKILL | Yes — increase limits |
| EnsoRejected | agent_id does not match contract | No — fix registration |
| NetworkError | ICP call timed out | Yes — retry with backoff |
| InternalError | Failed to serialize attestation | No — investigate |

45+ error patterns matched. Confidence scored. Actionable operator guidance.

### Agent Governance Toolkit

Every agent action flows through a governance checkpoint:

```
Agent wants to run "deploy" → RiskScore: 80/high
  → Exceeds approval threshold → Queued for human approval
  → Ticket gov-0001 created, expires in 5 min
  → Human approves → Action executes
  → Audit trail recorded
```

Risk taxonomy: SAFE(0) → LOW(20) → MEDIUM(50) → HIGH(80) → CRITICAL(100). Per-tool auto-classification. Policy fingerprint included in ENSO attestation — proves governance was active.

### Quality Critic

Two-layer response review before delivery:

1. **Heuristic** (fast, free): catches empty responses, hallucination markers ("as an AI, I cannot..."), repeated sentences, missing code blocks
2. **LLM Critic** (accurate, cheap model): structured JSON critique — score 1–10, issues with severity/category/suggestion

```json
{
  "score": 4,
  "acceptable": false,
  "issues": [
    {
      "severity": "error",
      "category": "factual_error",
      "description": "Claims function returns String but it returns Result<String>",
      "suggestion": "Check the actual return type in src/lib.rs line 42"
    }
  ],
  "summary": "Response contains a factual error about the return type"
}
```

### Multi-Agent Shared Memory

Swarm agents share a common blackboard. Thread-safe, versioned, TTL-aware.

```
Agent A discovers market signal → writes "market:BTC:signal=BUY"
Agent B reads "market:BTC:signal" → opens position
Agent C reads result → reports to ENSO
```

Namespaced queries: `read_by_prefix("market:")` returns all market data. `read_by_author("agent-2")` returns everything agent-2 discovered. Persisted to `~/.arli/shared_memory.json`.

---

## Commands

```
arli chat              Interactive TUI chat
arli chat -q "..."     Single query
arli setup             Interactive configuration wizard
arli model             Change model/provider interactively
arli doctor            System health check
arli update            Self-update from GitHub Releases

arli harness analyze   Analyze telemetry + lessons → insights + recommendations

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
arli enso oracle       Autonomous attestation loop (daemon mode)

arli marketplace rfq-create  Create RFQ for ENSO marketplace
arli marketplace rfq-list    List open RFQs
arli marketplace stats       Marketplace statistics

arli kanban create     Create task board
arli kanban add        Add card
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
use arli_trading::fusion_live::FusionLiveTrader;

let trader = FusionLiveTrader::new(config, metrics).await?;
trader.run().await; // WebSocket loop + signals + execution
```

- 229 perpetual + 19 spot markets on Hyperliquid mainnet
- Cointegration signal generation (10 min candles)
- OCSF event hash embedded in every order for attestation
- Prometheus metrics: PnL, positions, signal count, execution latency

---

## ENSO — On-Chain Settlement (ICP)

Full attestation loop: contract → sandbox execution → verification pipeline → ed25519 attestation → ICP settlement. Single atomic call.

```
ARLI Oracle                    ENSO Contracts (ICP)           ICP Ledger
─────                          ────────────────────           ──────────
│                              │
│ 1. Snapshot workspace        │
│ 2. Verify (compile→test)     │
│ 3. Execute agent             │
│ 4. Attribute failures        │
│ 5. Build OCSF attestation    │
│    + ed25519 signature       │
│    + task contract hash      │
│    + governance fingerprint  │
│                              │
│ 6. submit_arli_payment ────→ │
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

---

## Comparison

| | Hermes | Claude Code | ARLI |
|---|---|---|---|
| Language | Python | TypeScript | **Rust** |
| Binary size | ~200MB | ~150MB | **12MB** |
| Cold start | 2–5s | 1–3s | **~50ms** |
| LLM providers | 37 | 3 | **36** |
| Messaging platforms | 21 | — | **20** |
| **Kernel sandbox** | Partial | Partial | **Landlock + seccomp** |
| **Fault tolerance** | — | — | **Snapshot + rollback** |
| **Verification pipeline** | — | — | **compile→lint→test→fuzz** |
| **Task contracts** | — | — | **Upfront declarations + hash** |
| **Experiential memory** | Partial | — | **Learn from failures, auto-apply** |
| **Failure attribution** | — | — | **6 categories, 45+ patterns** |
| **Governance toolkit** | Partial | — | **Risk scoring + approval queue** |
| **Quality critic** | — | — | **Heuristic + LLM review** |
| **Shared agent memory** | — | — | **Swarm blackboard** |
| **Harness analytics** | — | — | **CLI: `arli harness analyze`** |
| Swarm orchestration | Partial | Partial | **Native (TaskRouter, fan-out)** |
| Cron scheduler | Native | — | **Native** |
| MCP server | Native | — | **Native** |
| Self-update | — | Native | **Native** |
| **Live trading** | — | — | **Hyperliquid, WebSocket, cointegration** |
| **On-chain settlement** | — | — | **ENSO/ICP, atomic verify+settle+release** |
| Kanban boards | — | — | **SQLite, WIP limits** |
| Web dashboard | Native | — | **axum + htmx** |
| Audit logging | — | — | **OCSF (SIEM-compatible)** |
| Tests | — | — | **448 (0 fail)** |

---

## Sandbox

Kernel-level isolation. Three enforcement layers applied before the child process starts:

1. **Seccomp BPF** — syscall whitelist. Blocks `ptrace`, `mount`, `reboot`, `kexec_load`, and 30+ other dangerous calls
2. **Landlock** — filesystem access control at kernel level. Whitelist directories, deny everything else
3. **Privilege drop** — `initgroups` → `setgid` → `setuid` before `exec`. Process runs as nobody (UID 65534)

```rust
let sandbox = Sandbox::from_policy(&policy)?;
let output = sandbox.execute_isolated("cargo build --release")?;
```

---

## Configuration

```toml
# ~/.arli/config.toml
model = "deepseek-chat"
max_iterations = 90

[provider]
name = "deepseek"
api_key = "sk-..."

[gateway]
telegram_token = "..."

[enso]
icp_gateway = "https://icp0.io"
contracts_canister_id = "..."

[trading]
hyperliquid_wallet = "..."
```

---

## Install

```bash
# One-liner
curl -fsSL https://raw.githubusercontent.com/ARLI-Research/arli/main/install.sh | bash

# From source
git clone https://github.com/ARLI-Research/arli
cd arli && cargo build --release
```

## Tests

```bash
cargo test -p arli-core       # 448 tests, 0 fail
cargo test --workspace        # all crates
```

## License

MIT
