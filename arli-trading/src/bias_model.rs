//! Bias Model — multi-timeframe directional trend filter.
//!
//! Unlike the [FairValueModel] which reacts to every tick, the Bias Model changes
//! slowly and provides context for decision confidence. It answers: "Which way is
//! the market trending, and how strong is that trend?"
//!
//! Adapted from the coinman2 bot architecture (mlmodelpoly `bias_model.py`).
//!
//! # Components per timeframe
//!
//! 1. **Slope** (bps): linear regression of log-prices over N bars
//! 2. **EMA spread** (bps): (EMA_fast − EMA_slow) / EMA_slow × 10000
//! 3. **Direction**: UP / DOWN / NEUTRAL from combined score
//!
//! # Multi-TF aggregation
//!
//! Each timeframe contributes to an overall bias score with these weights:
//!
//! | Timeframe | Weight |
//! |-----------|--------|
//! | 1m        | 0.10   |
//! | 5m        | 0.20   |
//! | 15m       | 0.30   |
//! | 1h        | 0.40   |
//!
//! Higher timeframes have more weight because they filter out noise.
//!
//! # Normalization
//!
//! The original mlmodelpoly code had FIX-BIAS-002: "Previous normalization was too
//! aggressive! Real data shows slope: -1 to +3 bps, spread: -7 to +10 bps."
//! This implementation uses empirically calibrated normalization: slope/3, spread/15.

use std::collections::{HashMap, VecDeque};

// ── Configuration ──────────────────────────────────────────────────────────────

/// Per-timeframe state tracker.
#[derive(Debug, Clone)]
struct TfState {
    /// Rolling window of close prices (newest last).
    closes: VecDeque<f64>,
    /// Exponential moving average (fast).
    ema_fast: Option<f64>,
    /// Exponential moving average (slow).
    ema_slow: Option<f64>,
    /// Timestamp of last update.
    last_update: Option<u64>,
    /// Last observed close.
    last_close: Option<f64>,
}

impl TfState {
    fn new(capacity: usize) -> Self {
        Self {
            closes: VecDeque::with_capacity(capacity),
            ema_fast: None,
            ema_slow: None,
            last_update: None,
            last_close: None,
        }
    }
}

/// Multi-timeframe directional bias model.
///
/// # Usage
///
/// ```ignore
/// let mut bias = BiasModel::default();
///
/// // Feed 1-minute closes — auto-aggregates to higher TFs
/// bias.update_1m(close_price, timestamp_ms);
///
/// // Or update specific TF directly
/// bias.update_tf("5m", close_price, timestamp_ms);
///
/// // Get current bias
/// let snap = bias.snapshot(None);
/// // snap.dir → "UP" | "DOWN" | "NEUTRAL"
/// // snap.strength → 0.0–1.0
/// // snap.bias_up_prob → 0.0–1.0
/// ```
#[derive(Debug, Clone)]
pub struct BiasModel {
    // ── Parameters ──
    /// Bars for slope calculation via linear regression.
    slope_lookback: usize,
    /// EMA fast period.
    ema_fast_period: usize,
    /// EMA slow period.
    ema_slow_period: usize,
    /// Weight of slope component vs EMA spread (0–1).
    slope_weight: f64,
    /// Weight of EMA spread component (0–1). Must sum to 1 with slope_weight.
    ema_weight: f64,
    /// Score above this → UP direction.
    up_threshold: f64,
    /// Score below this → DOWN direction.
    down_threshold: f64,
    /// Snapshot cache TTL in milliseconds.
    snapshot_interval_ms: u64,

    // ── State ──
    states: HashMap<String, TfState>,
    /// Aggregation buffer: 1m closes queued for higher-TF construction.
    agg_buffer: Vec<f64>,
    agg_count: usize,

    // ── Cache ──
    last_snapshot_ms: Option<u64>,
    cached_snapshot: Option<BiasSnapshot>,
}

/// Result of a bias model snapshot.
#[derive(Debug, Clone)]
pub struct BiasSnapshot {
    /// Overall direction.
    pub dir: Direction3,
    /// How strong the bias is (0.0–1.0).
    pub strength: f64,
    /// Probability-like score biased toward UP (0.0–1.0).
    pub bias_up_prob: f64,
    /// 1.0 − bias_up_prob.
    pub bias_down_prob: f64,
    /// Per-timeframe breakdown.
    pub tf_breakdown: HashMap<String, TfBiasResult>,
    /// Timestamp of snapshot (ms).
    pub last_update_ms: u64,
}

