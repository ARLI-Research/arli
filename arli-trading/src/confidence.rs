//! Confidence Engine — systematic multi-factor confidence scoring.
//!
//! Replaces ad-hoc confidence computation in trading strategies with a
//! structured 4-component model from the coinman2 bot architecture
//! (mlmodelpoly `confidence.py`).
//!
//! # Components
//!
//! | Component  | Weight | What it measures |
//! |-----------|--------|------------------|
//! | Net Edge  | 0.35   | How much edge after spread/buffer (sigmoid) |
//! | Agreement | 0.30   | Do fair_fast, fair_smooth, and bias all agree? |
//! | Events    | 0.20   | Are spike/dip triggers supporting the trade? |
//! | Quality   | 0.15   | Data health (OK / DEGRADED / BAD) |
//!
//! # Output
//!
//! - `confidence`: 0.0–1.0 numeric score
//! - `level`: HIGH (≥0.7) / MED (≥0.3) / LOW (<0.3)
//! - `reasons`: positive factors contributing to confidence
//! - `penalties`: negative factors reducing confidence
//!
//! # Usage
//!
//! ## Standalone
//!
//! ```ignore
//! use arli_trading::confidence::{ConfidenceEngine, CandidateSide};
//!
//! let engine = ConfidenceEngine::default();
//! let result = engine.evaluate(ConfidenceInput {
//!     candidate: CandidateSide::Up,
//!     fair_fast_up: Some(0.58),
//!     fair_fast_down: Some(0.42),
//!     fair_smooth_up: Some(0.55),
//!     fair_smooth_down: Some(0.45),
//!     bias_dir: Some("UP"),
//!     bias_strength: 0.6,
//!     net_edge_bps: 150.0,
//!     triggers: TriggerSet::default(),
//!     quality: DataQuality::Ok,
//!     ..Default::default()
//! });
//! // result.confidence → 0.72 (HIGH)
//! ```
//!
//! ## With FairValueModel + BiasModel
//!
//! ```ignore
//! use arli_trading::confidence::{ConfidenceEngine, ConfidenceInput, CandidateSide, DataQuality};
//! use arli_trading::fair_value::FairValueModel;
//! use arli_trading::bias_model::BiasModel;
//!
//! let fv = FairValueModel::default();
//! let mut bias = BiasModel::default();
//! let engine = ConfidenceEngine::default();
//!
//! // Feed prices into bias model, get snapshot
//! bias.update_1m(104500.0, now_ms);
//! let bias_snap = bias.snapshot(None);
//!
//! // Fair value: spot=104500, ref=104000, sigma=0.003, 67% window remaining
//! let fair_up = fv.probability_up(104500.0, 104000.0, 0.003, 0.67).unwrap_or(0.5);
//!
//! // Wire into confidence engine
//! let input = ConfidenceInput::from_fair_bias(
//!     CandidateSide::Up,
//!     fair_up,
//!     &bias_snap,
//!     Some(150.0), // net_edge_bps
//!     TriggerSet::default(),
//!     DataQuality::Ok,
//! );
//! let result = engine.evaluate(&input);
//! // result.confidence → model-driven score
//! ```

/// Which side the strategy wants to trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CandidateSide {
    #[default]
    Up,
    Down,
}

/// Data quality level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DataQuality {
    /// All systems nominal.
    #[default]
    Ok,
    /// Some data sources stale but trading still possible.
    Degraded,
    /// Critical data missing — should not trade.
    Bad,
}

/// Event triggers supporting or opposing a trade.
#[derive(Debug, Clone, Default)]
pub struct TriggerSet {
    /// Spot price spiked up recently.
    pub up_spike: bool,
    /// Spot price spiked down recently.
    pub down_spike: bool,
    /// Polymarket UP token dipped (buy opportunity for UP).
    pub up_dip: bool,
    /// Polymarket DOWN token dipped (buy opportunity for DOWN).
    pub down_dip: bool,
    /// Counter-trend signal: reversal toward DOWN.
    pub countertrend_down: bool,
    /// Counter-trend signal: reversal toward UP.
    pub countertrend_up: bool,
    /// RSI indicates overheated (overbought/oversold).
    pub overheated: bool,
}

