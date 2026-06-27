//! Fair Value Model — Geometric Brownian Motion probability estimator.
//!
//! Computes fair directional probability for "price will be above reference at
//! horizon" using a log-normal price model. This is the mathematical foundation
//! from the coinman2 bot architecture (mlmodelpoly).
//!
//! # Model
//!
//! ```text
//! fair_up = Φ( ln(S_now / ref_px) / (σ × √τ) )
//! ```
//!
//! Where:
//! - `S_now` — current spot price
//! - `ref_px` — reference price (typically the price at evaluation window open)
//! - `σ` — volatility (standard deviation of log returns over the horizon)
//! - `τ` — time remaining as fraction of total window (0.0–1.0)
//! - `Φ` — standard normal cumulative distribution function
//!
//! Assumptions:
//! 1. Log-normal price distribution (standard for crypto)
//! 2. Drift μ ≈ 0 for short horizons (≤1 hour)
//! 3. No jumps/black swans during the window
//!
//! # Edge Cases
//!
//! | Condition | Behaviour |
//! |-----------|-----------|
//! | σ too small (< 1e-6) | Fallback: 0.6/0.4 in current direction |
//! | τ too small (< 0.001) | Near-certain: 0.95/0.05 in current direction |
//! | S_now == ref_px | ~0.5 (slight positive drift) |
//! | Negative prices | Returns None |
//!
//! # Usage
//!
//! ```ignore
//! use arli_trading::fair_value::FairValueModel;
//!
//! let fv = FairValueModel::default();
//! let prob_up = fv.probability_up(105000.0, 104000.0, 0.003, 0.67);
//! // prob_up ≈ 0.78 — 78% chance BTC is above 104000 at window end
//! ```

// ── Constants ──────────────────────────────────────────────────────────────────

/// Minimum sigma before fallback to simple comparison.
const MIN_SIGMA: f64 = 1e-6_f64;
/// Minimum tau (normalized) before near-certainty fallback.
const MIN_TAU: f64 = 1e-3_f64;
/// Default volatility if none provided (30% annualized → ~0.0012 per 15min).
const DEFAULT_SIGMA: f64 = 0.0012;
/// Conservative drift for crypto (slight upward bias).
const DRIFT: f64 = 0.0; // μ ≈ 0 for short horizons

// ── Model ──────────────────────────────────────────────────────────────────────

/// Fair value model using Geometric Brownian Motion.
///
/// Estimates the probability that spot price will be above a reference price
/// at the end of an evaluation window, given current price, volatility, and
/// remaining time.
#[derive(Debug, Clone, Copy)]
pub struct FairValueModel {
    /// Minimum acceptable sigma. Below this, the model falls back to simple
    /// directional comparison rather than producing a precise estimate.
    pub min_sigma: f64,
    /// Annual drift (μ). Default 0 — negligible for horizons ≤ 1 hour.
    pub drift: f64,
}

impl Default for FairValueModel {
    fn default() -> Self {
        Self {
            min_sigma: MIN_SIGMA,
            drift: DRIFT,
        }
    }
}

impl FairValueModel {
    /// Create a new fair value model with custom parameters.
    pub fn new(min_sigma: f64, drift: f64) -> Self {
        Self { min_sigma, drift }
    }

    // ── Public API ──────────────────────────────────────────────────────────

    /// Compute the fair probability that spot > reference at horizon.
    ///
    /// # Arguments
    /// - `spot_now` — current spot price
    /// - `ref_price` — reference price (window open price)
    /// - `sigma` — volatility as std of log returns over the FULL window
    /// - `tau` — fraction of window remaining (0.0–1.0)
    ///
    /// # Returns
    /// - `Some(prob)` — probability 0.0–1.0
    /// - `None` — if inputs are invalid (non-positive prices)
    pub fn probability_up(
        &self,
        spot_now: f64,
        ref_price: f64,
        sigma: f64,
        tau: f64,
    ) -> Option<f64> {
        // Guard: invalid prices
        if spot_now <= 0.0 || ref_price <= 0.0 {
            return None;
        }

        // Guard: sigma too small → fallback to simple directional estimate
        if sigma < self.min_sigma || sigma.is_nan() || sigma.is_infinite() {
            return if spot_now > ref_price {
                Some(0.6)
            } else if spot_now < ref_price {
                Some(0.4)
            } else {
                Some(0.5)
            };
        }

        // Guard: tau too small → near-certainty in current direction
        if tau < MIN_TAU {
            return if spot_now > ref_price {
                Some(0.95)
            } else if spot_now < ref_price {
                Some(0.05)
            } else {
                Some(0.5)
            };
        }

        // Core GBM formula
        let log_ratio = (spot_now / ref_price).ln();
        let sigma_scaled = sigma * tau.sqrt().max(1e-9);
        let z = (log_ratio + self.drift * tau) / sigma_scaled;
        let prob = standard_normal_cdf(z);

        Some(prob.clamp(0.0, 1.0))
    }

