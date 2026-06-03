# ARLI Deployment Guide

**Version:** 1.0 — June 2026  
**Target Audience:** DevOps engineers, SRE teams, system administrators  
**Expected duration:** 15–30 minutes for basic deployment, 1–2 hours for production hardening

---

## System Requirements

### Linux (Production)

| Component | Minimum | Recommended |
|---|---|---|
| **Kernel** | 5.13+ (Landlock ABI V3) | 6.1+ LTS |
| **Architecture** | x86_64 | x86_64 |
| **RAM** | 512 MB | 2 GB+ |
| **Disk** | 200 MB (binary + config) | 1 GB+ (with logs, workspaces, SQLite) |
| **libc** | glibc 2.31+ or musl 1.2+ | glibc 2.35+ |
| **Runtime deps** | None (static binary) | `unshare` from util-linux (for namespace isolation) |
| **Kernel modules** | Landlock LSM enabled | SELinux or AppArmor (additional LSM layer) |

**Verify Landlock availability:**
```bash
# Check kernel version
uname -r  # Must be >= 5.13

# Check Landlock support
cat /sys/kernel/security/lsm  # Should include "landlock"

# Or use arli itself
arli doctor
```

### macOS (Development / CI)

| Component | Minimum |
|---|---|
| **OS** | macOS 12 (Monterey) or later |
| **Architecture** | Apple Silicon (arm64) or Intel (x86_64) |
| **RAM** | 512 MB |
| **Disk** | 200 MB |

Note: macOS uses Seatbelt/sandbox-exec for isolation, which has fewer capabilities than Linux Landlock+seccomp. Production deployments handling untrusted code should use Linux.

---

## Installation

### Method 1: One-Liner Install (Recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/ARLI-Research/arli/main/install.sh | bash
```

The install script:
1. Detects OS and architecture
2. Downloads the latest binary from GitHub Releases
3. Installs to `/usr/local/bin/arli` (or `~/.local/bin/` for non-root)
4. Runs `arli setup` for interactive configuration

### Method 2: From Source

```bash
git clone https://github.com/ARLI-Research/arli
cd arli
cargo build --release
./target/release/arli setup
```

Requires Rust toolchain (rustc 1.78+). Build time: ~3–5 minutes on modern hardware.

### Method 3: Binary Releases

Download pre-built binaries from [GitHub Releases](https://github.com/ARLI-Research/arli/releases):

```bash
# Example for Linux x86_64
wget https://github.com/ARLI-Research/arli/releases/latest/download/arli-linux-x86_64.tar.gz
tar xzf arli-linux-x86_64.tar.gz
sudo mv arli /usr/local/bin/
chmod +x /usr/local/bin/arli
arli setup
```

### Post-Installation Verification

```bash
arli doctor       # System health check
arli --version    # Should print version >= 0.5.0
arli --help       # Full command listing
```

---

## Configuration

### config.toml Walkthrough

ARLI's configuration lives at `~/.arli/config.toml`. Run `arli setup` for interactive configuration, or edit manually:

```toml
# --- Core Settings ---
model = "deepseek-chat"           # Default model
max_iterations = 90               # Max agent steps per turn (1–200)
tool_progress = "all"             # off / new / all / verbose
compression_threshold = 0.5       # Context compaction trigger (0.5–0.95)

# --- Provider Configuration ---
[provider]
name = "deepseek"                 # Provider name from registry
api_key = "sk-..."               # API key

# Alternative: use environment variable
# api_key = "${DEEPSEEK_API_KEY}"

# --- Session Management ---
[session_reset]
mode = "inactivity_daily"         # inactivity_daily / inactivity / daily / never
inactivity_minutes = 1440         # 24 hours
daily_reset_hour = 4              # Reset at 4 AM local time

# --- Tool Backends ---
[search]
provider = "duckduckgo"           # duckduckgo / brave / searxng / tavily / firecrawl / exa

[memory]
provider = "builtin"              # builtin / mem0 / chromadb / qdrant / ...

