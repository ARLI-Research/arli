# ARLI Compliance Matrix

**Version:** 1.0 — June 2026  
**Target Audience:** Compliance officers, security auditors, enterprise procurement teams  
**Scope:** Regulatory and framework compliance mapping for ARLI v0.5.0+

---

## Introduction

AI agent systems present novel compliance challenges. Unlike traditional SaaS applications, autonomous agents execute code, make financial decisions, and interact with external systems without human-in-the-loop approval on every action. ARLI addresses these challenges through a defense-in-depth architecture: kernel-level sandboxing, cryptographic attestation, SIEM-native audit trails, and a safety guardrail with configurable enforcement modes.

This document maps ARLI's security controls to major regulatory frameworks and compliance standards, providing a reference for audit preparation and vendor risk assessment.

---

## Framework Mappings

### SOC 2 Type II — Trust Service Criteria

SOC 2 evaluates systems against five Trust Service Criteria (TSC). ARLI's coverage:

#### Security (Common Criteria — CC1–CC9)

| Criterion | ARLI Control | Evidence |
|---|---|---|
| **CC1.1** — COSO Principle 1 (integrity/ethics) | Guardrail enforces safety policies; no agent action bypasses the sandbox | Guardrail block logs in OCSF audit trail |
| **CC3.1** — COSO Principle 5 (accountability) | Ed25519 attestations cryptographically bind each execution to an agent identity | `ArliAttestation` with embedded public key and signature |
| **CC5.2** — Control activities | Three-layer sandbox (seccomp → Landlock → privdrop) enforced at kernel level | Sandbox configuration hash in every attestation |
| **CC6.1** — Logical access controls | Per-agent sandbox policies; credential pools with gateway-managed keys; `0o600` key file permissions | Sandbox policy YAML files; `arli doctor` output |
| **CC6.2** — User access provisioning | Tenant API keys with rate limiting and model restrictions | Tenant configuration in `config.toml` |
| **CC6.6** — External threats | Seccomp BPF blocks 20 dangerous syscall classes; Landlock prevents filesystem escapes; TTSR stream rules detect credential leakage | Seccomp violation logs; Landlock denial events; guardrail blocks |
| **CC7.2** — System monitoring | Prometheus metrics endpoint; OCSF audit events with SHA-256 hashing; health check endpoint | Metrics dashboard; SIEM-exported OCSF logs |
| **CC8.1** — Change management | Binary hash embedded in every attestation; deterministic seccomp filter; content-hash-anchored policy config | Auditable binary hash chain; attestation verification |

#### Availability (A1)

| Criterion | ARLI Control |
|---|---|
| **A1.1** — Availability monitoring | `/health` endpoint; Prometheus `up` metric; systemd auto-restart with `Restart=always` |
| **A1.2** — Capacity management | Tenant rate limiting prevents resource exhaustion; sandbox memory/timeout limits per process |

#### Confidentiality (C1)

| Criterion | ARLI Control |
|---|---|
| **C1.1** — Confidential information identification and protection | Gateway pattern — agents never see raw credentials; environment variable isolation; key files at `0o600` |
| **C1.2** — Data disposal | Configurable session reset policies (`inactivity_daily`, `daily`, `never`); FTS5 searchable session store supports deletion |

#### Processing Integrity (PI1)

| Criterion | ARLI Control |
|---|---|
| **PI1.3** — Accuracy of processing | Attestation protocol proves job was executed as specified; OCSF event hash chain enables end-to-end verification |
| **PI1.4** — Data input integrity | Stream rules (TTSR) validate output against regex policies; guardrail blocks unsafe responses |

---

### ISO 27001:2022 — Annex A Controls Mapping