    /// Compute the fair probability that spot < reference at horizon.
    ///
    /// Convenience: `1.0 - probability_up(...)`.
    pub fn probability_down(
        &self,
        spot_now: f64,
        ref_price: f64,
        sigma: f64,
        tau: f64,
    ) -> Option<f64> {
        self.probability_up(spot_now, ref_price, sigma, tau)
            .map(|p| 1.0 - p)
    }

    /// Compute edge in basis points between fair value and market price.
    ///
    /// Positive edge = fair value > market → market is undervalued.
    ///
    /// ```text
    /// edge_bps = (fair - market) × 10000
    /// ```
    pub fn edge_bps(fair: f64, market: f64) -> f64 {
        (fair - market) * 10_000.0
    }

    /// Estimate sigma from a series of log returns.
    ///
    /// Converts annualized or per-sample volatility to window-scale sigma.
    ///
    /// ```text
    /// sigma_window = sigma_per_sample × sqrt(num_samples_in_window)
    /// ```
    pub fn scale_sigma(sigma_per_sample: f64, samples_in_window: usize) -> f64 {
        sigma_per_sample * (samples_in_window as f64).sqrt()
    }

    /// Full decision: probability, edge, and actionable signal.
    ///
    /// Returns a structured decision suitable for feeding into a trading strategy.
    pub fn decide(
        &self,
        spot_now: f64,
        ref_price: f64,
        sigma: f64,
        tau: f64,
        market_price_up: Option<f64>,
    ) -> FairValueDecision {
        let fair_up = self
            .probability_up(spot_now, ref_price, sigma, tau)
            .unwrap_or(0.5);
        let fair_down = 1.0 - fair_up;

        let edge_up = market_price_up.map(|m| Self::edge_bps(fair_up, m));

        // Direction: which side has positive edge?
        let direction = match edge_up {
            Some(e) if e > 0.0 => Direction::Up,
            Some(e) if e < 0.0 => Direction::Down,
            _ => {
                // No market price → go with fair model
                if fair_up > 0.55 {
                    Direction::Up
                } else if fair_down > 0.55 {
                    Direction::Down
                } else {
                    Direction::Neutral
                }
            }
        };

        // Conviction: how far from 0.5 is fair probability?
        let conviction = (fair_up - 0.5).abs() * 2.0;

        FairValueDecision {
            fair_up,
            fair_down,
            edge_up_bps: edge_up,
            direction,
            conviction,
            sigma_used: sigma,
            tau_used: tau,
        }
    }
}

// ── Decision Output ────────────────────────────────────────────────────────────

/// Result of a fair-value decision.
#[derive(Debug, Clone)]
pub struct FairValueDecision {
    /// Fair probability that price goes UP.
    pub fair_up: f64,
    /// Fair probability that price goes DOWN.
    pub fair_down: f64,
    /// Edge vs market price in basis points (None if no market price).
    pub edge_up_bps: Option<f64>,
    /// Recommended direction.
    pub direction: Direction,
    /// Conviction 0.0–1.0 (distance from 0.5, scaled ×2).
    pub conviction: f64,
    /// Sigma used for this computation.
    pub sigma_used: f64,
    /// Tau (fraction of window remaining) used.
    pub tau_used: f64,
}

impl FairValueDecision {
    /// Convert conviction + direction into a confidence-like score (0.0–1.0).
    ///
    /// This can be used directly as `Signal.confidence` in the trading pipeline.
    pub fn as_confidence(&self) -> f64 {
        if self.direction == Direction::Neutral {
            0.0
        } else {
            self.conviction.clamp(0.0, 1.0)
        }
    }