[terminal]
backend = "local"                 # local / docker / ssh / modal / daytona

[browser]
provider = "local"                # local / camofox / browserbase / firecrawl / browser_use

# --- Gateway (for multi-platform messaging) ---
[gateway]
# At minimum, set one platform token:
telegram_token = "..."
# discord_token = "..."
# slack_token = "..."
# ...

[gateway.metrics]
enabled = true
port = 9090

# --- ENSO / ICP ---
[enso]
icp_gateway = "https://icp0.io"
registry_canister_id = "ENSO_REGISTRY_CANISTER_ID"
contracts_canister_id = "7yv6j-ryaaa-aaaaa-qhheq-cai"
arli_public_key = "..."          # From `arli key show`

# --- Live Trading ---
[trading]
hyperliquid_wallet = "..."        # Wallet address
hyperliquid_rpc = "https://api.hyperliquid.xyz"
max_position_usd = 10000          # Safety limit
daily_trade_limit_usd = 50000     # Safety limit

# --- Sandbox ---
[sandbox]
landlock_compatibility = "best_effort"  # best_effort / hard_requirement
default_policy = "restrictive"          # restrictive / permissive / custom
```

### Environment Variables

All `config.toml` values can be overridden via environment variables with the `ARLI_` prefix using double-underscore nesting:

```bash
export ARLI_PROVIDER__API_KEY="sk-..."
export ARLI_GATEWAY__TELEGRAM_TOKEN="..."
export ARLI_TRADING__HYPERLIQUID_WALLET="..."
export ARLI_SANDBOX__LANDLOCK_COMPATIBILITY="hard_requirement"
```

Environment variables take precedence over config file values. Secrets should always be set via environment variables, never written to config files.

### Provider Setup

ARLI supports 36 LLM providers. Configure at least one:

```bash
# Interactive
arli setup              # Walks through provider selection

# Manual — set env var and specify provider
export DEEPSEEK_API_KEY="sk-..."
arli config set provider.name deepseek

# Or edit config.toml directly
```

For multi-provider failover, configure multiple providers and set routing strategy:

```toml
[inference]
routing = "fallback"            # round_robin / fallback / affinity
providers = ["deepseek", "openai", "anthropic"]
```

---

## Gateway Deployment

The ARLI gateway is a daemon that connects to 20 messaging platforms and routes messages to agent sessions. It should run as a persistent systemd service.

### systemd Unit

Create `/etc/systemd/system/arli-gateway.service`:

```ini
[Unit]
Description=ARLI Gateway Daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=arli
Group=arli
EnvironmentFile=/etc/arli/gateway.env
ExecStart=/usr/local/bin/arli gateway start
Restart=always
RestartSec=10
LimitNOFILE=65536

# Security hardening
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/var/lib/arli /var/log/arli
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictRealtime=yes
MemoryMax=2G

[Install]
WantedBy=multi-user.target
```

Create the environment file `/etc/arli/gateway.env`:

```bash
# Provider keys
DEEPSEEK_API_KEY="sk-..."
OPENAI_API_KEY="sk-..."

# Platform tokens
ARLI_GATEWAY__TELEGRAM_TOKEN="..."
# ARLI_GATEWAY__DISCORD_TOKEN="..."
# ... other platforms as needed

# ENSO keys (if using on-chain settlement)
ARLI_ENSO__ARLI_PUBLIC_KEY="..."

# Trading (if applicable)
ARLI_TRADING__HYPERLIQUID_WALLET="..."
```

```bash
# Create arli user and directories
sudo useradd -r -s /bin/false -d /var/lib/arli arli
sudo mkdir -p /var/lib/arli /var/log/arli
sudo chown -R arli:arli /var/lib/arli /var/log/arli

# Enable and start
sudo systemctl daemon-reload
sudo systemctl enable --now arli-gateway
sudo systemctl status arli-gateway
```

### Health Checks

```bash
# Health check endpoint (requires `arli serve -p PORT` running separately)
curl http://localhost:9090/health