| Annex A Control | ARLI Implementation |
|---|---|
| **A.5.1** — Policies for information security | Sandbox policies in YAML (version-controlled, hash-anchored); guardrail policy configuration |
| **A.5.15** — Access control | Tenant API keys with SHA-256 hashing; per-tenant sandbox profiles; credential pools with weighted rotation |
| **A.5.16** — Identity management | Ed25519 keypairs for agent identity; ENSO canister registration of public keys and capabilities |
| **A.5.17** — Authentication information | Private keys stored at `0o600`; never logged; never in memory as plaintext after use |
| **A.5.24** — Information security incident management | OCSF audit trail with cryptographic chain of custody; guardrail block events with full classification |
| **A.5.33** — Protection of records | Append-only OCSF audit log; SHA-256 event hashing; off-site replication via SIEM integration |
| **A.8.1** — User endpoint devices | macOS Seatbelt sandbox for development environments; Linux Landlock for production |
| **A.8.2** — Privileged access rights | Privilege drop (UID 65534 = nobody) before any code execution; no root access in sandbox |
| **A.8.8** — Technical vulnerability management | Seccomp BPF blocks known dangerous syscalls; io_uring, ptrace, mount API, userfaultfd all blocked |
| **A.8.9** — Configuration management | Binary hash embedded in attestations; deterministic seccomp filter; policy YAML content-hashed |
| **A.8.12** — Data leakage prevention | Gateway pattern prevents agent access to raw credentials; TTSR stream rules detect credential leakage in output |
| **A.8.16** — Monitoring activities | Prometheus metrics; OCSF audit events; health check endpoint; guardrail activity logging |
| **A.8.20** — Network security | Network namespace isolation; iptables OUTPUT DROP; proxy mode with TLS-enforced endpoints |
| **A.8.25** — Secure development life cycle | Rust (memory-safe); 254 tests, 0 failures; CI/CD with GitHub Actions; multi-platform releases |
| **A.8.28** — Secure coding | `unsafe` blocks scoped to FFI boundaries; pre_exec() isolation hook; privilege drop with post-drop verification |

---

### GDPR — EU General Data Protection Regulation

| Requirement | ARLI Implementation |
|---|---|
| **Art. 5(1)(c)** — Data minimization | Session data stored in SQLite; FTS5 search enables targeted retrieval; configurable auto-purge via session reset policies |
| **Art. 17** — Right to erasure | Session store with deletion support; memory system with `remove` action; OCSF audit events can be purged per session |
| **Art. 25** — Data protection by design | Sandbox isolation prevents unauthorized data access; gateway proxy pattern limits credential exposure |
| **Art. 32** — Security of processing | Kernel-level sandboxing; ed25519 attestation; seccomp syscall filtering; Landlock filesystem boundaries |
| **Art. 33/34** — Breach notification | OCSF audit trail enables rapid detection and scoping of breaches; log aggregation to SIEM enables alerting |
| **Art. 35** — DPIA readiness | Sandbox configuration is auditable (content-hashed); attestation proves what code executed in what environment; guardrail logs show all safety blocks |

**Recommendations for GDPR deployment:**
- Set `session_reset.mode = "inactivity_daily"` with appropriate `inactivity_minutes`
- Implement a data retention policy for OCSF audit logs (e.g., 90-day retention)
- Document the lawful basis for processing in `config.toml` metadata
- Use `arli session_search` to locate and delete user data upon DSAR request

---

### HIPAA — Health Insurance Portability and Accountability Act

**Applicability:** ARLI is not a healthcare application, but may process PHI if deployed in a healthcare context.

| HIPAA Safeguard | ARLI Control | Status |
|---|---|---|
| **Access Control** (§164.312(a)(1)) | Tenant API keys; per-tenant sandbox profiles; credential pools | ✅ Implemented |
| **Audit Controls** (§164.312(b)) | OCSF audit trail with SHA-256 hashing; SIEM integration | ✅ Implemented |
| **Integrity** (§164.312(c)(1)) | Ed25519 attestation protocol; OCSF event hash chain | ✅ Implemented |
| **Person or Entity Authentication** (§164.312(d)) | Ed25519 keypairs; ENSO registry; tenant API key hashing | ✅ Implemented |
| **Transmission Security** (§164.312(e)(1)) | TLS-enforced proxy mode; network namespace isolation | ✅ Implemented |
| **Encryption at Rest** | Key files at `0o600`; SQLite with file-level encryption (requires OS/filesystem-level encryption) | ⚠️ Requires OS-level encryption (LUKS, FileVault) |

