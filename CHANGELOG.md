# ARLI Changelog

## v0.6.0 — ENSO Production Ready

### ENSO Oracle Daemon
- `arli enso oracle start` — background daemon with PID file
- `arli enso oracle stop` — graceful shutdown (SIGTERM → SIGKILL after 10s)
- `arli enso oracle status` — show PID and log size
- `arli enso oracle log -l N` — tail last N log lines
- Daemon writes PID to `~/.arli/enso_oracle.pid`, logs to `~/.arli/enso_oracle.log`
- Catches SIGTERM/SIGINT for clean exit, removes PID file on stop

### Revenue Tracking
- `arli enso revenue` — parse oracle logs, show total contracts + ICP earned
- `arli enso revenue -r N` — show last N settlements with tx_id and amounts
- Distinguishes paid settlements from attested-only

### Binary Hash Approval Check
- Oracle (foreground + daemon) checks ENSO for approved binaries on startup
- If hash not approved: prints exact `approve_arli_binary` command for ENSO operator
- `list_approved_binaries` method added to `EnsoClient`

### Contract Inspection
- `arli enso admin status <contract>` — human-readable contract + escrow status
- `arli enso admin escrow <id>` — raw escrow details
- Parses Candid hashed fields into named fields (status, amount_funded, token, etc.)

### Idempotency & Reliability
- `submit_arli_payment` idempotent for all terminal escrow states (Completed, Refunded, Released)
- Deadline safety: oracle skips contracts within 5 minutes of deadline
- Auto-release support: escrow returns funds after deadline
- Binary approval check prevents silent attestation failures

### CLI
- `arli enso oracle` — daemon management (was alias for `run`, now separate)
- `arli enso revenue` — revenue tracking
- `arli enso admin` — contract inspection
- `arli enso run` — foreground oracle (unchanged)

## v0.5.0 — ENSO Mainnet Integration

- ENSO oracle: automated contract execution + attestation
- `submit_arli_payment` — atomic attest + settle on ICP
- Trading handler: mean-reversion, trend-following strategies
- Sandbox: Landlock + seccomp + nobody user
- Ed25519 attestation signing
- `arli enso onboard` — one-shot setup (keygen, config, registration)
- ENSO config: `~/.arli/enso.toml`