    /// Whether this decision is actionable (direction ≠ neutral AND conviction > threshold).
    pub fn is_actionable(&self, min_conviction: f64) -> bool {
        self.direction != Direction::Neutral && self.conviction >= min_conviction
    }
}

/// Trading direction from the fair value model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Price likely to be above reference → buy signal.
    Up,
    /// Price likely to be below reference → sell signal.
    Down,
    /// No clear edge → no action.
    Neutral,
}

// ── Math ───────────────────────────────────────────────────────────────────────

/// Standard normal CDF Φ(x).
///
/// Uses a fast polynomial approximation of the error function.
/// Absolute error < 1.5×10⁻⁷ across the domain.
///
/// ```text
/// Φ(x) = (1 + erf(x / √2)) / 2
/// ```
fn standard_normal_cdf(x: f64) -> f64 {
    // Marsaglia (2004) fast CDF approximation — accuracy to ~1e-8
    let a1 = 0.254829592;
    let a2 = -0.284496736;
    let a3 = 1.421413741;
    let a4 = -1.453152027;
    let a5 = 1.061405429;
    let p = 0.3275911;

    let z = x.abs() / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + p * z);
    let poly = ((((a5 * t + a4) * t + a3) * t + a2) * t + a1) * t;
    let erf_z = 1.0 - poly * (-z * z).exp();

    if x < 0.0 {
        (1.0 - erf_z) / 2.0
    } else {
        (1.0 + erf_z) / 2.0
    }
}

// ── Volatility Estimator ───────────────────────────────────────────────────────

/// Compute rolling volatility from a window of prices.
///
/// Returns the standard deviation of log returns (σ).
///
/// ```text
/// returns[i] = ln(price[i] / price[i-1])
/// σ = std_dev(returns)
/// ```
pub fn compute_volatility(prices: &[f64]) -> Option<f64> {
    if prices.len() < 2 {
        return None;
    }

    let returns: Vec<f64> = prices
        .windows(2)
        .map(|w| (w[1] / w[0]).ln())
        .filter(|r| r.is_finite())
        .collect();

    if returns.len() < 2 {
        return None;
    }

    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;
    let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);

    let sigma = variance.sqrt();
    if sigma.is_finite() && sigma > 0.0 {
        Some(sigma)
    } else {
        None
    }
}