**HIPAA Deployment Requirements:**
1. Enable full-disk encryption (LUKS/dm-crypt on Linux, FileVault on macOS)
2. Configure `landlock_compatibility = "hard_requirement"`
3. Encrypt OCSF audit logs at rest via filesystem encryption
4. Implement BAA (Business Associate Agreement) with any third-party LLM providers
5. Disable model provider logging on all API accounts
6. Configure session reset to `daily` with short retention

---

### PCI DSS — Payment Card Industry Data Security Standard

**Applicability:** Relevant if ARLI processes, transmits, or stores cardholder data (CHD).

| PCI DSS Requirement | ARLI Control |
|---|---|
| **Req 1** — Firewalls | Network namespace isolation; iptables OUTPUT DROP; proxy mode with TLS enforcement |
| **Req 2** — Secure configurations | Sandbox policy YAML (content-hashed); privilege drop to `nobody`; `0o600` key permissions |
| **Req 3** — Protect stored cardholder data | Gateway pattern prevents agent access to raw data; TTSR stream rules catch credential leakage |
| **Req 4** — Encrypt transmission | TLS-enforced network endpoints; proxy mode requires TLS on all allowed hosts |
| **Req 6** — Secure systems | Rust (memory-safe); 254 tests; seccomp BPF blocks memory manipulation syscalls |
| **Req 7** — Access control | Tenant isolation; per-agent sandbox policies; credential pools |
| **Req 10** — Track and monitor access | OCSF audit trail (class_uid 6007); SHA-256 event hashing; SIEM integration |
| **Req 11** — Test security | `arli doctor` health checks; 254 automated tests; deterministic security controls |

**Cardholder Data Isolation:**
- ARLI does **not** store cardholder data natively. If deployed in a CDE (Cardholder Data Environment), the sandbox policy must exclude all `read_write` paths that could contain CHD.
- Payment processing should occur through the ENSO settlement loop (ICP canister-based, on-chain), which isolates ARLI from direct CHD handling.

---

### NIST AI RMF — AI Risk Management Framework

NIST AI RMF 1.0 defines four core functions: Govern, Map, Measure, Manage. ARLI's alignment:

#### Govern

| NIST Category | ARLI Implementation |
|---|---|
| **GOVERN 1.1** — Legal/regulatory requirements | Compliance matrix (this document); configurable guardrail modes |
| **GOVERN 1.2** — AI actor accountability | Ed25519 attestations bind every execution to a specific agent identity |
| **GOVERN 1.3** — Workforce AI competency | `arli setup` interactive wizard; documentation-first approach; extensive CLI help |
| **GOVERN 1.6** — Policies and procedures | Sandbox policies (YAML); guardrail policies; stream rules (TTSR) |

#### Map

| NIST Category | ARLI Implementation |
|---|---|
| **MAP 1.1** — Intended purpose and context | Per-agent configuration (`max_iterations`, `tool_progress`, policy assignment) |
| **MAP 2.3** — AI capabilities and limitations | 18 built-in tools with documented capabilities; sandbox resource limits enforce boundaries |
| **MAP 3.5** — Impact on individuals and society | Guardrail 3D risk taxonomy includes PsychologicalHarm, PrivacyHarm, ReputationalHarm |

#### Measure

| NIST Category | ARLI Implementation |
|---|---|
| **MEASURE 2.1** — Testing and evaluation | 254 tests (0 failures); CI/CD pipeline; deterministic security controls |
| **MEASURE 2.3** — Trustworthiness | Attestation protocol proves execution integrity; OCSF audit trail provides full observability |
| **MEASURE 2.6** — Safety evaluation | AgentDoG 1.5 guardrail with policy-based, LLM judge, and hybrid modes |
| **MEASURE 2.8** — Transparency | Open-source (MIT); documented architecture; auditable binary and sandbox hashes |