# systemd health
systemctl is-active arli-gateway

# Logs
journalctl -u arli-gateway -f
```

### Prometheus Metrics

Enable metrics in `config.toml`:

```toml
[gateway.metrics]
enabled = true
port = 9090
```

Key metrics:

| Metric | Type | Description |
|---|---|---|
| `arli_agent_sessions_active` | Gauge | Current active agent sessions |
| `arli_agent_turns_total` | Counter | Total agent turns processed |
| `arli_guardrail_blocks_total` | Counter | Safety blocks (by mode, classification) |
| `arli_sandbox_isolated_total` | Counter | Isolated vs. non-isolated executions |
| `arli_attestation_sign_total` | Counter | Signed attestations (by result) |
| `arli_provider_requests_total` | Counter | LLM provider requests (by provider, status) |
| `arli_trading_pnl` | Gauge | Current unrealized PnL |
| `arli_trading_positions` | Gauge | Open position count |
| `arli_gateway_messages_total` | Counter | Messages processed by platform |

Scrape with Prometheus:

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'arli-gateway'
    static_configs:
      - targets: ['localhost:9090']
```

---

## Multi-Tenant Setup

### Inference Brokering

ARLI supports multi-tenant inference brokering with tenant API keys, rate limiting, and usage billing.

```toml
[tenants]
enabled = true

[tenants.defaults]
rate_limit_rpm = 100            # Requests per minute per tenant
rate_limit_rpd = 10000          # Requests per day per tenant
max_concurrent_sessions = 5     # Max active agent sessions
default_model = "deepseek-chat"

[[tenants.accounts]]
id = "tenant-corp-a"
name = "Corp A"
api_key_hash = "sha256:..."
rate_limit_rpm = 500
allowed_models = ["deepseek-chat", "openai-gpt-4o"]
sandbox_policy = "restrictive"

[[tenants.accounts]]
id = "tenant-corp-b"
name = "Corp B"
api_key_hash = "sha256:..."
rate_limit_rpm = 200
allowed_models = ["deepseek-chat"]
sandbox_policy = "network-agent"
```

**Tenant isolation:**
- Each tenant receives a separate SQLite database (or schema-level isolation in shared DB)
- Per-tenant sandbox policies enforce filesystem boundaries
- Rate limits are enforced per-tenant, not globally
- Usage metrics are tagged with `tenant_id` for billing

### API Key Authentication

Tenants authenticate via `Authorization: Bearer arli-t-<tenant_key>` headers. The gateway validates the key hash (SHA-256) against the configured tenant list and injects the tenant context into the agent session.

---

## Sandbox Configuration

### Policy YAML

Define per-agent sandbox policies in `~/.arli/policies/`:

```yaml
# ~/.arli/policies/network-agent.yaml
filesystem:
  read_only:
    - /usr
    - /lib
    - /lib64
    - /bin
    - /etc
  read_write:
    - /tmp
    - /workspace
  include_workdir: true

process:
  run_as_user: nobody
  run_as_group: nogroup
  allow_core_dumps: false

network:
  mode: proxy
  allowed_endpoints:
    - host: api.github.com
      port: 443
      tls: true
    - host: "*.example.com"
      tls: true

landlock:
  compatibility: hard_requirement
```

### Host Glob Matching

Network endpoint hosts support glob patterns:

| Pattern | Matches |
|---|---|
| `api.example.com` | Exact match only |
| `*.example.com` | `api.example.com`, `www.example.com` (single subdomain level) |
| `**.example.com` | Any depth: `a.b.example.com`, `x.y.z.example.com` |
| `10.0.*` | Any host in `10.0.0.0/16` |
| `*` | Everything (use with caution) |

### Per-Agent Profiles

Assign policies to specific agents:

```toml
[[agents]]
name = "code-reviewer"
sandbox_policy = "restrictive"
max_iterations = 30

[[agents]]
name = "web-researcher"
sandbox_policy = "network-agent"
max_iterations = 60
```