/// Inputs to the confidence engine.
#[derive(Debug, Clone, Default)]
pub struct ConfidenceInput {
    /// Which side we're evaluating.
    pub candidate: CandidateSide,
    /// Fair value UP probability (fast model).
    pub fair_fast_up: Option<f64>,
    /// Fair value DOWN probability (fast model).
    pub fair_fast_down: Option<f64>,
    /// Fair value UP probability (smooth model).
    pub fair_smooth_up: Option<f64>,
    /// Fair value DOWN probability (smooth model).
    pub fair_smooth_down: Option<f64>,
    /// Bias direction: "UP" / "DOWN" / "NEUTRAL".
    pub bias_dir: Option<&'static str>,
    /// Bias strength 0.0–1.0.
    pub bias_strength: f64,
    /// Net edge in basis points.
    pub net_edge_bps: Option<f64>,
    /// Event triggers active.
    pub triggers: TriggerSet,
    /// Data quality mode.
    pub quality: DataQuality,
    /// TAAPI alignment score (0–100), bonus for high values.
    pub taapi_alignment: Option<u32>,
}

impl ConfidenceInput {
    /// Construct from FairValueModel probability + BiasModel snapshot.
    ///
    /// This is the primary integration point: feed mathematical model outputs
    /// into the structured confidence engine. Strategies call this instead of
    /// manually populating every field.
    ///
    /// # Arguments
    /// - `candidate` — which direction the strategy wants to trade
    /// - `fair_up` — fair probability of UP (from [`FairValueModel::probability_up`])
    /// - `bias` — bias snapshot (from [`BiasModel::snapshot`])
    /// - `net_edge_bps` — edge after spread/buffer in basis points
    /// - `triggers` — active event triggers (spikes, dips, countertrends)
    /// - `quality` — data quality assessment
    ///
    /// # Fair value
    ///
    /// `fair_up` sets both `fair_fast_up` and `fair_smooth_up`. For a dual-model
    /// setup (fast + smooth volatility estimates), construct `ConfidenceInput`
    /// manually.
    pub fn from_fair_bias(
        candidate: CandidateSide,
        fair_up: f64,
        bias: &crate::bias_model::BiasSnapshot,
        net_edge_bps: Option<f64>,
        triggers: TriggerSet,
        quality: DataQuality,
    ) -> Self {
        use crate::bias_model::Direction3;

        let fair_down = 1.0 - fair_up;
        let bias_dir: Option<&'static str> = match bias.dir {
            Direction3::Up => Some("UP"),
            Direction3::Down => Some("DOWN"),
            Direction3::Neutral => None,
        };

        ConfidenceInput {
            candidate,
            fair_fast_up: Some(fair_up),
            fair_fast_down: Some(fair_down),
            fair_smooth_up: Some(fair_up),
            fair_smooth_down: Some(fair_down),
            bias_dir,
            bias_strength: bias.strength,
            net_edge_bps,
            triggers,
            quality,
            ..Default::default()
        }
    }
}

/// Output of confidence evaluation.
#[derive(Debug, Clone)]
pub struct ConfidenceResult {
    /// Confidence score 0.0–1.0.
    pub confidence: f64,
    /// Categorical level.
    pub level: ConfidenceLevel,
    /// Factors increasing confidence.
    pub reasons: Vec<&'static str>,
    /// Factors decreasing confidence.
    pub penalties: Vec<&'static str>,
    /// Component breakdown.
    pub components: ConfidenceComponents,
}

/// Confidence level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfidenceLevel {
    High,
    Medium,
    Low,
}

/// Per-component confidence breakdown.
#[derive(Debug, Clone, Default)]
pub struct ConfidenceComponents {
    pub net_edge_score: f64,
    pub agreement_score: f64,
    pub events_score: f64,
    pub quality_score: f64,
    pub taapi_bonus: f64,
}

// ── Default Weights ────────────────────────────────────────────────────────────

const DEFAULT_W_NET_EDGE: f64 = 0.35;
const DEFAULT_W_AGREEMENT: f64 = 0.30;
const DEFAULT_W_EVENTS: f64 = 0.20;
const DEFAULT_W_QUALITY: f64 = 0.15;