/// Bias result for a single timeframe.
#[derive(Debug, Clone)]
pub struct TfBiasResult {
    /// Is this TF warm (≥ 5 bars).
    pub ready: bool,
    /// Slope in basis points per bar.
    pub slope_bps: Option<f64>,
    /// EMA spread in basis points.
    pub ema_spread_bps: Option<f64>,
    /// Combined bias score (0.0–1.0, 0.5 = neutral).
    pub bias_score: f64,
    /// Direction for this TF.
    pub dir: Direction3,
    /// Number of bars in window.
    pub n_bars: usize,
}

/// Three-way direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction3 {
    Up,
    Down,
    Neutral,
}

// ── Default Parameters ────────────────────────────────────────────────────────

const DEFAULT_SLOPE_LOOKBACK: usize = 20;
const DEFAULT_EMA_FAST: usize = 12;
const DEFAULT_EMA_SLOW: usize = 26;
const DEFAULT_SLOPE_WEIGHT: f64 = 0.6;
const DEFAULT_EMA_WEIGHT: f64 = 0.4;
const DEFAULT_UP_THRESHOLD: f64 = 0.55;
const DEFAULT_DOWN_THRESHOLD: f64 = 0.45;
const DEFAULT_SNAPSHOT_INTERVAL_MS: u64 = 10_000;

/// Timeframe weights for aggregation.
const TF_WEIGHTS: &[(&str, f64)] = &[
    ("1m", 0.10),
    ("5m", 0.20),
    ("15m", 0.30),
    ("1h", 0.40),
];

impl Default for BiasModel {
    fn default() -> Self {
        let mut states = HashMap::new();
        for &(tf, _) in TF_WEIGHTS {
            states.insert(tf.to_string(), TfState::new(100));
        }

        Self {
            slope_lookback: DEFAULT_SLOPE_LOOKBACK,
            ema_fast_period: DEFAULT_EMA_FAST,
            ema_slow_period: DEFAULT_EMA_SLOW,
            slope_weight: DEFAULT_SLOPE_WEIGHT,
            ema_weight: DEFAULT_EMA_WEIGHT,
            up_threshold: DEFAULT_UP_THRESHOLD,
            down_threshold: DEFAULT_DOWN_THRESHOLD,
            snapshot_interval_ms: DEFAULT_SNAPSHOT_INTERVAL_MS,
            states,
            agg_buffer: Vec::new(),
            agg_count: 0,
            last_snapshot_ms: None,
            cached_snapshot: None,
        }
    }
}

impl BiasModel {
    /// Create with custom parameters.
    pub fn new(
        slope_lookback: usize,
        ema_fast: usize,
        ema_slow: usize,
        slope_weight: f64,
        ema_weight: f64,
        up_threshold: f64,
        down_threshold: f64,
    ) -> Self {
        let mut model = Self::default();
        model.slope_lookback = slope_lookback;
        model.ema_fast_period = ema_fast;
        model.ema_slow_period = ema_slow;
        model.slope_weight = slope_weight;
        model.ema_weight = ema_weight;
        model.up_threshold = up_threshold;
        model.down_threshold = down_threshold;
        model
    }

    // ── Update Methods ──────────────────────────────────────────────────────

    /// Feed a 1-minute close price. Auto-aggregates to higher timeframes.
    ///
    /// - Every 1 bar → updates 1m
    /// - Every 5th bar → updates 5m
    /// - Every 15th bar → updates 15m
    /// - Every 60th bar → updates 1h
    pub fn update_1m(&mut self, close: f64, ts_ms: u64) {
        if close <= 0.0 {
            return;
        }

        self.update_tf_direct("1m", close, ts_ms);

        self.agg_count += 1;

        if self.agg_count % 5 == 0 {
            self.update_tf_direct("5m", close, ts_ms);
        }
        if self.agg_count % 15 == 0 {
            self.update_tf_direct("15m", close, ts_ms);
        }
        if self.agg_count % 60 == 0 {
            self.update_tf_direct("1h", close, ts_ms);
        }

        // Trim buffer
        self.agg_buffer.push(close);
        if self.agg_buffer.len() > 200 {
            self.agg_buffer.remove(0);
        }
    }