---

## Trading Setup

### Hyperliquid Wallet

```bash
# Set wallet address and optional private key
export ARLI_TRADING__HYPERLIQUID_WALLET="0x..."
# Private key should be in a separate, restricted environment variable
export HYPERLIQUID_PRIVATE_KEY="..."

# Or in config.toml
# [trading]
# hyperliquid_wallet = "0x..."
```

### Safety Limits

Always configure safety limits before enabling live trading:

```toml
[trading.safety]
max_position_size_usd = 10000      # Max single position
max_total_exposure_usd = 50000     # Max total exposure across all positions
daily_trade_limit_usd = 100000     # Max daily volume
max_slippage_percent = 1.0         # Max acceptable slippage
require_guardrail = true           # Require guardrail approval for all trades
```

These limits are enforced at three levels:
1. **Guardrail (AgentDoG 1.5)** — blocks trades exceeding limits before execution
2. **Trading engine** — client-side pre-trade validation
3. **Exchange level** — Hyperliquid account-level limits (configured separately)

### Metrics

Trading-specific Prometheus metrics:
- `arli_trading_pnl{market}` — unrealized PnL per market
- `arli_trading_positions{market, side}` — open positions
- `arli_trading_executions_total{market, side}` — trade count
- `arli_trading_execution_latency_ms` — execution latency histogram
- `arli_trading_signal_count` — cointegration signal count

---

## ENSO/ICP Setup

### Key Generation

```bash
# Generate ed25519 keypair for attestation signing
arli key generate

# Display public key (share this for canister registration)
arli key show
```

Keys are stored at `~/.arli/keys/` with `0o600` permissions.

### Canister Registration

```bash
# One-shot setup: keygen + config + canister registration
arli enso setup --contracts 7yv6j-ryaaa-aaaaa-qhheq-cai

# Verify registration
arli enso status
```

This registers the agent's public key, binary hash, name, and capabilities with the ENSO Contracts canister on ICP mainnet.

### Oracle Mode

Run as a daemon that polls ENSO contracts and auto-attests:

```bash
# Start oracle for specific contracts
ENSO_CONTRACTS=contract_xxx,contract_yyy arli enso oracle

# As a systemd service
[Service]
ExecStart=/usr/local/bin/arli enso oracle
Environment=ENSO_CONTRACTS=contract_xxx,contract_yyy
```

The oracle:
1. Polls the ENSO Contracts canister for pending attestation requests
2. Executes the job in the sandbox
3. Builds and signs an OCSF attestation
4. Submits `submit_arli_payment(contract_id, attestation_json)` to the canister
5. Receives settlement confirmation

---

## Monitoring

### Prometheus Metrics Endpoint

```
GET :9090/metrics
```

Returns Prometheus-formatted metrics. See Gateway Deployment section for the full metric list.

### Health Endpoints

```bash
# Start health check HTTP server
arli serve -p 9090

# Endpoints:
# GET :9090/health   → {"status": "ok", "version": "0.5.0"}
# GET :9090/metrics  → Prometheus metrics
```

### Log Aggregation

ARLI writes structured logs (if `tracing` subscriber is configured) to stdout and optionally to a file. For production, ship logs to your existing aggregation stack:

```bash
# systemd journal → Loki (via promtail)
# stdout → Docker log driver → ELK/Datadog
# file → Filebeat → Elasticsearch
```

**OCSF audit events** are separate from operational logs. They are written to `~/.arli/audit/` in JSON Lines format, one event per line, and can be shipped directly to SIEM via file monitoring.

### Dashboard

```bash
# Launch web UI dashboard
arli dashboard -p 8080
```

The dashboard (axum + htmx) provides real-time metrics, recent sessions, guardrail blocks, and sandbox execution stats.

---

## Troubleshooting

### Common Issues

**"Landlock not available" (arli doctor)**
- Kernel version < 5.13. Upgrade kernel or set `landlock_compatibility = "best_effort"`.
- Landlock LSM not enabled. Check `cat /sys/kernel/security/lsm`. Add `landlock` to kernel cmdline if using custom kernel.