#### Manage

| NIST Category | ARLI Implementation |
|---|---|
| **MANAGE 1.2** — Risk treatment | Configurable guardrail enforcement; sandbox compatibility modes (best_effort/hard_requirement) |
| **MANAGE 2.1** — Incident response | OCSF audit trail enables rapid scoping; guardrail block logs for safety incidents |
| **MANAGE 2.3** — Monitoring and review | Prometheus metrics; health check; dashboard; SIEM integration |
| **MANAGE 3.1** — Stakeholder communication | OCSF log export to SIEM; attestation verification by external canisters |

---

## OCSF Audit Trail — SIEM Compatibility

### Event Schema

ARLI produces OCSF events with `class_uid: 6007` (Agent Activity). Events are written as JSON Lines to `~/.arli/audit/` and can be shipped to any SIEM platform.

### SIEM Integration Summary

| SIEM Platform | Ingestion Method | Configuration |
|---|---|---|
| **Splunk** | HEC (HTTP Event Collector) or UF file monitor | Monitor `~/.arli/audit/*.jsonl`; sourcetype `arli:ocsf` |
| **Elasticsearch / ELK** | Filebeat → Logstash → Elasticsearch | Filebeat `filestream` input on `~/.arli/audit/` |
| **Datadog** | Agent log integration | Add `~/.arli/audit/*.jsonl` to `conf.d/arli.d/conf.yaml` |
| **Wazuh** | Agent log collector | Add `<localfile>` entry for audit directory |
| **Graylog** | File input or GELF | File input on audit directory or GELF HTTP sender |

### Sample SIEM Queries

**Splunk:**
```
sourcetype="arli:ocsf" class_uid=6007 activity_name=Execute
| stats count by agent.policy result
```

**Elasticsearch:**
```json
{
  "query": {
    "bool": {
      "must": [
        {"term": {"class_uid": 6007}},
        {"term": {"activity_name": "Execute"}}
      ]
    }
  }
}
```

---

## Attestation Chain of Custody

The attestation protocol creates a cryptographic chain of custody from agent execution to audit:

```
Agent Execution
     │
     ├─ OCSF event generated (class_uid 6007)
     │   └─ SHA-256(event_json) → ocsf_event_hash
     │
     ├─ Attestation built:
     │   run_id || job_id || timestamp_ns || ocsf_event_hash
     │   || sandbox_config_hash || arli_binary_hash
     │   || landlock_enforced || seccomp_enforced || uid
     │
     ├─ Ed25519 signature over SHA-256 of above
     │   └─ ArliAttestation.signature
     │
     └─ ENSO Canister verifies:
         ├─ ed25519 signature valid for registered public key
         ├─ binary_hash matches registered agent
         ├─ sandbox_config_hash matches known-policy
         ├─ landlock_enforced == true
         ├─ seccomp_enforced == true
         └─ → Escrow released, payment settled
```

**For auditors, this proves:**
1. The agent execution happened (non-repudiation via ed25519)
2. The execution was sandboxed (Landlock + seccomp flags in attestation)
3. The sandbox policy was the expected one (config hash matches)
4. The binary was the authorized version (binary hash matches)
5. The audit log is complete and untampered (OCSF event hash chain)
6. The attestation cannot be replayed (timestamp + job_id in message hash)

---

## Data Flow Diagram

