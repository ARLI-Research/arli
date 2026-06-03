# ARLI Security Whitepaper

**Version:** 1.0 — June 2026  
**Target Audience:** Security architects, compliance officers, enterprise DevOps teams  
**Classification:** Public

---

## Executive Summary

ARLI is a Rust-native AI agent harness built for production environments where untrusted code execution is a daily requirement. It provides a defense-in-depth security architecture spanning kernel-level sandboxing, cryptographic attestation, safety guardrails, and SIEM-compatible audit logging — all in a single 12 MB binary with zero runtime dependencies.

This whitepaper documents ARLI's complete security model: the threat landscape it operates in, the layered controls that mitigate those threats, and the operational practices recommended for secure deployment.

**Key security properties:**

- **Kernel-enforced isolation.** Every agent command runs through three mandatory isolation layers: seccomp BPF (syscall filter), Landlock (filesystem access control), and privilege drop (UID/GID reduction). The attack surface is closed before the child process starts, via `Command::pre_exec()`.
- **Cryptographic non-repudiation.** Every agent execution produces an ed25519-signed OCSF attestation covering run identity, job identity, timestamp, sandbox configuration hash, and binary hash. Replay is cryptographically impossible.
- **SIEM-native audit trail.** All agent activity is logged in OCSF format (`class_uid: 6007`), directly ingestible by Splunk, ELK, Datadog, and other SIEM platforms.
- **Safety guardrail.** AgentDoG 1.5 enforces a Pre-Reply checkpoint with a 3D risk taxonomy (Risk Source × Failure Mode × Real-World Harm) and operates in three modes: policy-based, LLM judge, or hybrid.

---

## Threat Model

### Who Are the Attackers?

ARLI's threat model identifies five adversary profiles:

| Adversary | Capability | Motivation |
|---|---|---|
| **Malicious prompt author** | Social engineering, prompt injection, jailbreak attempts | Extract secrets, trigger unauthorized actions |
| **Compromised upstream model** | Model poisoning, backdoor weights, hidden instruction following | Covert data exfiltration, supply chain compromise |
| **Rogue agent behavior** | Agent autonomously exceeds its mandate, makes destructive tool calls | Unintended (due to hallucination, misalignment) or adversarial |
| **Network adversary** | Man-in-the-middle on API calls, traffic interception | Steal API keys, modify model responses, inject commands |
| **Insider threat** | Legitimate access to the ARLI host, config files, or key material | Elevate privileges, exfiltrate data, tamper with audit logs |

### What Are the Assets?

| Asset | Classification | Protection Requirement |
|---|---|---|
| Agent signing keys (ed25519) | Critical | File permissions 0o600, never logged, never in memory as plaintext after use |
| Provider API keys | High | Environment variables, credential pools with OpenShell pattern, gateway-managed rotation |
| Sandboxed workspaces | Medium | Landlock-enforced read/write boundaries, per-agent filesystem profiles |
| OCSF audit log | High | Append-only storage, cryptographic chain of custody, off-site replication |
| User PII in conversations | High | Data minimization, configurable retention, right-to-deletion support |
| Hyperliquid trading keys | Critical | Encrypted at rest, key sharding recommended, daily spend limits enforced at guardrail level |
| ENSO attestation payloads | High | ed25519-signed, hash-chained, ICP canister-verified |

### Trust Boundaries

```
┌──────────────────────────────────────────────────────────┐
│                    UNTRUSTED ZONE                         │
│  User messages · Model responses · Web content            │
│  External API responses · File uploads · Prompt injection │
└────────────┬─────────────────────────────┬───────────────┘
             │                             │
    ┌────────▼──────────┐         ┌───────▼──────────────┐
    │  Guardrail        │         │  Sandbox              │
    │  Pre-Reply check  │         │  Landlock · seccomp   │
    │  3D risk taxonomy │         │  privdrop · namespaces│
    └────────┬──────────┘         └───────┬──────────────┘
             │                             │
             └──────────┬──────────────────┘
                        │
              ┌─────────▼──────────────┐
              │     TRUSTED ZONE       │
              │  Agent actor model     │
              │  OCSF audit trail      │
              │  ed25519 attestation   │
              │  ENSO settlement loop  │
              └────────────────────────┘
```

---

## Sandbox Architecture

ARLI's sandbox is a three-layer defense applied in strict order. The enforcement chain is the same pattern used by OpenShell and aligns with the principle that sandboxing must happen **before** the child process executes any user code.