    /// Update a specific timeframe directly.
    pub fn update_tf(&mut self, tf: &str, close: f64, ts_ms: u64) {
        if close <= 0.0 {
            return;
        }
        self.update_tf_direct(tf, close, ts_ms);
    }

    fn update_tf_direct(&mut self, tf: &str, close: f64, ts_ms: u64) {
        let Some(state) = self.states.get_mut(tf) else {
            return;
        };

        state.closes.push_back(close);
        if state.closes.len() > 100 {
            state.closes.pop_front();
        }
        state.last_close = Some(close);
        state.last_update = Some(ts_ms);

        // Update EMAs
        let alpha_fast = 2.0 / (self.ema_fast_period as f64 + 1.0);
        let alpha_slow = 2.0 / (self.ema_slow_period as f64 + 1.0);

        match state.ema_fast {
            None => {
                state.ema_fast = Some(close);
                state.ema_slow = Some(close);
            }
            Some(prev_fast) => {
                let prev_slow = state.ema_slow.unwrap();
                state.ema_fast = Some(alpha_fast * close + (1.0 - alpha_fast) * prev_fast);
                state.ema_slow = Some(alpha_slow * close + (1.0 - alpha_slow) * prev_slow);
            }
        }

        // Invalidate cache
        self.cached_snapshot = None;
    }

    /// Warm up a timeframe from a slice of historical close prices.
    ///
    /// Processes prices in order (oldest first). Returns number of prices processed.
    pub fn warmup_tf(&mut self, tf: &str, closes: &[f64], ts_ms: u64) -> usize {
        let state = match self.states.get_mut(tf) {
            Some(s) => s,
            None => return 0,
        };

        state.closes.clear();
        state.ema_fast = None;
        state.ema_slow = None;

        let mut count = 0;
        for &close in closes {
            if close <= 0.0 {
                continue;
            }
            state.closes.push_back(close);
            if state.closes.len() > 100 {
                state.closes.pop_front();
            }
            state.last_close = Some(close);

            let alpha_fast = 2.0 / (self.ema_fast_period as f64 + 1.0);
            let alpha_slow = 2.0 / (self.ema_slow_period as f64 + 1.0);
            match state.ema_fast {
                None => {
                    state.ema_fast = Some(close);
                    state.ema_slow = Some(close);
                }
                Some(prev) => {
                    state.ema_fast = Some(alpha_fast * close + (1.0 - alpha_fast) * prev);
                    state.ema_slow =
                        Some(alpha_slow * close + (1.0 - alpha_slow) * state.ema_slow.unwrap());
                }
            }
            count += 1;
        }

        state.last_update = Some(ts_ms);
        self.cached_snapshot = None;
        count
    }

    // ── Computation ─────────────────────────────────────────────────────────

    /// Compute slope in basis points via linear regression on log-prices.
    fn compute_slope_bps(closes: &VecDeque<f64>, lookback: usize) -> Option<f64> {
        let n = closes.len();
        if n < 3 {
            return None;
        }

        let use_n = lookback.min(n);
        // Take last `use_n` prices
        let start = n - use_n;
        let prices: Vec<f64> = closes.iter().skip(start).copied().collect();

        // Log prices
        let log_prices: Vec<f64> = prices.iter().map(|&p| p.ln()).collect();
        let m = log_prices.len() as f64;

        let sum_x: f64 = (0..log_prices.len()).map(|i| i as f64).sum();
        let sum_y: f64 = log_prices.iter().sum();
        let sum_xy: f64 = log_prices
            .iter()
            .enumerate()
            .map(|(i, &y)| i as f64 * y)
            .sum();
        let sum_x2: f64 = (0..log_prices.len()).map(|i| (i as f64).powi(2)).sum();

        let denom = m * sum_x2 - sum_x * sum_x;
        if denom.abs() < 1e-10 {
            return Some(0.0);
        }

        let slope = (m * sum_xy - sum_x * sum_y) / denom;
        // Convert to bps per bar: ln(slope) ≈ fractional return, ×10000 for bps
        Some(slope * 10_000.0)
    }