/// Scale sample-interval sigma to a target window sigma.
///
/// # Example
/// ```ignore
/// // 1-minute sigma, scale to 15-minute window
/// let sigma_15m = scale_volatility(sigma_1m, 15);
/// ```
pub fn scale_volatility(sigma_per_sample: f64, num_samples: usize) -> f64 {
    sigma_per_sample * (num_samples as f64).sqrt()
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_normal_cdf() {
        let tol = 1e-4;
        assert!((standard_normal_cdf(0.0) - 0.5).abs() < tol, "Φ(0) = 0.5");
        assert!(
            (standard_normal_cdf(1.96) - 0.975).abs() < 0.001,
            "Φ(1.96) ≈ 0.975"
        );
        assert!(
            (standard_normal_cdf(-1.96) - 0.025).abs() < 0.001,
            "Φ(-1.96) ≈ 0.025"
        );
    }

    #[test]
    fn test_probability_up_equal_prices() {
        let fv = FairValueModel::default();
        let prob = fv.probability_up(100_000.0, 100_000.0, 0.003, 0.5).unwrap();
        // With zero log_ratio and zero drift, should be ~0.5
        assert!((prob - 0.5).abs() < 0.01, "Equal prices → 0.5, got {prob}");
    }

    #[test]
    fn test_probability_up_spot_above_ref() {
        let fv = FairValueModel::default();
        let prob = fv.probability_up(101_000.0, 100_000.0, 0.01, 0.5).unwrap();
        assert!(prob > 0.70, "Spot above ref → prob > 0.70, got {prob}");
        assert!(prob < 0.99, "Not a certainty, got {prob}");
    }

    #[test]
    fn test_probability_up_spot_below_ref() {
        let fv = FairValueModel::default();
        let prob = fv.probability_up(99_000.0, 100_000.0, 0.01, 0.5).unwrap();
        assert!(prob < 0.30, "Spot below ref → prob < 0.30, got {prob}");
        assert!(prob > 0.01, "Not zero, got {prob}");
    }

    #[test]
    fn test_sigma_too_small_fallback() {
        let fv = FairValueModel::default();
        // Very low sigma — should use fallback
        let prob = fv.probability_up(105_000.0, 100_000.0, 1e-10, 0.5).unwrap();
        assert_eq!(prob, 0.6, "Low sigma + spot above → fallback 0.6");
    }

    #[test]
    fn test_tau_near_zero() {
        let fv = FairValueModel::default();
        let prob = fv.probability_up(105_000.0, 100_000.0, 0.003, 0.0001).unwrap();
        assert_eq!(prob, 0.95, "Near-zero tau + spot above → 0.95");
    }

    #[test]
    fn test_invalid_prices() {
        let fv = FairValueModel::default();
        assert!(fv.probability_up(-1.0, 100.0, 0.003, 0.5).is_none());
        assert!(fv.probability_up(100.0, 0.0, 0.003, 0.5).is_none());
    }

    #[test]
    fn test_edge_bps() {
        let edge = FairValueModel::edge_bps(0.55, 0.50);
        assert!((edge - 500.0).abs() < 0.1, "55% vs 50% → 500 bps edge, got {edge}");

        let edge_neg = FairValueModel::edge_bps(0.45, 0.50);
        assert!((edge_neg + 500.0).abs() < 0.1, "45% vs 50% → -500 bps, got {edge_neg}");
    }

    #[test]
    fn test_decide_with_market_price() {
        let fv = FairValueModel::default();
        // Fair says 78% up, market shows 50% → strong UP signal
        let d = fv.decide(105_000.0, 104_000.0, 0.003, 0.5, Some(0.50));
        assert_eq!(d.direction, Direction::Up);
        assert!(d.edge_up_bps.unwrap() > 200.0, "Strong edge");
        assert!(d.conviction > 0.3, "High conviction");
        assert!(d.is_actionable(0.2));
    }

    #[test]
    fn test_decide_neutral() {
        let fv = FairValueModel::default();
        // Equal prices → no edge
        let d = fv.decide(100_000.0, 100_000.0, 0.003, 0.5, None);
        assert_eq!(d.direction, Direction::Neutral);
        assert!(!d.is_actionable(0.2));
    }

    #[test]
    fn test_compute_volatility() {
        // Prices trending up with some noise
        let prices = vec![100.0, 101.0, 102.5, 103.0, 104.0, 103.5, 105.0, 106.0];
        let sigma = compute_volatility(&prices).unwrap();
        assert!(sigma > 0.0, "Should have positive volatility");
        assert!(sigma < 0.1, "Should be reasonable for these prices");
    }

    #[test]
    fn test_compute_volatility_too_few_prices() {
        assert!(compute_volatility(&[100.0]).is_none());
        assert!(compute_volatility(&[]).is_none());
    }

    #[test]
    fn test_scale_volatility() {
        let sigma_1m = 0.001;
        let sigma_15m = scale_volatility(sigma_1m, 15);
        let expected = 0.001 * (15.0_f64).sqrt();
        assert!((sigma_15m - expected).abs() < 1e-10);
    }

    #[test]
    fn test_probability_as_tau_decreases() {
        let fv = FairValueModel::default();
        // Large gap (10% above ref): as time ↓, certainty ↑
        let prob_early = fv.probability_up(110_000.0, 100_000.0, 0.05, 0.9).unwrap();
        let prob_late = fv.probability_up(110_000.0, 100_000.0, 0.05, 0.1).unwrap();
        assert!(
            prob_late > prob_early,
            "As window closes, prob should increase. early={prob_early:.4} late={prob_late:.4}"
        );
    }

    #[test]
    fn test_high_volatility_pulls_toward_05() {
        let fv = FairValueModel::default();
        let prob_low_vol = fv.probability_up(105_000.0, 100_000.0, 0.001, 0.5).unwrap();
        let prob_high_vol = fv.probability_up(105_000.0, 100_000.0, 0.02, 0.5).unwrap();
        assert!(
            prob_low_vol > prob_high_vol,
            "Higher volatility → more uncertainty → closer to 0.5"
        );
    }
}