### Enforcement Order

```
fork()
  │
  ├─ 1. SECCOMP BPF ─── syscall whitelist via BPF program
  │   (applied first: can only be tightened, never loosened)
  │
  ├─ 2. LANDLOCK ────── filesystem access control via Linux LSM
  │   (path_beneath rules: read-only and read-write sets)
  │
  ├─ 3. PRIVILEGE DROP ─ initgroups → setgid → setuid → verify
  │   (process runs as nobody:nogroup, UID 65534)
  │
  └─ exec()
```

All three layers execute inside `Command::pre_exec()`, which runs in the forked child before `exec()`. If any layer fails, the child exits with an error — no code ever executes without protection.

### Layer 1: Seccomp BPF

The seccomp filter is a BPF program compiled via the `seccompiler` crate. It takes a **default-allow** posture: all syscalls are permitted except those explicitly blocked. This avoids the fragility of a whitelist while eliminating the most dangerous kernel attack surfaces.

**Blocked syscalls (20 total):**

- **Socket creation:** `socket` (41) — prevents all network socket creation
- **Memory manipulation:** `memfd_create` (319) — blocks anonymous file creation
- **Debugging/inspection:** `ptrace` (101), `process_vm_readv` (310), `process_vm_writev` (311)
- **BPF:** `bpf` (321) — prevents loading new seccomp filters
- **io_uring:** `io_uring_setup` (425), `io_uring_enter` (426), `io_uring_register` (427)
- **Mount API:** `mount` (40), `fsmount` (432), `fsopen` (433), `fspick` (434), `move_mount` (435), `open_tree` (436)
- **Namespace manipulation:** `unshare` (272), `setns` (375)
- **Performance monitoring:** `perf_event_open` (298)
- **userfaultfd:** `userfaultfd` (323)

Blocked syscalls return `EPERM` (errno 1). The agent receives a clear failure signal rather than a silent denial. The filter is deterministic — the same ruleset compiles to the identical BPF bytecode every time, making it auditable and reproducible.

### Layer 2: Landlock Filesystem Isolation

Landlock is a Linux Security Module (LSM) merged in kernel 5.13 that allows unprivileged processes to restrict their own filesystem access. ARLI uses the `landlock` crate (v0.4+) with **ABI V3** semantics, automatically falling back to older ABIs on older kernels.

**How it works:**

1. A `Ruleset` is created with the desired access control handles.
2. For each read-only path, `path_beneath_rules()` generates `PathBeneath` entries with `AccessFs::from_read()`.
3. For each read-write path, entries use `AccessFs::from_read() | AccessFs::from_write()`.
4. `restrict_self()` is called — after this, the process physically cannot access any path not in the ruleset.

**Two compatibility modes:**

- **BestEffort** (default): If Landlock is unavailable or partially enforced, execution continues. Suitable for environments where Landlock is desirable but not mandatory.
- **HardRequirement**: Execution aborts if Landlock is not fully enforced. Required for high-security workloads (trading, ENSO settlement).

**Default restrictive policy:**

- Read-only: `/usr`, `/lib`, `/lib64`, `/bin`, `/etc`
- Read-write: `/tmp`, `/dev/null`, `/dev/urandom`, plus the working directory
- Network: fully blocked
- User: `nobody:nogroup`

Per-agent policies are defined in YAML and can override these defaults. Policies are content-hashed (SHA-256), and the hash is embedded in every attestation to prove which policy was enforced.

### Layer 3: Privilege Drop

The `privdrop` module follows OpenShell's verified privilege-dropping sequence:

1. **initgroups** — Clear supplementary group list and set the target group
2. **setgid** — Set real, effective, and saved GID
3. **setuid** — Set real, effective, and saved UID
4. **Verify** — Confirm current UID/GID match the target (defense against CWE-250: Execution with Unnecessary Privileges)

If the process is already non-root, the drop is a no-op. Process hardening is also applied: core dumps are disabled via `setrlimit(RLIMIT_CORE, 0)` and `prctl(PR_SET_DUMPABLE, 0)`.

### macOS Sandbox

On macOS, ARLI supports Apple's native sandbox via **Seatbelt** (also known as `sandbox-exec`). The sandbox policy is defined in SBPL (Sandbox Profile Language) and restricts:

- Filesystem access (`file-read*`, `file-write*`)
- Network access (`network-outbound`, `network-inbound`)
- Process execution (`process-exec`, `process-fork`)
- Mach IPC and IOKit access

