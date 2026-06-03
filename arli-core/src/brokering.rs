//! Inference Brokering — multi-tenant rate limiting, usage tracking, and billing.
//!
//! Provides tenant management with API key rotation, token-bucket rate limiting,
//! per-request usage recording, monthly billing reports with configurable margin,
//! and a tenant-aware router that wraps the core [`InferenceRouter`].

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::inference::{InferenceRoute, InferenceRouter};

// ═════════════════════════════════════════════════════════════════════════════
// Error type
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Error)]
pub enum BrokeringError {
    #[error("tenant not found: {0}")]
    TenantNotFound(Uuid),

    #[error("tenant is disabled: {0}")]
    TenantDisabled(Uuid),

    #[error("rate limit exceeded for tenant {tenant_id}: {detail}")]
    RateLimitExceeded { tenant_id: Uuid, detail: String },

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("all providers exhausted for tenant {0}")]
    AllProvidersExhausted(Uuid),
}

pub type BrokeringResult<T> = std::result::Result<T, BrokeringError>;

// ═════════════════════════════════════════════════════════════════════════════
// Configuration
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokeringConfig {
    #[serde(default = "default_margin")]
    pub margin_percent: f64,

    #[serde(default = "default_max_rpm")]
    pub default_max_rpm: u32,

    #[serde(default = "default_max_tpm")]
    pub default_max_tpm: u32,

    #[serde(default)]
    pub tenant_overrides: HashMap<String, TenantRateLimitOverride>,

    #[serde(default)]
    pub db_path: Option<String>,
}

fn default_margin() -> f64 { 0.15 }
fn default_max_rpm() -> u32 { 60 }
fn default_max_tpm() -> u32 { 200_000 }