const DEFAULT_LOW_THRESHOLD: f64 = 0.3;
const DEFAULT_HIGH_THRESHOLD: f64 = 0.7;

/// Sigmoid scale for net edge (bps). At 200 bps, sigmoid ≈ 0.73.
const NET_EDGE_SCALE: f64 = 200.0;

// ── Engine ─────────────────────────────────────────────────────────────────────

/// Confidence scoring engine with configurable weights and thresholds.
#[derive(Debug, Clone)]
pub struct ConfidenceEngine {
    w_net_edge: f64,
    w_agreement: f64,
    w_events: f64,
    w_quality: f64,
    low_threshold: f64,
    high_threshold: f64,
}

impl Default for ConfidenceEngine {
    fn default() -> Self {
        Self {
            w_net_edge: DEFAULT_W_NET_EDGE,
            w_agreement: DEFAULT_W_AGREEMENT,
            w_events: DEFAULT_W_EVENTS,
            w_quality: DEFAULT_W_QUALITY,
            low_threshold: DEFAULT_LOW_THRESHOLD,
            high_threshold: DEFAULT_HIGH_THRESHOLD,
        }
    }
}

impl ConfidenceEngine {
    /// Create with custom weights.
    pub fn new(
        w_net_edge: f64,
        w_agreement: f64,
        w_events: f64,
        w_quality: f64,
        low_threshold: f64,
        high_threshold: f64,
    ) -> Self {
        Self {
            w_net_edge,
            w_agreement,
            w_events,
            w_quality,
            low_threshold,
            high_threshold,
        }
    }