    /// Compute bias for a single timeframe.
    fn compute_tf_bias(&self, tf: &str) -> TfBiasResult {
        let state = match self.states.get(tf) {
            Some(s) => s,
            None => {
                return TfBiasResult {
                    ready: false,
                    slope_bps: None,
                    ema_spread_bps: None,
                    bias_score: 0.5,
                    dir: Direction3::Neutral,
                    n_bars: 0,
                }
            }
        };

        if state.closes.len() < 5 {
            return TfBiasResult {
                ready: false,
                slope_bps: None,
                ema_spread_bps: None,
                bias_score: 0.5,
                dir: Direction3::Neutral,
                n_bars: state.closes.len(),
            };
        }

        // Slope
        let slope_bps = Self::compute_slope_bps(&state.closes, self.slope_lookback);

        // EMA spread
        let ema_spread_bps = match (state.ema_fast, state.ema_slow) {
            (Some(fast), Some(slow)) if slow > 0.0 => {
                Some((fast - slow) / slow * 10_000.0)
            }
            _ => None,
        };

        // Normalize (FIX-BIAS-002: empirically calibrated for crypto)
        let slope_norm = slope_bps
            .map(|s| (s / 3.0).clamp(-1.0, 1.0))
            .unwrap_or(0.0);
        let ema_norm = ema_spread_bps
            .map(|e| (e / 15.0).clamp(-1.0, 1.0))
            .unwrap_or(0.0);

        // Combined: weighted sum, mapped to [0.05, 0.95]
        let combined = self.slope_weight * slope_norm + self.ema_weight * ema_norm;
        let bias_score = (0.5 + combined * 0.45).clamp(0.05, 0.95);

        let dir = if bias_score > self.up_threshold {
            Direction3::Up
        } else if bias_score < self.down_threshold {
            Direction3::Down
        } else {
            Direction3::Neutral
        };

        TfBiasResult {
            ready: true,
            slope_bps: slope_bps.map(|s| (s * 100.0).round() / 100.0),
            ema_spread_bps: ema_spread_bps.map(|e| (e * 100.0).round() / 100.0),
            bias_score: (bias_score * 1000.0).round() / 1000.0,
            dir,
            n_bars: state.closes.len(),
        }
    }

    /// TAAPI directional string → numeric score.
    fn taapi_bias_to_score(bias_str: &str) -> f64 {
        match bias_str {
            "UP" => 0.7,
            "DOWN" => 0.3,
            _ => 0.5,
        }
    }

    // ── Snapshot ────────────────────────────────────────────────────────────