```
                          ┌──────────────────────┐
                          │   End Users (20       │
                          │   messaging platforms)│
                          └──────────┬───────────┘
                                     │ (TLS)
                          ┌──────────▼───────────┐
                          │   ARLI Gateway        │
                          │   · Auth (API keys)   │
                          │   · Rate limiting     │
                          │   · Platform routing  │
                          └──────────┬───────────┘
                                     │
              ┌──────────────────────┼──────────────────────┐
              │                      │                      │
    ┌─────────▼─────────┐  ┌────────▼────────┐  ┌─────────▼─────────┐
    │  Agent Actor       │  │  Guardrail      │  │  Sandbox          │
    │  · Model call      │  │  · Policy eval  │  │  · Seccomp BPF    │
    │  · Tool execution  │  │  · LLM judge    │  │  · Landlock LSM   │
    │  · Context mgmt    │  │  · Pre-Reply    │  │  · Privdrop       │
    └────────┬───────────┘  └────────┬────────┘  └────────┬──────────┘
             │                       │                    │
             └───────────────────────┼────────────────────┘
                                     │
                         ┌───────────▼───────────┐
                         │   OCSF Audit Trail    │
                         │   · JSONL events      │
                         │   · SHA-256 hashed    │
                         │   · SIEM-exported     │
                         └───────────┬───────────┘
                                     │
                         ┌───────────▼───────────┐
                         │   ENSO/ICP Canister   │
                         │   · Attestation verify│
                         │   · Escrow release    │
                         │   · Payment settlement│
                         └───────────────────────┘
                                     │
                         ┌───────────▼───────────┐
                         │   Hyperliquid         │
                         │   · Trade execution   │
                         │   · WS fusion feed    │
                         │   · Position mgmt     │
                         └───────────────────────┘
```

**Data classification by zone:**

| Zone | Data Types | Protection |
|---|---|---|
| Gateway → Agent | User messages, platform metadata | TLS in transit; guardrail-evaluated before delivery |
| Agent → Model | Prompt text, context window | TLS in transit; provider API keys via env vars only |
| Model → Agent | LLM response text | Guardrail evaluated before user delivery |
| Agent → Sandbox | Tool commands, working directory | Kernel-enforced isolation; namespace boundaries |
| Sandbox → OCSF | Execution results, exit codes | SHA-256 event hashing; append-only storage |
| OCSF → SIEM | Audit events | TLS in transit (SIEM collector); at-rest via OS encryption |
| Agent → ENSO | Signed attestation | Ed25519 signature; ICP HTTPS gateway |
| Agent → Hyperliquid | Trade orders | TLS + API key; safety limits at guardrail and engine level |

---

## Gap Analysis

### What's Covered

| Control Area | Coverage | Mechanism |
|---|---|---|
| Code execution isolation | ✅ Full | Seccomp + Landlock + privdrop (3-layer kernel sandbox) |
| Execution integrity proof | ✅ Full | Ed25519 attestation with replay protection |
| Safety guardrail | ✅ Full | AgentDoG 1.5 (policy, LLM judge, hybrid) |
| Audit trail | ✅ Full | OCSF class_uid 6007; SHA-256 hashed; SIEM-compatible |
| Credential protection | ✅ Full | Gateway pattern; env vars; 0o600 permissions |
| Network isolation | ✅ Full | Network namespaces; iptables; proxy mode with TLS |
| Tenant isolation | ✅ Partial | Per-tenant configs; per-agent sandbox policies (requires separate DB schemas for full isolation) |
| Memory safety | ✅ Full | Rust (no use-after-free, buffer overflows); unsafe only at FFI boundaries |

### What Needs Additional Controls

| Control Area | Gap | Mitigation |
|---|---|---|
| **Data-at-rest encryption** | SQLite and OCSF logs are not natively encrypted | Use LUKS/dm-crypt (Linux) or FileVault (macOS) for full-disk encryption |
| **HSM/TPM key storage** | ed25519 keys stored as files with filesystem permissions | Integrate with TPM-backed key storage or external HSM for production ENSO keys |
| **Network traffic inspection** | No deep packet inspection on model API calls | Deploy behind a corporate TLS inspection proxy; use provider allowlists |
| **Model supply chain verification** | Model weights not verified cryptographically | Pin to specific model versions; use provider-provided model hashes where available |
| **Multi-region data residency** | No built-in geo-routing for data residency | Deploy separate ARLI instances per region; use region-specific provider endpoints |
| **Access review automation** | No automated access review workflows | Integrate OCSF audit trail with IGA/IGM tools; periodic manual review of tenant configs |
| **DDoS protection** | No built-in DDoS mitigation at gateway layer | Deploy behind a CDN or L7 load balancer with rate limiting; configure tenant rate limits conservatively |