    /// Evaluate confidence for a candidate trade direction.
    ///
    /// Returns `ConfidenceResult` with score, level, reasons, and component breakdown.
    pub fn evaluate(&self, input: &ConfidenceInput) -> ConfidenceResult {
        let mut reasons: Vec<&'static str> = Vec::new();
        let mut penalties: Vec<&'static str> = Vec::new();

        // ── 1. Net Edge Score ──────────────────────────────────────────
        let net_edge_score = match input.net_edge_bps {
            Some(bps) if bps > 0.0 => {
                let s = sigmoid(bps, NET_EDGE_SCALE);
                reasons.push("positive_edge");
                s
            }
            Some(bps) if bps < 0.0 => {
                penalties.push("negative_edge");
                sigmoid(bps, NET_EDGE_SCALE)
            }
            _ => {
                penalties.push("no_edge");
                0.5
            }
        };

        // ── 2. Agreement Score ─────────────────────────────────────────
        let mut agreement_points = 0.0_f64;
        let agreement_max: f64 = 3.0;

        // Check fair_fast alignment
        if let (Some(fu), Some(fd)) = (input.fair_fast_up, input.fair_fast_down) {
            match input.candidate {
                CandidateSide::Up if fu > 0.5 => {
                    agreement_points += 1.0;
                    reasons.push("fair_fast_aligned");
                }
                CandidateSide::Down if fd > 0.5 => {
                    agreement_points += 1.0;
                    reasons.push("fair_fast_aligned");
                }
                CandidateSide::Up => penalties.push("fair_fast_against"),
                CandidateSide::Down => penalties.push("fair_fast_against"),
            }
        }

        // Check fair_smooth alignment
        if let (Some(su), Some(sd)) = (input.fair_smooth_up, input.fair_smooth_down) {
            match input.candidate {
                CandidateSide::Up if su > 0.5 => {
                    agreement_points += 1.0;
                    reasons.push("fair_smooth_aligned");
                }
                CandidateSide::Down if sd > 0.5 => {
                    agreement_points += 1.0;
                    reasons.push("fair_smooth_aligned");
                }
                CandidateSide::Up => penalties.push("fair_smooth_against"),
                CandidateSide::Down => penalties.push("fair_smooth_against"),
            }
        }

        // Check bias alignment
        if let Some(dir) = input.bias_dir {
            let strength_bonus = (input.bias_strength.max(0.0)).min(1.0);
            match (input.candidate, dir) {
                (CandidateSide::Up, "UP") => {
                    agreement_points += 1.0 * (strength_bonus + 0.5).min(1.5);
                    reasons.push("bias_aligned");
                }
                (CandidateSide::Down, "DOWN") => {
                    agreement_points += 1.0 * (strength_bonus + 0.5).min(1.5);
                    reasons.push("bias_aligned");
                }
                (CandidateSide::Up, "DOWN") => {
                    agreement_points -= 0.5 * strength_bonus;
                    penalties.push("bias_against");
                }
                (CandidateSide::Down, "UP") => {
                    agreement_points -= 0.5 * strength_bonus;
                    penalties.push("bias_against");
                }
                _ => {}
            }
        }

        let agreement_score = (agreement_points / agreement_max).clamp(0.0, 1.0);

        // ── 3. Events Score ────────────────────────────────────────────
        let mut events_score: f64 = 0.5; // Neutral start

        match input.candidate {
            CandidateSide::Up => {
                if input.triggers.down_spike {
                    events_score += 0.3;
                    reasons.push("down_spike_trigger");
                }
                if input.triggers.up_dip {
                    events_score += 0.2;
                    reasons.push("up_dip_trigger");
                }
                if input.triggers.up_spike {
                    events_score -= 0.2;
                    penalties.push("up_spike_late");
                }
                if input.triggers.countertrend_up {
                    events_score += 0.25;
                    reasons.push("countertrend_up");
                }
            }
            CandidateSide::Down => {
                if input.triggers.up_spike {
                    events_score += 0.3;
                    reasons.push("up_spike_trigger");
                }
                if input.triggers.down_dip {
                    events_score += 0.2;
                    reasons.push("down_dip_trigger");
                }
                if input.triggers.down_spike {
                    events_score -= 0.2;
                    penalties.push("down_spike_late");
                }
                if input.triggers.countertrend_down {
                    events_score += 0.25;
                    reasons.push("countertrend_down");
                }
            }
        }

        if input.triggers.overheated {
            events_score -= 0.1;
            penalties.push("overheated");
        }

        let events_score = events_score.clamp(0.0, 1.0);

        // ── 4. Quality Score ───────────────────────────────────────────
        let quality_score = match input.quality {
            DataQuality::Ok => {
                reasons.push("data_quality_ok");
                1.0
            }
            DataQuality::Degraded => {
                penalties.push("data_degraded");
                0.6
            }
            DataQuality::Bad => {
                penalties.push("data_bad");
                0.2
            }
        };

        // ── 5. TAAPI Alignment Bonus ───────────────────────────────────
        let taapi_bonus = match input.taapi_alignment {
            Some(a) if a >= 70 => {
                let bonus = 0.05 + (a as f64 - 70.0) / 300.0; // +0.05 to +0.1
                reasons.push("taapi_aligned");
                bonus
            }
            Some(a) if a >= 50 => {
                reasons.push("taapi_moderate");
                0.02
            }
            Some(a) if a < 30 => {
                penalties.push("taapi_misaligned");
                -0.05
            }
            _ => 0.0,
        };

        // ── 6. Combined Score ──────────────────────────────────────────
        let total_w = self.w_net_edge + self.w_agreement + self.w_events + self.w_quality;
        let mut confidence = (self.w_net_edge * net_edge_score
            + self.w_agreement * agreement_score
            + self.w_events * events_score
            + self.w_quality * quality_score)
            / total_w;

        confidence += taapi_bonus;
        confidence = confidence.clamp(0.0, 1.0);

        let level = if confidence >= self.high_threshold {
            ConfidenceLevel::High
        } else if confidence >= self.low_threshold {
            ConfidenceLevel::Medium
        } else {
            ConfidenceLevel::Low
        };

        ConfidenceResult {
            confidence: (confidence * 1000.0).round() / 1000.0,
            level,
            reasons,
            penalties,
            components: ConfidenceComponents {
                net_edge_score: (net_edge_score * 1000.0).round() / 1000.0,
                agreement_score: (agreement_score * 1000.0).round() / 1000.0,
                events_score: (events_score * 1000.0).round() / 1000.0,
                quality_score: (quality_score * 1000.0).round() / 1000.0,
                taapi_bonus: (taapi_bonus * 1000.0).round() / 1000.0,
            },
        }
    }
}

// ── Helper ─────────────────────────────────────────────────────────────────────