**Limitations vs. Linux:**

| Feature | Linux | macOS |
|---|---|---|
| Syscall filtering | seccomp BPF (fine-grained) | Not available |
| Filesystem isolation | Landlock (LSM, path-based) | Seatbelt (SBPL, operation-based) |
| Privilege drop | initgroups → setgid → setuid | Not needed (no root typically) |
| Network isolation | Network namespace + iptables | SBPL network rules |
| Namespace isolation | Mount/PID/UTS/IPC/Net | Not available |
| Container alternative | unshare + cgroups | Seatbelt + chroot |

macOS sandboxing is adequate for development and CI/CD environments. Production deployments handling untrusted code should prefer Linux with kernel 5.13+.

---

## Attestation Protocol

ARLI implements an ed25519-based cryptographic attestation protocol that proves agent execution integrity. Each attestation is an on-chain verifiable claim: *Agent X executed job Y in sandbox Z at time T, with Landlock+seccomp enforced, under binary hash B.*

### Data Structure

```rust
struct ArliAttestation {
    run_id: String,              // Unique run identifier (ARLI-internal)
    agent_id: String,            // ENSO agent ID (from registry)
    job_id: String,              // ENSO contract job ID
    timestamp_ns: u64,           // Nanosecond timestamp
    ocsf_event_hash: String,     // SHA-256 of full OCSF audit event
    ocsf_event_uri: Option<String>, // Optional URI to full event (IPFS/Arweave)
    sandbox_config_hash: String, // SHA-256 of YAML sandbox policy
    arli_binary_hash: String,    // SHA-256 of ARLI binary
    landlock_enforced: bool,     // Landlock was active
    seccomp_enforced: bool,      // Seccomp was active
    uid: u32,                    // UID process ran under (65534 = nobody)
    signature: String,           // ed25519 signature (hex)
    public_key: String,          // ed25519 public key (hex)
}
```

### Replay Protection

The signature covers a SHA-256 hash of the concatenated attestation fields:

```
SHA-256(run_id || job_id || timestamp_ns || ocsf_event_hash
     || sandbox_config_hash || arli_binary_hash
     || landlock_enforced || seccomp_enforced || uid)
```

This prevents replay in three dimensions:

1. **Cross-job replay:** Changing `job_id` invalidates the signature.
2. **Cross-time replay:** Changing `timestamp_ns` invalidates the signature.
3. **Cross-agent replay:** Changing `agent_id` or `public_key` pair invalidates the signature.

The attestation is verified by the ENSO Contracts canister on ICP mainnet. The canister calls `verify()` which checks the ed25519 signature against the embedded public key and recomputes the message hash. A valid attestation triggers escrow release and USDC/ICP payment settlement — all in a single atomic ICP call.

### Key Management

Keys are generated with `arli key generate`, which produces an ed25519 keypair using `OsRng`. The private key is stored in PEM-like format with `0o600` permissions. The public key is registered with the ENSO Contracts canister via `register_arli_agent(pubkey, binary_hash, name, capabilities)`.

Keys are **never logged, never serialized to JSON in plaintext, and never included in error messages**. The `sign_attestation()` method accepts a mutable attestation reference, signs it in place, and returns immediately.

---

## Credential Management

ARLI follows the **OpenShell credential pattern**: the agent runtime never sees raw credentials. All provider API keys, platform tokens, and trading keys are managed by the gateway layer.

### Gateway-Managed Keys

- Keys are stored in environment variables or credential pools, never in agent-accessible filesystem paths.
- Agent sessions receive only opaque session tokens, not the underlying credentials.
- The gateway enforces rate limits, usage quotas, and provider routing — the agent cannot bypass these through direct API calls.

### Credential Pools

For multi-tenant deployments, ARLI supports credential pools:

```yaml
credential_pools:
  - name: "tenant-a-openai"
    provider: "openai"
    keys:
      - key_ref: "env:OPENAI_KEY_1"
        weight: 50
      - key_ref: "env:OPENAI_KEY_2"
        weight: 50
    rate_limit_rpm: 1000
```

Keys are selected via weighted round-robin with automatic failover. A key that returns `429` is temporarily removed from the pool and retried after a backoff window.

### Key Rotation