---

## External Audit Readiness Checklist

Use this checklist when preparing for a SOC 2, ISO 27001, or similar external audit of an ARLI deployment.

### Pre-Audit Preparation

- [ ] Document the ARLI deployment architecture (use the data flow diagram above as a starting point)
- [ ] Collect and review `config.toml` from all environments
- [ ] Verify `landlock_compatibility` setting per environment
- [ ] Confirm guardrail mode (`policy_based`, `llm_judge`, or `hybrid`)
- [ ] Review tenant configurations and rate limits
- [ ] Verify all provider API keys are stored in environment variables, not in config files
- [ ] Confirm ed25519 keypair presence and ENSO canister registration (if applicable)

### Evidence Collection

- [ ] Export OCSF audit logs for the audit period (from SIEM or `~/.arli/audit/`)
- [ ] Export Prometheus metrics for the audit period (guardrail block counts, sandbox execution counts)
- [ ] Collect `arli doctor` output from all hosts
- [ ] Collect sandbox policy YAML files from all agents
- [ ] Export attestation records from ENSO canister (for on-chain settlement workflows)
- [ ] Provide binary hashes for the ARLI versions in use during the audit period

### Control Demonstration

- [ ] Demonstrate sandbox isolation: attempt a blocked syscall → show seccomp EPERM
- [ ] Demonstrate guardrail: craft a prompt that triggers a safety block → show classification
- [ ] Demonstrate attestation: execute a job → show signed attestation with valid signature verification
- [ ] Demonstrate key protection: show `0o600` permissions on key files
- [ ] Demonstrate audit trail: query OCSF events for a specific agent session → show complete timeline
- [ ] Demonstrate tenant isolation: attempt cross-tenant access → show authorization failure

### Documentation

- [ ] System description and architecture diagram
- [ ] Risk assessment (can reference the Threat Model in the Security Whitepaper)
- [ ] Configuration management policy
- [ ] Key management and rotation policy
- [ ] Incident response runbook
- [ ] Data retention and deletion policy
- [ ] Business continuity and disaster recovery plan
- [ ] Vendor management (LLM provider BAA/DPA)

---

## Appendix: Framework Cross-Reference

| Control | SOC 2 | ISO 27001:2022 | GDPR | HIPAA | PCI DSS | NIST AI RMF |
|---|---|---|---|---|---|---|
| Sandbox (seccomp+Landlock+privdrop) | CC6.1, CC6.6 | A.8.2, A.8.28 | Art. 32 | §164.312(a)(1) | Req 2, Req 6 | MAP 2.3 |
| Attestation (ed25519) | CC3.1, PI1.3 | A.5.16, A.8.9 | Art. 32 | §164.312(c)(1) | Req 10 | GOVERN 1.2 |
| OCSF Audit Trail | CC7.2 | A.5.33, A.8.16 | Art. 33/34 | §164.312(b) | Req 10 | MEASURE 2.3 |
| Guardrail (AgentDoG 1.5) | PI1.4 | A.5.24 | Art. 25 | — | — | MEASURE 2.6, MANAGE 1.2 |
| Credential Management | CC6.1, C1.1 | A.5.17, A.8.12 | Art. 32 | §164.312(d) | Req 3, Req 7 | GOVERN 1.6 |
| Gateway (tenant isolation) | CC6.2 | A.5.15 | — | §164.312(a)(1) | Req 7 | MANAGE 1.2 |
| Prometheus Metrics | CC7.2, A1.1 | A.8.16 | — | — | — | MANAGE 2.3 |
| Stream Rules (TTSR) | P1.4 | A.8.12 | Art. 25 | — | Req 3 | MAP 2.3 |