impl Default for BrokeringConfig {
    fn default() -> Self {
        Self {
            margin_percent: default_margin(),
            default_max_rpm: default_max_rpm(),
            default_max_tpm: default_max_tpm(),
            tenant_overrides: HashMap::new(),
            db_path: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantRateLimitOverride {
    pub max_rpm: Option<u32>,
    pub max_tpm: Option<u32>,
}

// ═════════════════════════════════════════════════════════════════════════════
// Tenant Manager
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantInfo {
    pub id: Uuid,
    pub name: String,
    pub contact_email: String,
    pub api_key_hash: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

pub struct TenantManager {
    db: Arc<Mutex<Connection>>,
}

impl TenantManager {
    pub fn new(db_path: &str) -> BrokeringResult<Self> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tenants (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                contact_email TEXT NOT NULL,
                api_key_hash TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL
            );",
        )?;
        Ok(Self { db: Arc::new(Mutex::new(conn)) })
    }

    pub fn register_tenant(&self, name: &str, contact_email: &str) -> BrokeringResult<Uuid> {
        let id = Uuid::new_v4();
        let api_key = Uuid::new_v4().to_string();
        let api_key_hash = hex::encode(Sha256::digest(api_key.as_bytes()));
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO tenants (id, name, contact_email, api_key_hash, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5)",
            params![id.to_string(), name, contact_email, api_key_hash, Utc::now().to_rfc3339()],
        )?;
        debug!(tenant_id = %id, tenant_name = name, "tenant registered");
        Ok(id)
    }

    pub fn rotate_api_key(&self, tenant_id: Uuid) -> BrokeringResult<String> {
        let new_key = Uuid::new_v4().to_string();
        let new_hash = hex::encode(Sha256::digest(new_key.as_bytes()));
        let db = self.db.lock().unwrap();
        let rows = db.execute(
            "UPDATE tenants SET api_key_hash = ?1 WHERE id = ?2",
            params![new_hash, tenant_id.to_string()],
        )?;
        if rows == 0 {
            return Err(BrokeringError::TenantNotFound(tenant_id));
        }
        Ok(new_key)
    }

    pub fn list_tenants(&self) -> BrokeringResult<Vec<TenantInfo>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT id, name, contact_email, api_key_hash, enabled, created_at FROM tenants ORDER BY name",
        )?;
        let tenants = stmt.query_map([], |row| {
            Ok(TenantInfo {
                id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                name: row.get(1)?,
                contact_email: row.get(2)?,
                api_key_hash: row.get(3)?,
                enabled: row.get::<_, i32>(4)? != 0,
                created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                    .unwrap()
                    .with_timezone(&Utc),
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(tenants)
    }

    pub fn get_tenant(&self, tenant_id: Uuid) -> BrokeringResult<TenantInfo> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT id, name, contact_email, api_key_hash, enabled, created_at FROM tenants WHERE id = ?1",
        )?;
        stmt.query_row(params![tenant_id.to_string()], |row| {
            Ok(TenantInfo {
                id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap(),
                name: row.get(1)?,
                contact_email: row.get(2)?,
                api_key_hash: row.get(3)?,
                enabled: row.get::<_, i32>(4)? != 0,
                created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                    .unwrap()
                    .with_timezone(&Utc),
            })
        }).map_err(|_| BrokeringError::TenantNotFound(tenant_id))
    }

    pub fn enable_tenant(&self, tenant_id: Uuid) -> BrokeringResult<()> {
        let db = self.db.lock().unwrap();
        let rows = db.execute(
            "UPDATE tenants SET enabled = 1 WHERE id = ?1",
            params![tenant_id.to_string()],
        )?;
        if rows == 0 {
            return Err(BrokeringError::TenantNotFound(tenant_id));
        }
        Ok(())
    }

    pub fn disable_tenant(&self, tenant_id: Uuid) -> BrokeringResult<()> {
        let db = self.db.lock().unwrap();
        let rows = db.execute(
            "UPDATE tenants SET enabled = 0 WHERE id = ?1",
            params![tenant_id.to_string()],
        )?;
        if rows == 0 {
            return Err(BrokeringError::TenantNotFound(tenant_id));
        }
        Ok(())
    }
}

impl Clone for TenantManager {
    fn clone(&self) -> Self {
        Self { db: Arc::clone(&self.db) }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Rate Limiter
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct RateLimitResult {
    pub allowed: bool,
    pub remaining: u32,
    pub reset_at: DateTime<Utc>,
}

#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    last_refill: DateTime<Utc>,
    max_tokens: f64,
    refill_rate_per_sec: f64,
}

#[derive(Debug)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<Uuid, TokenBucket>>,
    config: BrokeringConfig,
}

impl RateLimiter {
    pub fn new(config: &BrokeringConfig) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            config: config.clone(),
        }
    }

    fn get_or_create_bucket(buckets: &mut HashMap<Uuid, TokenBucket>, tenant_id: Uuid, rpm: u32) {
        let now = Utc::now();
        buckets.entry(tenant_id).or_insert_with(|| TokenBucket {
            tokens: rpm as f64,
            last_refill: now,
            max_tokens: rpm as f64,
            refill_rate_per_sec: rpm as f64 / 60.0,
        });
    }

    fn get_rate_limits(&self, tenant_name: &str) -> (u32, u32) {
        let rpm = self.config.tenant_overrides.get(tenant_name)
            .and_then(|o| o.max_rpm)
            .unwrap_or(self.config.default_max_rpm);
        let tpm = self.config.tenant_overrides.get(tenant_name)
            .and_then(|o| o.max_tpm)
            .unwrap_or(self.config.default_max_tpm);
        (rpm, tpm)
    }

    pub fn check_rate(&self, tenant_id: Uuid, tenant_name: &str, estimated_tokens: u32) -> RateLimitResult {
        let (rpm, tpm) = self.get_rate_limits(tenant_name);
        let mut buckets = self.buckets.lock().unwrap();
        Self::get_or_create_bucket(&mut buckets, tenant_id, rpm);

        let bucket = buckets.get_mut(&tenant_id).unwrap();
        let now = Utc::now();
        let elapsed = (now - bucket.last_refill).num_milliseconds() as f64 / 1000.0;
        bucket.tokens = (bucket.tokens + elapsed * bucket.refill_rate_per_sec).min(bucket.max_tokens);
        bucket.last_refill = now;

        let cost = (1.0 + (estimated_tokens as f64 / tpm as f64) * bucket.max_tokens * 0.1).min(bucket.max_tokens);
        let allowed = bucket.tokens >= cost;
        if allowed {
            bucket.tokens -= cost;
        }

        let remaining = bucket.tokens.floor() as u32;
        let reset_at = now + chrono::Duration::seconds((bucket.max_tokens / bucket.refill_rate_per_sec) as i64);

        RateLimitResult { allowed, remaining, reset_at }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Usage Tracker
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    pub tenant_id: Uuid,
    pub provider: String,
    pub model: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_cents: u64,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageSummary {
    pub total_requests: u64,
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
    pub total_cost_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyUsage {
    pub date: NaiveDate,
    pub requests: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_cents: u64,
}

#[derive(Debug)]
pub struct UsageTracker {
    db: Arc<Mutex<Connection>>,
}

impl UsageTracker {
    pub fn new(db_path: &str) -> BrokeringResult<Self> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tenant_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                tokens_in INTEGER NOT NULL,
                tokens_out INTEGER NOT NULL,
                cost_cents INTEGER NOT NULL,
                recorded_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_usage_tenant ON usage_records(tenant_id, recorded_at);",
        )?;
        Ok(Self { db: Arc::new(Mutex::new(conn)) })
    }

    pub fn record_usage(
        &self, tenant_id: Uuid, provider: &str, model: &str,
        tokens_in: u64, tokens_out: u64, cost_cents: u64,
    ) -> BrokeringResult<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO usage_records (tenant_id, provider, model, tokens_in, tokens_out, cost_cents, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![tenant_id.to_string(), provider, model, tokens_in as i64, tokens_out as i64, cost_cents as i64, Utc::now().to_rfc3339()],
        )?;
        debug!(tenant_id = %tenant_id, provider, tokens_in, tokens_out, "usage recorded");
        Ok(())
    }

    pub fn get_usage(&self, tenant_id: Uuid, from: DateTime<Utc>, to: DateTime<Utc>) -> BrokeringResult<UsageSummary> {
        let db = self.db.lock().unwrap();
        let sql = "SELECT COUNT(*), COALESCE(SUM(tokens_in),0), COALESCE(SUM(tokens_out),0), COALESCE(SUM(cost_cents),0) FROM usage_records WHERE tenant_id = ?1 AND recorded_at >= ?2 AND recorded_at <= ?3";
        let mut stmt = db.prepare(sql)?;
        stmt.query_row(params![tenant_id.to_string(), from.to_rfc3339(), to.to_rfc3339()], |row| {
            Ok(UsageSummary {
                total_requests: row.get::<_, i64>(0)? as u64,
                total_tokens_in: row.get::<_, i64>(1)? as u64,
                total_tokens_out: row.get::<_, i64>(2)? as u64,
                total_cost_cents: row.get::<_, i64>(3)? as u64,
            })
        }).map_err(Into::into)
    }

    pub fn get_daily_usage(&self, tenant_id: Uuid, since: DateTime<Utc>) -> BrokeringResult<Vec<DailyUsage>> {
        let db = self.db.lock().unwrap();
        let sql = "SELECT date(recorded_at) as d, COUNT(*), COALESCE(SUM(tokens_in),0), COALESCE(SUM(tokens_out),0), COALESCE(SUM(cost_cents),0) FROM usage_records WHERE tenant_id = ?1 AND recorded_at >= ?2 GROUP BY d ORDER BY d ASC";
        let mut stmt = db.prepare(sql)?;
        let rows = stmt.query_map(params![tenant_id.to_string(), since.to_rfc3339()], |row| {
            let date_str: String = row.get(0)?;
            Ok(DailyUsage {
                date: NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").unwrap(),
                requests: row.get::<_, i64>(1)? as u64,
                tokens_in: row.get::<_, i64>(2)? as u64,
                tokens_out: row.get::<_, i64>(3)? as u64,
                cost_cents: row.get::<_, i64>(4)? as u64,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

impl Clone for UsageTracker {
    fn clone(&self) -> Self {
        Self { db: Arc::clone(&self.db) }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Billing Reporter
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderBreakdown {
    pub provider: String,
    pub requests: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_cents: u64,
    pub with_margin_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonthlyBillingReport {
    pub tenant_id: Uuid,
    pub tenant_name: String,
    pub year: i32,
    pub month: u32,
    pub total_requests: u64,
    pub total_cost_cents: u64,
    pub total_with_margin_cents: u64,
    pub margin_percent: f64,
    pub provider_breakdown: Vec<ProviderBreakdown>,
    pub generated_at: DateTime<Utc>,
}

pub struct BillingReporter {
    tracker: UsageTracker,
    tenant_manager: TenantManager,
    config: BrokeringConfig,
}

impl BillingReporter {
    pub fn new(tracker: UsageTracker, tenant_manager: TenantManager, config: BrokeringConfig) -> Self {
        Self { tracker, tenant_manager, config }
    }

    pub fn generate_monthly_report(
        &self, tenant_id: Uuid, year: i32, month: u32,
    ) -> BrokeringResult<MonthlyBillingReport> {
        let tenant = self.tenant_manager.get_tenant(tenant_id)?;
        let from: DateTime<Utc> = Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).unwrap();
        let to: DateTime<Utc> = if month == 12 {
            Utc.with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0).unwrap()
        } else {
            Utc.with_ymd_and_hms(year, month + 1, 1, 0, 0, 0).unwrap()
        } - chrono::Duration::seconds(1);

        let summary = self.tracker.get_usage(tenant_id, from, to)?;
        let db = self.tracker.db.lock().unwrap();
        let sql = "SELECT provider, COUNT(*), COALESCE(SUM(tokens_in),0), COALESCE(SUM(tokens_out),0), COALESCE(SUM(cost_cents),0) FROM usage_records WHERE tenant_id = ?1 AND recorded_at >= ?2 AND recorded_at <= ?3 GROUP BY provider ORDER BY provider";
        let mut stmt = db.prepare(sql)?;
        let breakdown = stmt.query_map(
            params![tenant_id.to_string(), from.to_rfc3339(), to.to_rfc3339()],
            |row| {
                let cost: i64 = row.get(4)?;
                let cost_u64 = cost as u64;
                let margin = (cost_u64 as f64 * (1.0 + self.config.margin_percent)) as u64;
                Ok(ProviderBreakdown {
                    provider: row.get(0)?,
                    requests: row.get::<_, i64>(1)? as u64,
                    tokens_in: row.get::<_, i64>(2)? as u64,
                    tokens_out: row.get::<_, i64>(3)? as u64,
                    cost_cents: cost_u64,
                    with_margin_cents: margin,
                })
            },
        )?.collect::<std::result::Result<Vec<_>, _>>()?;

        let total_with_margin = (summary.total_cost_cents as f64 * (1.0 + self.config.margin_percent)) as u64;

        Ok(MonthlyBillingReport {
            tenant_id, tenant_name: tenant.name, year, month,
            total_requests: summary.total_requests,
            total_cost_cents: summary.total_cost_cents,
            total_with_margin_cents: total_with_margin,
            margin_percent: self.config.margin_percent,
            provider_breakdown: breakdown,
            generated_at: Utc::now(),
        })
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Brokering Router
// ═════════════════════════════════════════════════════════════════════════════

pub struct BrokeringRouter {
    tenant_manager: TenantManager,
    rate_limiter: Arc<RateLimiter>,
    usage_tracker: Arc<UsageTracker>,
    inference: InferenceRouter,
}

impl std::fmt::Debug for BrokeringRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrokeringRouter")
            .field("rate_limiter", &self.rate_limiter)
            .field("usage_tracker", &self.usage_tracker)
            .finish_non_exhaustive()
    }
}

impl BrokeringRouter {
    pub fn new(
        tenant_manager: TenantManager,
        rate_limiter: Arc<RateLimiter>,
        usage_tracker: Arc<UsageTracker>,
    ) -> Self {
        Self { tenant_manager, rate_limiter, usage_tracker, inference: InferenceRouter::new() }
    }

    pub fn route(
        &self, tenant_id: Uuid, provider_name: &str,
        model: Option<&str>, estimated_tokens: u32,
    ) -> BrokeringResult<InferenceRoute> {
        let tenant = self.tenant_manager.get_tenant(tenant_id)?;
        if !tenant.enabled {
            return Err(BrokeringError::TenantDisabled(tenant_id));
        }
        let result = self.rate_limiter.check_rate(tenant_id, &tenant.name, estimated_tokens);
        if !result.allowed {
            return Err(BrokeringError::RateLimitExceeded {
                tenant_id,
                detail: format!("try again after {}", result.reset_at),
            });
        }
        self.inference.resolve(provider_name, model)
            .ok_or_else(|| BrokeringError::Config(format!("unknown provider: {provider_name}")))
    }

    pub fn route_with_fallback(
        &self, tenant_id: Uuid, primary: &str, fallbacks: &[&str],
        model: Option<&str>, estimated_tokens: u32,
    ) -> BrokeringResult<InferenceRoute> {
        let mut last_err = None;
        let all_providers = std::iter::once(&primary).chain(fallbacks.iter());
        for provider_name in all_providers {
            match self.route(tenant_id, provider_name, model, estimated_tokens) {
                Ok(route) => {
                    if provider_name != &primary {
                        debug!(tenant_id = %tenant_id, primary, fallback = provider_name, "routed via fallback");
                    }
                    return Ok(route);
                }
                Err(e) => {
                    warn!(tenant_id = %tenant_id, provider = provider_name, error = %e, "provider failed, trying next");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or(BrokeringError::AllProvidersExhausted(tenant_id)))
    }

    pub fn record_completion(
        &self, tenant_id: Uuid, provider: &str, model: &str,
        tokens_in: u64, tokens_out: u64, cost_cents: u64,
    ) -> BrokeringResult<()> {
        self.usage_tracker.record_usage(tenant_id, provider, model, tokens_in, tokens_out, cost_cents)
    }

    pub fn get_tenant_manager(&self) -> &TenantManager { &self.tenant_manager }
    pub fn get_usage_tracker(&self) -> &UsageTracker { &self.usage_tracker }
}

// ═════════════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;
    use tempfile::TempDir;

    fn setup_db_path() -> (String, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("brokering.db").to_string_lossy().to_string();
        (path, dir)
    }

    fn setup() -> (TenantManager, BrokeringConfig, TempDir) {
        let (path, dir) = setup_db_path();
        let tm = TenantManager::new(&path).unwrap();
        let cfg = BrokeringConfig::default();
        (tm, cfg, dir)
    }

    #[test]
    fn test_register_and_list_tenants() {
        let (tm, _cfg, _dir) = setup();
        let id1 = tm.register_tenant("acme-corp", "acme@example.com").unwrap();
        let id2 = tm.register_tenant("globex", "globex@example.com").unwrap();
        assert_ne!(id1, id2);
        let tenants = tm.list_tenants().unwrap();
        assert_eq!(tenants.len(), 2);
        assert!(tenants.iter().any(|t| t.name == "acme-corp"));
        assert!(tenants.iter().all(|t| t.enabled));
    }

    #[test]
    fn test_enable_disable_tenant() {
        let (tm, _cfg, _dir) = setup();
        let id = tm.register_tenant("test-co", "test@example.com").unwrap();
        assert!(tm.get_tenant(id).unwrap().enabled);
        tm.disable_tenant(id).unwrap();
        assert!(!tm.get_tenant(id).unwrap().enabled);
        tm.enable_tenant(id).unwrap();
        assert!(tm.get_tenant(id).unwrap().enabled);
    }

    #[test]
    fn test_disable_nonexistent_tenant() {
        let (tm, _cfg, _dir) = setup();
        let err = tm.disable_tenant(Uuid::new_v4()).unwrap_err();
        assert!(matches!(err, BrokeringError::TenantNotFound(_)));
    }

    #[test]
    fn test_api_key_rotation() {
        let (tm, _cfg, _dir) = setup();
        let id = tm.register_tenant("rotate-me", "rotate@example.com").unwrap();
        let original = tm.get_tenant(id).unwrap().api_key_hash;
        let new_key = tm.rotate_api_key(id).unwrap();
        assert!(!new_key.is_empty());
        let updated = tm.get_tenant(id).unwrap().api_key_hash;
        assert_ne!(original, updated);
    }

    #[test]
    fn test_rate_limiter_allows_first_request() {
        let (tm, cfg, _dir) = setup();
        let id = tm.register_tenant("rate-test", "rate@example.com").unwrap();
        let rl = RateLimiter::new(&cfg);
        let result = rl.check_rate(id, "rate-test", 1000);
        assert!(result.allowed);
    }

    #[test]
    fn test_rate_limiter_exhaustion() {
        let (tm, mut cfg, _dir) = setup();
        cfg.default_max_rpm = 1;
        let id = tm.register_tenant("tight", "tight@example.com").unwrap();
        let rl = RateLimiter::new(&cfg);
        assert!(rl.check_rate(id, "tight", 10).allowed);
        assert!(!rl.check_rate(id, "tight", 10).allowed);
    }

    #[test]
    fn test_rate_limiter_per_tenant_override() {
        let (tm, mut cfg, _dir) = setup();
        cfg.default_max_rpm = 60;
        cfg.tenant_overrides.insert("vip-tenant".into(), TenantRateLimitOverride { max_rpm: Some(1000), max_tpm: None });
        let id = tm.register_tenant("vip-tenant", "vip@example.com").unwrap();
        let rl = RateLimiter::new(&cfg);
        let result = rl.check_rate(id, "vip-tenant", 1000);
        assert!(result.allowed);
        assert!(result.remaining <= 1000);
    }

    #[test]
    fn test_usage_tracking() {
        let (path, _dir) = setup_db_path();
        let (tm, _cfg, _dir2) = setup();
        let ut = UsageTracker::new(&path).unwrap();
        let id = tm.register_tenant("usage-co", "usage@example.com").unwrap();
        ut.record_usage(id, "openai", "gpt-4o", 500, 200, 700).unwrap();
        ut.record_usage(id, "deepseek", "deepseek-chat", 300, 100, 50).unwrap();
        let from = Utc::now() - chrono::Duration::hours(1);
        let to = Utc::now() + chrono::Duration::hours(1);
        let summary = ut.get_usage(id, from, to).unwrap();
        assert_eq!(summary.total_requests, 2);
        assert_eq!(summary.total_tokens_in, 800);
        assert_eq!(summary.total_cost_cents, 750);
    }

    #[test]
    fn test_daily_usage() {
        let (path, _dir) = setup_db_path();
        let (tm, _cfg, _dir2) = setup();
        let ut = UsageTracker::new(&path).unwrap();
        let id = tm.register_tenant("daily-co", "daily@example.com").unwrap();
        ut.record_usage(id, "openai", "gpt-4o", 100, 50, 30).unwrap();
        ut.record_usage(id, "openai", "gpt-4o", 200, 100, 60).unwrap();
        let since = Utc::now() - chrono::Duration::hours(1);
        let daily = ut.get_daily_usage(id, since).unwrap();
        assert!(!daily.is_empty());
        let today = daily.iter().find(|d| d.date == Utc::now().date_naive()).unwrap();
        assert_eq!(today.requests, 2);
        assert_eq!(today.cost_cents, 90);
    }

    #[test]
    fn test_monthly_billing_report() {
        let (path, _dir) = setup_db_path();
        let tm = TenantManager::new(&path).unwrap();
        let ut = UsageTracker::new(&path).unwrap();
        let id = tm.register_tenant("billing-co", "billing@example.com").unwrap();
        ut.record_usage(id, "openai", "gpt-4o", 1000, 500, 100).unwrap();
        ut.record_usage(id, "deepseek", "deepseek-chat", 500, 200, 10).unwrap();
        let reporter = BillingReporter::new(ut, tm, BrokeringConfig::default());
        let now = Utc::now();
        let report = reporter.generate_monthly_report(id, now.date_naive().year(), now.date_naive().month()).unwrap();
        assert_eq!(report.total_requests, 2);
        assert_eq!(report.total_cost_cents, 110);
        assert!(report.total_with_margin_cents > report.total_cost_cents);
        assert_eq!(report.provider_breakdown.len(), 2);
        assert_eq!(report.margin_percent, 0.15);
    }

    #[test]
    fn test_brokering_router_routes_with_valid_tenant() {
        let (path, _dir) = setup_db_path();
        let tm = TenantManager::new(&path).unwrap();
        let cfg = BrokeringConfig::default();
        let id = tm.register_tenant("router-co", "router@example.com").unwrap();
        let rl = Arc::new(RateLimiter::new(&cfg));
        let ut = Arc::new(UsageTracker::new(&path).unwrap());
        let router = BrokeringRouter::new(tm, rl, ut);
        let route = router.route(id, "deepseek", None, 100).unwrap();
        assert_eq!(route.provider.name, "deepseek");
    }

    #[test]
    fn test_brokering_router_rejects_disabled_tenant() {
        let (path, _dir) = setup_db_path();
        let tm = TenantManager::new(&path).unwrap();
        let cfg = BrokeringConfig::default();
        let id = tm.register_tenant("disabled-co", "disabled@example.com").unwrap();
        tm.disable_tenant(id).unwrap();
        let rl = Arc::new(RateLimiter::new(&cfg));
        let ut = Arc::new(UsageTracker::new(&path).unwrap());
        let router = BrokeringRouter::new(tm, rl, ut);
        let err = router.route(id, "deepseek", None, 100).unwrap_err();
        assert!(matches!(err, BrokeringError::TenantDisabled(_)));
    }

    #[test]
    fn test_route_with_fallback_all_fail() {
        let (path, _dir) = setup_db_path();
        let tm = TenantManager::new(&path).unwrap();
        let cfg = BrokeringConfig::default();
        let id = tm.register_tenant("fb-fail", "fbf@example.com").unwrap();
        let rl = Arc::new(RateLimiter::new(&cfg));
        let ut = Arc::new(UsageTracker::new(&path).unwrap());
        let router = BrokeringRouter::new(tm, rl, ut);
        let err = router.route_with_fallback(id, "nope", &["also-nope"], None, 10).unwrap_err();
        assert!(matches!(err, BrokeringError::Config(_)) || matches!(err, BrokeringError::AllProvidersExhausted(_)));
    }

    #[test]
    fn test_record_completion_updates_usage() {
        let (path, _dir) = setup_db_path();
        let tm = TenantManager::new(&path).unwrap();
        let cfg = BrokeringConfig::default();
        let id = tm.register_tenant("comp-test", "comp@example.com").unwrap();
        let rl = Arc::new(RateLimiter::new(&cfg));
        let ut = Arc::new(UsageTracker::new(&path).unwrap());
        let router = BrokeringRouter::new(tm.clone(), rl, ut.clone());
        router.record_completion(id, "openai", "gpt-4o", 500, 200, 700).unwrap();
        let from = Utc::now() - chrono::Duration::minutes(5);
        let to = Utc::now() + chrono::Duration::minutes(5);
        let summary = ut.get_usage(id, from, to).unwrap();
        assert_eq!(summary.total_requests, 1);
        assert_eq!(summary.total_cost_cents, 700);
    }
}