    /// Get current bias snapshot.
    ///
    /// Uses cached value if within the snapshot interval (default 10s) to
    /// avoid recomputing on every tick. Pass `force=true` to bypass cache.
    ///
    /// `taapi_context` is an optional map with keys like `"bias_1h"`, `"bias_15m"`,
    /// etc. — integrates external TAAPI-style signals into the bias score.
    pub fn snapshot(
        &mut self,
        force: bool,
        taapi_context: Option<&HashMap<String, String>>,
    ) -> BiasSnapshot {
        let ts_ms = Self::now_ms();

        // Use cache if fresh enough and no taapi (taapi changes externally)
        if !force && taapi_context.is_none() {
            if let (Some(last), Some(ref cached)) = (self.last_snapshot_ms, &self.cached_snapshot) {
                if ts_ms.wrapping_sub(last) < self.snapshot_interval_ms {
                    return cached.clone();
                }
            }
        }

        // Compute per-TF bias
        let mut tf_breakdown = HashMap::new();
        let mut own_weighted = 0.0;
        let mut own_total_weight = 0.0;

        for &(tf, weight) in TF_WEIGHTS {
            let tf_bias = self.compute_tf_bias(tf);
            if tf_bias.ready {
                own_weighted += tf_bias.bias_score * weight;
                own_total_weight += weight;
            }
            tf_breakdown.insert(tf.to_string(), tf_bias);
        }

        let own_score = if own_total_weight > 0.0 {
            own_weighted / own_total_weight
        } else {
            0.5
        };

        // ── TAAPI Integration ───────────────────────────────────────────
        let mut taapi_score = None;
        let mut taapi_weight = 0.0_f64;

        if let Some(ctx) = taapi_context {
            let mut taapi_weighted = 0.0;
            let mut taapi_total = 0.0;

            for &(tf, weight) in TF_WEIGHTS {
                let key = format!("bias_{tf}");
                if let Some(bias_str) = ctx.get(&key) {
                    let score = Self::taapi_bias_to_score(bias_str);
                    taapi_weighted += score * weight;
                    taapi_total += weight;
                }
            }

            if taapi_total > 0.0 {
                taapi_score = Some(taapi_weighted / taapi_total);
                taapi_weight = 0.5; // Give TAAPI 50% weight when available
            }
        }

        // ── Combined Score ──────────────────────────────────────────────
        let overall_score = match taapi_score {
            Some(ts) if taapi_weight > 0.0 => {
                own_score * (1.0 - taapi_weight) + ts * taapi_weight
            }
            _ => own_score,
        };

        let overall_dir = if overall_score > self.up_threshold {
            Direction3::Up
        } else if overall_score < self.down_threshold {
            Direction3::Down
        } else {
            Direction3::Neutral
        };

        let strength = ((overall_score - 0.5).abs() * 2.0).clamp(0.0, 1.0);

        let result = BiasSnapshot {
            dir: overall_dir,
            strength,
            bias_up_prob: overall_score,
            bias_down_prob: 1.0 - overall_score,
            tf_breakdown,
            last_update_ms: ts_ms,
        };

        // Cache (only without TAAPI to avoid stale hybrid data)
        if taapi_context.is_none() {
            self.cached_snapshot = Some(result.clone());
            self.last_snapshot_ms = Some(ts_ms);
        }

        result
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_bias_is_neutral() {
        let mut model = BiasModel::default();
        let snap = model.snapshot(false, None);
        assert_eq!(snap.dir, Direction3::Neutral);
        assert!(snap.strength < 0.1);
    }

    #[test]
    fn test_single_tf_warmup() {
        let mut model = BiasModel::default();
        // Prices trending up
        let prices: Vec<f64> = (0..30).map(|i| 100.0 + i as f64 * 0.5).collect();
        let count = model.warmup_tf("1m", &prices, 1000);
        assert_eq!(count, 30);

        let snap = model.snapshot(true, None);
        let tf = snap.tf_breakdown.get("1m").unwrap();
        assert!(tf.ready);
        assert!(tf.slope_bps.unwrap() > 0.0, "Should have positive slope");
    }

    #[test]
    fn test_downtrend_detected() {
        let mut model = BiasModel::default();
        let prices: Vec<f64> = (0..30).map(|i| 100.0 - i as f64 * 0.5).collect();
        model.warmup_tf("1h", &prices, 1000);

        let snap = model.snapshot(true, None);
        assert_eq!(snap.dir, Direction3::Down);
        assert!(snap.strength > 0.3, "Downtrend should have strength");
    }

    #[test]
    fn test_uptrend_detected() {
        let mut model = BiasModel::default();
        let prices: Vec<f64> = (0..30).map(|i| 100.0 + i as f64 * 1.0).collect();
        model.warmup_tf("1h", &prices, 1000);

        let snap = model.snapshot(true, None);
        assert_eq!(snap.dir, Direction3::Up);
        assert!(snap.bias_up_prob > 0.55);
    }

    #[test]
    fn test_sideways_stays_neutral() {
        let mut model = BiasModel::default();
        // Slightly noisy but flat prices
        let prices: Vec<f64> = vec![
            100.0, 100.1, 99.9, 100.0, 100.1, 99.9, 100.0, 100.1, 99.9, 100.0,
            100.1, 99.9, 100.0, 100.1, 99.9, 100.0, 100.1, 99.9, 100.0, 100.1,
            99.9, 100.0, 100.1, 99.9, 100.0, 100.1, 99.9, 100.0, 100.1, 99.9,
        ];
        model.warmup_tf("1h", &prices, 1000);

        let snap = model.snapshot(true, None);
        // Should be neutral or very weak
        assert!(snap.strength < 0.3 || snap.dir == Direction3::Neutral,
            "Sideways market should not produce strong directional bias, got strength={}", snap.strength);
    }

    #[test]
    fn test_update_1m_aggregates() {
        let mut model = BiasModel::default();
        // Feed 60 1-minute closes trending up
        for i in 0..60 {
            model.update_1m(100.0 + i as f64 * 0.1, i * 60_000);
        }

        let snap = model.snapshot(true, None);

        // 1m should have 60 bars
        let tf1 = snap.tf_breakdown.get("1m").unwrap();
        assert!(tf1.n_bars >= 60);

        // 5m should have bars (60/5 = 12 updates)
        let tf5 = snap.tf_breakdown.get("5m").unwrap();
        assert!(tf5.n_bars >= 12);

        // 15m should have bars (60/15 = 4 updates)
        let tf15 = snap.tf_breakdown.get("15m").unwrap();
        assert!(tf15.n_bars >= 4);

        // 1h should have 1 bar (60/60)
        let tf1h = snap.tf_breakdown.get("1h").unwrap();
        assert!(tf1h.n_bars >= 1);
    }

    #[test]
    fn test_bias_score_bounds() {
        let mut model = BiasModel::default();
        let prices: Vec<f64> = (0..30).map(|i| 100.0 + i as f64 * 0.2).collect();
        model.warmup_tf("1h", &prices, 1000);

        let snap = model.snapshot(true, None);
        assert!(snap.bias_up_prob >= 0.05 && snap.bias_up_prob <= 0.95);
        assert!((snap.bias_up_prob + snap.bias_down_prob - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_direction3_equality() {
        assert_eq!(Direction3::Up, Direction3::Up);
        assert_ne!(Direction3::Up, Direction3::Down);
        assert_ne!(Direction3::Neutral, Direction3::Up);
    }

    #[test]
    fn test_empty_prices_no_panic() {
        let mut model = BiasModel::default();
        let snap = model.snapshot(true, None);
        // Should return neutral with zero strength
        assert_eq!(snap.dir, Direction3::Neutral);
        assert!((snap.bias_up_prob - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_negative_prices_ignored() {
        let mut model = BiasModel::default();
        model.update_1m(-100.0, 1000);
        model.update_1m(50.0, 2000);
        model.update_1m(51.0, 3000);

        let snap = model.snapshot(true, None);
        // Should not have processed negative price
        let tf = snap.tf_breakdown.get("1m").unwrap();
        assert_eq!(tf.n_bars, 2); // Only two valid prices
    }

    #[test]
    fn test_snapshot_caching() {
        let mut model = BiasModel::default();
        model.warmup_tf("1h", &[100.0, 101.0, 102.0, 103.0, 104.0], 1000);

        let snap1 = model.snapshot(false, None);
        // Second call within interval should return cached
        let snap2 = model.snapshot(false, None);

        assert_eq!(snap1.bias_up_prob, snap2.bias_up_prob);
    }

    #[test]
    fn test_taapi_integration() {
        let mut model = BiasModel::default();
        let prices: Vec<f64> = (0..30).map(|_| 100.0).collect(); // Flat
        model.warmup_tf("1h", &prices, 1000);

        // Without TAAPI: neutral
        let snap_no_taapi = model.snapshot(true, None);
        assert_eq!(snap_no_taapi.dir, Direction3::Neutral);

        // With TAAPI UP signal on all TFs
        let mut taapi = HashMap::new();
        taapi.insert("bias_1h".to_string(), "UP".to_string());
        taapi.insert("bias_15m".to_string(), "UP".to_string());
        taapi.insert("bias_5m".to_string(), "UP".to_string());
        taapi.insert("bias_1m".to_string(), "UP".to_string());

        let snap_taapi = model.snapshot(true, Some(&taapi));
        // TAAPI UP should pull overall toward UP
        assert!(snap_taapi.bias_up_prob > snap_no_taapi.bias_up_prob);
    }

    #[test]
    fn test_higher_tf_has_more_weight() {
        let mut model = BiasModel::default();
        // 1h: downtrend, 1m: uptrend → overall should be DOWN (1h has 4× weight)
        let prices_down: Vec<f64> = (0..30).map(|i| 100.0 - i as f64 * 0.3).collect();
        model.warmup_tf("1h", &prices_down, 1000);
        let prices_up: Vec<f64> = (0..30).map(|i| 100.0 + i as f64 * 0.5).collect();
        model.warmup_tf("1m", &prices_up, 1000);

        let snap = model.snapshot(true, None);
        assert_eq!(snap.dir, Direction3::Down, "1h weight (0.4) > 1m weight (0.1)");
    }
}