/// Sigmoid: 1 / (1 + exp(−x / scale)).
fn sigmoid(x: f64, scale: f64) -> f64 {
    if x > 600.0 {
        1.0
    } else if x < -600.0 {
        0.0
    } else {
        1.0 / (1.0 + (-x / scale).exp())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input() -> ConfidenceInput {
        ConfidenceInput {
            candidate: CandidateSide::Up,
            fair_fast_up: Some(0.58),
            fair_fast_down: Some(0.42),
            fair_smooth_up: Some(0.55),
            fair_smooth_down: Some(0.45),
            bias_dir: Some("UP"),
            bias_strength: 0.6,
            net_edge_bps: Some(200.0),
            triggers: TriggerSet::default(),
            quality: DataQuality::Ok,
            taapi_alignment: Some(75),
        }
    }

    /// Baseline confidence score for the default input.
    fn base_input_confidence() -> f64 {
        ConfidenceEngine::default().evaluate(&base_input()).confidence
    }

    #[test]
    fn test_high_confidence_when_all_aligned() {
        let engine = ConfidenceEngine::default();
        let input = base_input();
        let result = engine.evaluate(&input);
        assert!(result.confidence >= 0.7, "All signals aligned → HIGH confidence, got {}", result.confidence);
        assert_eq!(result.level, ConfidenceLevel::High);
    }

    #[test]
    fn test_low_confidence_when_nothing_aligned() {
        let engine = ConfidenceEngine::default();
        let input = ConfidenceInput {
            candidate: CandidateSide::Up,
            fair_fast_up: Some(0.42),
            fair_fast_down: Some(0.58),
            fair_smooth_up: Some(0.40),
            fair_smooth_down: Some(0.60),
            bias_dir: Some("DOWN"),
            bias_strength: 0.7,
            net_edge_bps: Some(-100.0),
            quality: DataQuality::Degraded,
            ..Default::default()
        };
        let result = engine.evaluate(&input);
        assert!(result.confidence < 0.35,
            "Everything against → LOW confidence, got {}", result.confidence);
        assert_eq!(result.level, ConfidenceLevel::Medium);
    }

    #[test]
    fn test_bias_against_penalizes() {
        let engine = ConfidenceEngine::default();
        let mut input = base_input();
        input.bias_dir = Some("DOWN"); // Bias against our UP candidate

        let result = engine.evaluate(&input);
        assert!(result.penalties.contains(&"bias_against"));
        assert!(result.confidence < 0.75,
            "Bias against should reduce confidence, got {}", result.confidence);
    }

    #[test]
    fn test_bad_quality_reduces_confidence() {
        let engine = ConfidenceEngine::default();
        let mut input = base_input();
        input.quality = DataQuality::Bad;

        let result = engine.evaluate(&input);
        assert!(result.confidence < base_input_confidence(),
            "BAD quality should reduce confidence vs base, got {}", result.confidence);
        assert!(result.penalties.contains(&"data_bad"));
    }

    #[test]
    fn test_empty_input_returns_medium() {
        let engine = ConfidenceEngine::default();
        let input = ConfidenceInput {
            candidate: CandidateSide::Up,
            ..Default::default()
        };
        let result = engine.evaluate(&input);
        assert_eq!(result.level, ConfidenceLevel::Medium,
            "Empty input → MEDIUM (50% baseline), got {:?}", result.level);
    }

    #[test]
    fn test_down_candidate_mirrors_up() {
        let engine = ConfidenceEngine::default();
        let input = ConfidenceInput {
            candidate: CandidateSide::Down,
            fair_fast_up: Some(0.42),
            fair_fast_down: Some(0.58),
            fair_smooth_up: Some(0.45),
            fair_smooth_down: Some(0.55),
            bias_dir: Some("DOWN"),
            bias_strength: 0.6,
            net_edge_bps: Some(200.0),
            quality: DataQuality::Ok,
            ..Default::default()
        };
        let result = engine.evaluate(&input);
        assert!(result.confidence >= 0.7,
            "DOWN candidate with aligned signals → HIGH, got {}", result.confidence);
    }

    #[test]
    fn test_spike_triggers_affect_events_score() {
        let engine = ConfidenceEngine::default();

        // UP candidate with down_spike (good) vs without
        let mut with_spike = base_input();
        with_spike.triggers.down_spike = true;

        let mut no_spike = base_input();
        no_spike.triggers.down_spike = false;

        let res_with = engine.evaluate(&with_spike);
        let res_no = engine.evaluate(&no_spike);

        assert!(res_with.confidence > res_no.confidence,
            "Down spike should boost UP confidence. with={} > no={}",
            res_with.confidence, res_no.confidence);
    }

    #[test]
    fn test_overheated_penalizes() {
        let engine = ConfidenceEngine::default();
        let mut input = base_input();
        input.triggers.overheated = true;

        let result = engine.evaluate(&input);
        assert!(result.penalties.contains(&"overheated"));
    }

    #[test]
    fn test_sigmoid_symmetry() {
        assert!((sigmoid(0.0, 200.0) - 0.5).abs() < 1e-9);
        assert!(sigmoid(500.0, 200.0) > 0.9);
        assert!(sigmoid(-500.0, 200.0) < 0.1);
    }

    #[test]
    fn test_taapi_alignment_bonus() {
        let engine = ConfidenceEngine::default();

        let mut with_taapi = base_input();
        with_taapi.taapi_alignment = Some(90);

        let mut no_taapi = base_input();
        no_taapi.taapi_alignment = None;

        let res_with = engine.evaluate(&with_taapi);
        let res_no = engine.evaluate(&no_taapi);

        assert!(res_with.confidence > res_no.confidence,
            "TAAPI alignment should boost confidence. with={} > no={}",
            res_with.confidence, res_no.confidence);
    }

    /// Integration test: ConfidenceInput::from_fair_bias() with BiasSnapshot.
    #[test]
    fn test_from_fair_bias_wires_bias_correctly() {
        use crate::bias_model::{BiasSnapshot, Direction3, TfBiasResult};
        use std::collections::HashMap;

        let bias = BiasSnapshot {
            dir: Direction3::Up,
            strength: 0.7,
            bias_up_prob: 0.72,
            bias_down_prob: 0.28,
            tf_breakdown: HashMap::new(),
            last_update_ms: 1000,
        };

        let input = ConfidenceInput::from_fair_bias(
            CandidateSide::Up,
            0.65, // fair_up
            &bias,
            Some(120.0), // net_edge_bps
            TriggerSet::default(),
            DataQuality::Ok,
        );

        assert_eq!(input.candidate, CandidateSide::Up);
        assert!((input.fair_fast_up.unwrap() - 0.65).abs() < 1e-9);
        assert!((input.fair_fast_down.unwrap() - 0.35).abs() < 1e-9);
        assert_eq!(input.bias_dir, Some("UP"));
        assert!((input.bias_strength - 0.7).abs() < 1e-9);
        assert_eq!(input.net_edge_bps, Some(120.0));
        assert!(matches!(input.quality, DataQuality::Ok));

        // Evaluate through the engine — should produce HIGH confidence
        let engine = ConfidenceEngine::default();
        let result = engine.evaluate(&input);
        assert!(result.confidence >= 0.7,
            "Fair+bias aligned UP should be HIGH, got {}",
            result.confidence);
    }

    /// from_fair_bias with NEUTRAL bias — no bias penalty or bonus.
    #[test]
    fn test_from_fair_bias_neutral_bias() {
        use crate::bias_model::{BiasSnapshot, Direction3};
        use std::collections::HashMap;

        let bias = BiasSnapshot {
            dir: Direction3::Neutral,
            strength: 0.0,
            bias_up_prob: 0.5,
            bias_down_prob: 0.5,
            tf_breakdown: HashMap::new(),
            last_update_ms: 1000,
        };

        let input = ConfidenceInput::from_fair_bias(
            CandidateSide::Up,
            0.55,
            &bias,
            Some(50.0),
            TriggerSet::default(),
            DataQuality::Ok,
        );

        assert_eq!(input.bias_dir, None);
        assert!((input.bias_strength - 0.0).abs() < 1e-9);

        let result = ConfidenceEngine::default().evaluate(&input);
        // Neutral bias + modest edge → MEDIUM confidence
        assert!(result.confidence >= 0.3 && result.confidence < 0.7,
            "Neutral bias + modest edge → MEDIUM, got {}",
            result.confidence);
    }
}