Recommended rotation practices:
- **Provider API keys:** Rotate every 90 days
- **ENSO signing keys:** Generate once per agent identity. Rotate only if compromised — rotation requires re-registration with the canister.
- **Hyperliquid wallet keys:** Rotate when daily volume thresholds are exceeded. Use separate keys for spot and perpetual trading.

---

## Safety Guardrail — AgentDoG 1.5

ARLI's safety guardrail implements AgentDoG 1.5, a Pre-Reply safety checkpoint that intercepts every agent response before it reaches the user.

### 3D Risk Taxonomy

The guardrail classifies risks along three orthogonal dimensions:

**Dimension 1 — Risk Source** (where the risk originates):
- `UserInput` — Malicious or dangerous user prompt
- `DirectPromptInjection` — Injected instructions in retrieved content
- `EnvironmentObservation` — Agent observes dangerous system state
- `PersistentMemoryContamination` — Poisoned memory/context
- `InherentAgentFailure` — Agent hallucination or misalignment

**Dimension 2 — Failure Mode** (how the agent failed):
- `OverPrivilegedAction` — Action exceeds granted permissions
- `ActionScopeOverreach` — Action within permissions but outside intended scope
- `MissingValidation` — No validation of inputs/outputs
- `UnsafeShellExecution` — Dangerous shell command detected
- `ApprovalBypass` — Policy approval mechanism circumvented
- `InformationDisclosure` — Secrets or PII in response
- `HarmfulContent` — Malicious or dangerous content generated
- `DestructiveWorkspaceMutation` — Irreversible damage to workspace
- `FlawedReasoning` — Logical error with safety implications
- `ProceduralDeviation` — Correct action, wrong sequence/timing

**Dimension 3 — Real-World Harm** (what impact):
- `FinancialHarm` — Monetary loss
- `SecurityHarm` — System compromise
- `PrivacyHarm` — Data exposure
- `FunctionalHarm` — Service disruption
- `ComplianceHarm` — Regulatory violation
- `ReputationalHarm` — Brand damage
- `PsychologicalHarm` — User distress
- `NoImpact` — No real-world consequence

### Three Evaluation Modes

**PolicyBased** (default, zero cost): Scans tool history and response text for known risk patterns. Fast, deterministic, always available. Handles ~80% of common safety cases:
- Trading: blocks position size exceedance, daily trade limit breaches
- Coding: blocks `rm -rf /`, force push to protected branches
- Gateway: blocks credential leakage (API_KEY, private key, mnemonic patterns)
- General: blocks harmful keywords (exploit, malware, backdoor)

**LlmJudge** (accurate, costs tokens): Sends the full trajectory to a secondary LLM for safety evaluation. The judge returns a JSON verdict with confidence score. Confidence below the threshold (default 0.7) results in Safe (fail-open).

**Hybrid** (balanced): Policy first, then LLM escalation for complex trajectories (5+ tool calls). Combines the speed of policy-based with the accuracy of LLM judgment.

### Pre-Reply Checkpoint

The guardrail fires **after** the agent has produced its final response but **before** it is delivered to the user. If unsafe, the original response is replaced with a warning message explaining the block reason, risk classification, and confidence level. The user can rephrase their request to address the concern.

---

## Stream Rules — TTSR

ARLI implements **Time-Traveling Stream Rules (TTSR)** — regex-based content policies that sit dormant until violated, avoiding the context tax of injecting rules into every turn.

### How TTSR Works

1. Rules are defined as regex patterns with associated injection prompts.
2. Rules are **not** included in the system prompt (zero context cost).
3. When the agent's output matches a rule pattern:
   - The matching text is extracted.
   - The injection prompt is inserted into the conversation.
   - The agent's response is re-generated.
4. Maximum 3 retries per turn. After 3 failures, the last response is delivered with a warning.

### Example Rule

```yaml
stream_rules:
  - name: "no-credentials-in-output"
    pattern: "(?i)(api[_-]?key|token|secret|password|private[_-]?key)\\s*[:=]\\s*\\S+"
    injection: "Your previous response contained what appears to be credentials. Remove any secrets and regenerate your response."
    max_retries: 3
```

This prevents the model from ever outputting sensitive patterns without paying the context cost on every single turn.

---

## Audit Trail

ARLI logs every agent action in **OCSF (Open Cybersecurity Schema Framework)** format, the emerging industry standard for security telemetry.

### Event Format

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

### SIEM Integration