**"Seccomp filter failed to apply"**
- Running in a container without `CAP_SYS_ADMIN` or `seccomp=unconfined`. Grant the capability or use a seccomp profile that allows loading filters.
- Older Docker versions may block seccomp filter nesting. Set `--security-opt seccomp=unconfined` on the container (only in trusted environments).

**"Privilege drop failed: User 'nobody' not found"**
- Create the user: `useradd -r nobody` (or verify it exists in `/etc/passwd`).

**"Gateway fails to start: Telegram token not configured"**
- Set at least one platform token in `config.toml` or environment variable. The gateway requires at minimum one platform to start.
- Check token validity: an invalid token produces a different error. Test with Telegram Bot API directly.

**"ENSO attestation verification failed"**
- Public key mismatch: ensure the key registered with the canister matches `arli key show`.
- Binary hash mismatch: the canister has a different binary hash. Re-register after updating the binary.
- Replay detected: job_id or timestamp already seen by the canister.

**"Trading: position exceeds configured limits"**
- Increase limits in `config.toml` (`[trading.safety]` section) or reduce position size in the trading strategy.

**"Provider returns 429 (rate limited)"**
- The credential pool automatically rotates keys, but if all keys are rate-limited, increase `rate_limit_rpm` or add more API keys to the pool.
- For multi-tenant setups, check if one tenant is consuming disproportionate quota.

**"Sandbox execution killed (timeout)"**
- Increase `timeout_secs` in the sandbox config if legitimate workloads need more time.
- Check for infinite loops or hung processes in the agent's tool calls.

**"OCSF audit log growing unbounded"**
- Configure log rotation: ARLI supports `max_audit_log_size_mb` and `max_audit_log_age_days` in config.
- Set up log shipping to SIEM and purge local files after successful export.

### Log Locations

| Component | Location |
|---|---|
| Operational logs | `journalctl -u arli-gateway` (systemd) or stdout |
| OCSF audit events | `~/.arli/audit/` |
| Agent session logs | `~/.arli/sessions/` (SQLite with FTS5) |
| Prometheus metrics | `:9090/metrics` |
| Sandbox execution logs | Captured in agent tool call output |

### Recovery Procedures

**Gateway crash loop:**
```bash
systemctl stop arli-gateway
# Check logs
journalctl -u arli-gateway -n 100
# Validate config
arli doctor
# Fix the issue, then restart
systemctl start arli-gateway
```

**Corrupt SQLite database:**
```bash
cp ~/.arli/arli.db ~/.arli/arli.db.backup
sqlite3 ~/.arli/arli.db "PRAGMA integrity_check;"
# If corrupt, restore from backup or rebuild
arli setup  # Rebuilds fresh configuration
```

**Lost ed25519 key:**
- **Cannot be recovered.** Generate a new keypair and re-register with ENSO.
- Previous attestations signed with the old key remain verifiable (public key is embedded).

---

## Docker Deployment

For containerized environments, ARLI can run in Docker with appropriate security configurations.

### Dockerfile

```dockerfile
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    util-linux iptables ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY arli /usr/local/bin/arli
RUN chmod +x /usr/local/bin/arli

# Create arli user for privilege drop
RUN useradd -r -s /bin/false -d /var/lib/arli arli && \
    mkdir -p /var/lib/arli /var/log/arli && \
    chown -R arli:arli /var/lib/arli /var/log/arli

USER arli
WORKDIR /var/lib/arli

EXPOSE 9090
ENTRYPOINT ["/usr/local/bin/arli"]
```

### Docker Run (with Sandbox Support)

```bash
docker run -d \
  --name arli-gateway \
  --security-opt seccomp=unconfined \
  --cap-add=SYS_ADMIN \
  --cap-add=NET_ADMIN \
  -v /path/to/workspace:/var/lib/arli/workspace \
  -e DEEPSEEK_API_KEY="sk-..." \
  -e ARLI_GATEWAY__TELEGRAM_TOKEN="..." \
  -p 9090:9090 \
  arli:latest gateway start
```