OCSF `class_uid 6007` events are directly ingestible by:
- **Splunk** — via HEC or file monitor
- **Elasticsearch / ELK** — via Filebeat or Logstash
- **Datadog** — via Agent log integration
- **Wazuh** — via agent log collector
- **Graylog** — via GELF or file input

The event hash (`ocsf_event_hash`) provides a cryptographic fingerprint for each log entry. When combined with the attestation protocol, auditors can verify that a log entry was produced by a specific agent execution in a specific sandbox configuration — a complete chain of custody from execution to audit.

---

## Security Recommendations

### Deployment Hardening

1. **Run the gateway as a non-root systemd unit** with `User=arli`, `NoNewPrivileges=yes`, `ProtectSystem=strict`.
2. **Enable Landlock HardRequirement** for any deployment handling financial transactions or PII.
3. **Use separate Linux user accounts** for gateway, trading engine, and sandboxed agent execution.
4. **Mount workspaces on `noexec,nosuid`** filesystems when possible.
5. **Enable SELinux or AppArmor** as an additional LSM layer above Landlock.

### Key Management

1. **Never store ed25519 signing keys on the same filesystem as agent workspaces.**
2. **Use hardware security modules (HSMs) or TPM-backed key storage** for production ENSO attestation keys.
3. **Rotate provider API keys every 90 days**. Use credential pools to support zero-downtime rotation.
4. **Set file permissions `0o600`** on all key files — ARLI does this automatically, verify with `arli doctor`.

### Monitoring

1. **Export OCSF logs to a centralized SIEM** in real time. Configure alerts for:
   - Seccomp violations (indicates attempted syscall abuse)
   - Landlock denials (indicates filesystem escape attempts)
   - Guardrail blocks (indicates safety policy violations)
   - Attestation verification failures (indicates tampering or replay)
2. **Enable Prometheus metrics** at `:9090/metrics`. Monitor:
   - `arli_guardrail_blocks_total` — safety blocks by mode and classification
   - `arli_sandbox_isolated_total` — isolated vs. non-isolated executions
   - `arli_attestation_sign_total` — attestation count and latency
3. **Set up dead-man's switch monitoring** for the ENSO oracle — if attestations stop flowing, investigate immediately.

### Incident Response

1. **Guardrail blocks**: Review the classification and tool history. Adjust policy rules if false positives.
2. **Seccomp violations**: Immediate investigation. A blocked syscall means the agent attempted a prohibited operation. Review the tool call that triggered it.
3. **Key compromise**: Revoke the affected ed25519 key by removing it from the ENSO registry. Generate a new keypair and re-register. Rotate all provider keys.
4. **Audit log tampering**: The SHA-256 OCSF event hash chain makes log tampering detectable. Compare hashes against off-site copies.

---

## Namespaces and cgroups Isolation

In addition to the three-layer kernel sandbox (seccomp → Landlock → privdrop), ARLI leverages Linux namespaces and cgroups for broader resource isolation.

### Namespace Isolation

| Namespace | Purpose | ARLI Default |
|---|---|---|
| **Mount** (`CLONE_NEWNS`) | Private `/tmp` via tmpfs. Prevents sandbox from seeing or modifying host mounts | On |
| **Network** (`CLONE_NEWNET`) | Isolated network stack. Combined with iptables `OUTPUT DROP` when `allow_network: false` | On |
| **PID** (`CLONE_NEWPID`) | Sandboxed processes cannot see or signal host processes. PID 1 inside namespace is the sandbox root | On |
| **UTS** (`CLONE_NEWUTS`) | Isolated hostname and domain name | On |
| **IPC** (`CLONE_NEWIPC`) | Isolated System V IPC and POSIX message queues | On |

Namespaces are created via `unshare(1)` when available, with a graceful fallback to direct execution if the invoking process lacks `CAP_SYS_ADMIN`. In the `execute_isolated()` path (the primary path for production), namespaces are implicit — the `pre_exec()` hook handles all three kernel sandbox layers regardless of namespace availability.

### Resource Limits via cgroups

ARLI enforces resource limits through both `ulimit` wrappers (in-shell) and cgroups (when available):

- **Memory**: `memory_limit_bytes` (default: 512 MB). Enforced via `ulimit -v` and, on cgroups v2 systems, through `memory.max` in the sandbox cgroup.
- **CPU time**: `cpu_time_limit_secs` (default: 60s). Enforced via `ulimit -t`. cgroups v2 `cpu.max` is used when available for more precise throttling.
- **File size**: `max_file_size_bytes` (default: 100 MB). Enforced via `ulimit -f`.
- **Wall-clock timeout**: `timeout_secs` (default: 30s). Enforced via the `timeout(1)` command wrapper, which sends SIGTERM followed by SIGKILL.