**Security notes for containers:**
- `seccomp=unconfined` is required for ARLI's own seccomp filter nesting. Only use in trusted environments.
- `CAP_SYS_ADMIN` is needed for namespace creation (`unshare`). On kernels with `user_namespaces` enabled, this may not be necessary.
- Consider using `--read-only` root filesystem with tmpfs mounts for `/tmp` and `/var/lib/arli`.
- For production, use a dedicated non-root user inside the container (ARLI's privdrop handles this at the process level regardless).

### Docker Compose

```yaml
version: '3.8'
services:
  arli-gateway:
    image: arli:latest
    command: gateway start
    security_opt:
      - seccomp:unconfined
    cap_add:
      - SYS_ADMIN
      - NET_ADMIN
    environment:
      - DEEPSEEK_API_KEY=${DEEPSEEK_API_KEY}
      - ARLI_GATEWAY__TELEGRAM_TOKEN=${TELEGRAM_TOKEN}
    ports:
      - "9090:9090"
    volumes:
      - arli-data:/var/lib/arli
      - ./workspace:/var/lib/arli/workspace
    restart: unless-stopped

volumes:
  arli-data:
```

---

## Upgrade and Migration

### Self-Update

```bash
# Update to the latest release
arli update

# Verify after update
arli --version
arli doctor
```

The `arli update` command:
1. Checks GitHub Releases for the latest version
2. Downloads the binary
3. Verifies the SHA-256 hash
4. Replaces the current binary
5. Preserves all configuration and data

### Manual Upgrade

```bash
# Stop the service
systemctl stop arli-gateway

# Download new binary
wget https://github.com/ARLI-Research/arli/releases/latest/download/arli-linux-x86_64.tar.gz
tar xzf arli-linux-x86_64.tar.gz
sudo mv arli /usr/local/bin/arli

# Re-register with ENSO if binary hash changed
arli key show
arli enso setup --contracts 7yv6j-ryaaa-aaaaa-qhheq-cai

# Start the service
systemctl start arli-gateway
```

### Version Compatibility

- **Config files**: Forward-compatible within the same major version (0.x). New settings are added with safe defaults.
- **SQLite databases**: Schema is migrated automatically on startup. Downgrading is not supported — back up before upgrading.
- **ENSO attestations**: Binary hash changes on upgrade. Re-register with the ENSO canister. Old attestations remain verifiable (public key is embedded, binary hash is in the attestation body).
- **Provider APIs**: ARLI tracks provider API changes and ships compatibility updates. Check release notes for breaking changes.

---

## Production Checklist

Before going to production, verify each item:

- [ ] Kernel 5.13+ with Landlock LSM enabled (`arli doctor` passes)
- [ ] Gateway running as non-root systemd service with `NoNewPrivileges=yes`
- [ ] All secrets in environment variables, not config files
- [ ] ed25519 key generated and registered with ENSO canister (if applicable)
- [ ] `landlock_compatibility = "hard_requirement"` for trading/financial workloads
- [ ] Per-agent sandbox policies defined and tested
- [ ] Guardrail mode configured (`policy_based` minimum; `hybrid` recommended for trading)
- [ ] Safety limits configured for trading (`max_position_size_usd`, `daily_trade_limit_usd`)
- [ ] Prometheus metrics enabled and scraped
- [ ] OCSF audit log shipped to SIEM
- [ ] Health check endpoint monitored
- [ ] Alerting configured for: guardrail blocks, seccomp violations, attestation failures, gateway downtime
- [ ] Database backups configured for `~/.arli/arli.db`
- [ ] Key rotation schedule defined (90 days for provider keys)
- [ ] Incident response runbook available
- [ ] `arli doctor` returns clean on every host
- [ ] Load tested at expected concurrency (gateway, agent sessions, sandbox executions)