The combination of namespaces, cgroups, and the three-layer kernel sandbox means that even a compromised agent process faces six distinct isolation boundaries before it can affect the host system.

---

## Supply Chain Security

ARLI's security model extends to its own build and distribution pipeline.

### Binary Integrity

Every ARLI binary release is accompanied by a SHA-256 hash published in the GitHub Release notes. The `arli update` command verifies this hash before replacing the running binary. The binary hash is also embedded in every attestation, creating an auditable link between the software version and every execution result.

### Dependency Verification

ARLI's Rust dependency tree is pinned via `Cargo.lock`. All cryptographic dependencies (`ed25519-dalek`, `sha2`, `seccompiler`, `landlock`) are sourced from crates.io with version pinning. The build process runs on GitHub Actions with reproducible build flags.

### Deliberate Minimalism

The 12 MB binary size is a security property: fewer dependencies mean a smaller attack surface. ARLI has zero runtime dependencies — no Python, no Node.js, no Docker daemon, no system services required. The binary is self-contained, including the ripgrep search engine linked directly into the binary to avoid fork/exec overhead and reduce external surface area.

---

## Security Testing & Validation

ARLI's security controls are validated through automated testing and manual review.

### Automated Security Tests (254 total, 0 failures)

| Test Category | Count | Coverage |
|---|---|---|
| Attestation signing and verification | 8 | Key generation, signing, tamper detection, replay protection, key persistence |
| Sandbox configuration | 6 | Default policies, permissive/strict modes, YAML round-trip, namespace args |
| Landlock enforcement | 4 | Restrictive, permissive, hard requirement, nonexistent path handling |
| Seccomp filter | 2 | Filter build determinism, filter non-empty |
| Privilege drop | 3 | Non-root no-op, root rejection, core dump disabling |
| Guardrail (policy-based) | 4 | Trading limits, destructive commands, credential leakage, safe trajectories |
| Guardrail (LLM judge) | Via integration | JSON parsing, confidence threshold, edge cases |

### Key Security Properties Verified by Tests

- **Attestation tamper resistance**: Modifying `job_id` or `timestamp_ns` after signing causes `verify()` to return false (tests: `test_signature_rejects_tampered_attestation`, `test_replay_protection_different_job`, `test_replay_protection_different_timestamp`)
- **Cross-keypair forgery prevention**: An attestation signed by `kp1` but with `kp2`'s public key fails verification (test: `test_different_keypair_fails`)
- **Deterministic seccomp**: The same syscall blocklist always produces identical BPF bytecode (test: `test_filter_is_deterministic`)
- **Default-deny posture**: `SandboxPolicy::default()` returns a restrictive policy with network blocked and privilege drop to `nobody` (test: `test_default_policy_is_restrictive`)
- **Privilege drop verification**: Post-drop UID and GID are explicitly verified against the target, catching CWE-250-style failures (implementation in `privdrop.rs`, line 43-62)

### Manual Review Checklist

For production deployments, conduct the following manual security reviews before launch:

1. Review all sandbox policy YAML files — confirm no `read_write: ["/"]` in production
2. Confirm `landlock_compatibility = "hard_requirement"` for sensitive workloads
3. Verify Prometheus metrics are not publicly exposed (bind to `127.0.0.1` or use authentication)
4. Review tenant configurations for appropriate rate limits and model restrictions
5. Confirm OCSF audit logs are being exported to SIEM and alerts are configured
6. Test guardrail behavior with known-dangerous prompts in a staging environment

---

## Appendix: Cryptographic Dependencies

| Component | Library | Purpose |
|---|---|---|
| Key generation | `ed25519-dalek` | ed25519 keypair generation with `OsRng` |
| Signing | `ed25519-dalek` | EdDSA signing of attestation payloads |
| Verification | `ed25519-dalek` | Signature verification by ENSO canister |
| Hashing | `sha2` (SHA-256) | Message hash, event hash, config hash, binary hash |
| Seccomp BPF | `seccompiler` | BPF program compilation and application |
| Landlock | `landlock` (v0.4+) | Landlock ABI V3 ruleset construction |
| Privilege drop | `nix` + `libc` | initgroups, setgid, setuid, prctl, setrlimit |
