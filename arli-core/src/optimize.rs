//! Auto-optimization — DSPy-like prompt and strategy tuning.
//!
//! Provides declarative optimization: describe WHAT to optimize (metric,
//! parameters, constraints) and the optimizer finds the best configuration.
//!
//! ## Components
//!
//! - **PromptOptimizer**: Iterates prompt variations, scores them against
//!   a metric function, returns the best performer.
//! - **StrategyOptimizer**: Grid search over parameter space with
//!   configurable scoring (e.g., Sharpe ratio, win rate, max drawdown).
//! - **AutoFewShot**: Automatically selects the most effective few-shot
//!   examples from execution history based on similarity + performance.
//!
//! ## Usage
//!
//! ```ignore
//! use arli_core::optimize::{PromptOptimizer, PromptCandidate};
//!
//! let mut opt = PromptOptimizer::new("Trading advisor");
//! opt.add_candidate(PromptCandidate::new("v1", "Be aggressive"));
//! opt.add_candidate(PromptCandidate::new("v2", "Be conservative"));
//!
//! let best = opt.optimize(|candidate, test_inputs| {
//!     // Run agent with this prompt, measure results
//!     score_responses(candidate, test_inputs)
//! }).await?;
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// PROMPT OPTIMIZER
// ============================================================================

/// A prompt candidate — one system prompt variant to test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCandidate {
    /// Human-readable name for this variant
    pub name: String,
    /// The system prompt text
    pub system_prompt: String,
    /// Optional: few-shot examples to prepend
    #[serde(default)]
    pub few_shot_examples: Vec<FewShotExample>,
    /// Score assigned after evaluation (higher = better)
    #[serde(default)]
    pub score: f64,
    /// Number of evaluations run
    #[serde(default)]
    pub evaluations: u32,
    /// Arbitrary metadata
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl PromptCandidate {
    pub fn new(name: impl Into<String>, system_prompt: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            system_prompt: system_prompt.into(),
            few_shot_examples: Vec::new(),
            score: 0.0,
            evaluations: 0,
            metadata: HashMap::new(),
        }
    }

    /// Add a few-shot example to this candidate.
    pub fn with_example(mut self, example: FewShotExample) -> Self {
        self.few_shot_examples.push(example);
        self
    }

    /// Combine all text for the full prompt context.
    pub fn full_prompt(&self) -> String {
        if self.few_shot_examples.is_empty() {
            return self.system_prompt.clone();
        }

        let mut out = String::new();
        for ex in &self.few_shot_examples {
            out.push_str(&format!(
                "Example:\nInput: {}\nOutput: {}\n\n",
                ex.input, ex.output
            ));
        }
        out.push_str(&self.system_prompt);
        out
    }
}

/// A single few-shot example (input → expected output).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FewShotExample {
    pub input: String,
    pub output: String,
    /// How well this example performed (0.0–1.0)
    #[serde(default)]
    pub quality: f64,
    /// When this example was collected (Unix timestamp)
    #[serde(default)]
    pub timestamp: u64,
    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,
}

impl FewShotExample {
    pub fn new(input: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            output: output.into(),
            quality: 0.5,
            timestamp: chrono::Utc::now().timestamp() as u64,
            tags: Vec::new(),
        }
    }

    pub fn with_quality(mut self, q: f64) -> Self {
        self.quality = q.clamp(0.0, 1.0);
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }
}

/// Prompt optimizer — evaluates candidates against a scoring function.
pub struct PromptOptimizer {
    /// Human-readable task name
    pub task: String,
    /// All candidates to evaluate
    pub candidates: Vec<PromptCandidate>,
    /// Optimization history
    pub history: Vec<OptimizationStep>,
    /// Minimum evaluations per candidate before picking winner
    pub min_evaluations: u32,
}

impl PromptOptimizer {
    pub fn new(task: impl Into<String>) -> Self {
        Self {
            task: task.into(),
            candidates: Vec::new(),
            history: Vec::new(),
            min_evaluations: 3,
        }
    }

    /// Add a candidate to the pool.
    pub fn add_candidate(&mut self, candidate: PromptCandidate) {
        self.candidates.push(candidate);
    }

    /// Run the optimization loop.
    ///
    /// The `scorer` function is called for each candidate-test_input pair.
    /// It should return a score (higher = better).
    ///
    /// Returns a reference to the best candidate (by average score).
    pub async fn optimize<F, Fut>(
        &mut self,
        test_inputs: &[String],
        scorer: F,
    ) -> Option<&PromptCandidate>
    where
        F: Fn(&PromptCandidate, &str) -> Fut,
        Fut: std::future::Future<Output = f64>,
    {
        if self.candidates.is_empty() {
            return None;
        }

        tracing::info!(
            "PromptOptimizer[{}]: {} candidates, {} test inputs, min_evals={}",
            self.task,
            self.candidates.len(),
            test_inputs.len(),
            self.min_evaluations,
        );

        // Evaluate each candidate against each test input
        for round in 0..self.min_evaluations {
            for (ci, candidate) in self.candidates.iter_mut().enumerate() {
                let mut round_score = 0.0;
                let mut count = 0;

                for input in test_inputs {
                    let s = scorer(candidate, input).await;
                    round_score += s;
                    count += 1;
                }

                let avg = if count > 0 { round_score / count as f64 } else { 0.0 };

                // Update running average
                candidate.score =
                    (candidate.score * candidate.evaluations as f64 + avg)
                        / (candidate.evaluations + 1) as f64;
                candidate.evaluations += 1;

                let step = OptimizationStep {
                    candidate_name: candidate.name.clone(),
                    round,
                    candidate_index: ci,
                    avg_score: avg,
                    cumulative_score: candidate.score,
                };

                tracing::debug!(
                    "  [{}/{}] {}: round={:.3} cumulative={:.3}",
                    round + 1,
                    self.min_evaluations,
                    candidate.name,
                    avg,
                    candidate.score,
                );

                self.history.push(step);
            }
        }

        // Pick winner — highest cumulative score
        let best = self.candidates.iter().max_by(|a, b| {
            a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal)
        })?;

        tracing::info!(
            "PromptOptimizer[{}]: winner = '{}' (score={:.3}, evals={})",
            self.task,
            best.name,
            best.score,
            best.evaluations,
        );

        Some(best)
    }

    /// Get the best candidate without re-running optimization.
    pub fn best(&self) -> Option<&PromptCandidate> {
        self.candidates.iter().max_by(|a, b| {
            a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal)
        })
    }
}

/// A single optimization step — recorded for analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationStep {
    pub candidate_name: String,
    pub round: u32,
    pub candidate_index: usize,
    pub avg_score: f64,
    pub cumulative_score: f64,
}

// ============================================================================
// STRATEGY OPTIMIZER (trading)
// ============================================================================

/// A single parameter in a strategy's search space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyParam {
    /// Parameter name
    pub name: String,
    /// Type: "float", "int", "bool", "choice"
    pub param_type: String,
    /// Min value (for float/int)
    pub min: f64,
    /// Max value (for float/int)
    pub max: f64,
    /// Step size (for float/int grid search)
    pub step: f64,
    /// Current best value
    pub current: f64,
    /// Pre-defined choices (for "choice" type)
    #[serde(default)]
    pub choices: Vec<String>,
}

impl StrategyParam {
    pub fn int(name: &str, min: i64, max: i64, step: i64, current: i64) -> Self {
        Self {
            name: name.into(),
            param_type: "int".into(),
            min: min as f64,
            max: max as f64,
            step: step as f64,
            current: current as f64,
            choices: Vec::new(),
        }
    }

    pub fn float(name: &str, min: f64, max: f64, step: f64, current: f64) -> Self {
        Self {
            name: name.into(),
            param_type: "float".into(),
            min,
            max,
            step,
            current,
            choices: Vec::new(),
        }
    }

    pub fn choice(name: &str, choices: Vec<String>, current: &str) -> Self {
        let idx = choices.iter().position(|c| c == current).unwrap_or(0);
        Self {
            name: name.into(),
            param_type: "choice".into(),
            min: 0.0,
            max: choices.len() as f64 - 1.0,
            step: 1.0,
            current: idx as f64,
            choices,
        }
    }

    /// Generate all values in the search space for this parameter.
    pub fn grid_values(&self) -> Vec<f64> {
        match self.param_type.as_str() {
            "int" => {
                let mut vals = Vec::new();
                let mut v = self.min;
                while v <= self.max {
                    vals.push(v);
                    v += self.step;
                }
                vals
            }
            "float" => {
                let mut vals = Vec::new();
                let mut v = self.min;
                while v <= self.max + self.step * 0.5 {
                    vals.push((v * 1000.0).round() / 1000.0);
                    v += self.step;
                }
                vals
            }
            "choice" => {
                (0..self.choices.len()).map(|i| i as f64).collect()
            }
            _ => vec![self.current],
        }
    }

    /// Get the current value as a display string.
    pub fn current_display(&self) -> String {
        if self.param_type == "choice" && !self.choices.is_empty() {
            let idx = self.current as usize;
            self.choices.get(idx).cloned().unwrap_or_default()
        } else if self.param_type == "int" {
            format!("{}", self.current as i64)
        } else {
            format!("{:.3}", self.current)
        }
    }

    /// Set current from a grid value.
    pub fn set_current_from_grid(&mut self, grid_value: f64) {
        self.current = grid_value;
    }
}

/// A trading strategy configuration — set of parameters with scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    /// Strategy name
    pub name: String,
    /// Parameters to optimize
    pub params: Vec<StrategyParam>,
    /// Current best score
    pub best_score: f64,
    /// Best parameter snapshot
    pub best_params: HashMap<String, f64>,
    /// Optimization history
    pub history: Vec<StrategyEvaluation>,
}

impl StrategyConfig {
    pub fn new(name: impl Into<String>, params: Vec<StrategyParam>) -> Self {
        let best_params: HashMap<String, f64> = params
            .iter()
            .map(|p| (p.name.clone(), p.current))
            .collect();

        Self {
            name: name.into(),
            params,
            best_score: f64::NEG_INFINITY,
            best_params,
            history: Vec::new(),
        }
    }

    /// Total number of combinations in the grid.
    pub fn grid_size(&self) -> usize {
        self.params.iter().map(|p| p.grid_values().len()).product()
    }
}

/// Result of evaluating one strategy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyEvaluation {
    /// Parameter snapshot for this run
    pub params: HashMap<String, f64>,
    /// Score assigned by the scorer function
    pub score: f64,
    /// Additional metrics (e.g., Sharpe, win_rate, max_drawdown)
    pub metrics: HashMap<String, f64>,
    /// Execution time in ms
    pub duration_ms: u64,
}

/// Strategy optimizer — grid search over parameter space.
pub struct StrategyOptimizer {
    pub strategy: StrategyConfig,
    /// Maximum grid size before stopping (safety limit)
    pub max_grid_size: usize,
}

impl StrategyOptimizer {
    pub fn new(strategy: StrategyConfig) -> Self {
        Self {
            strategy,
            max_grid_size: 10_000,
        }
    }

    /// Run grid search optimization.
    ///
    /// The `scorer` function evaluates one parameter combination and
    /// returns a score + optional metrics (higher score = better).
    pub async fn optimize<F, Fut>(&mut self, scorer: F) -> Option<StrategyEvaluation>
    where
        F: Fn(&HashMap<String, f64>) -> Fut,
        Fut: std::future::Future<Output = (f64, HashMap<String, f64>)>,
    {
        let grid_size = self.strategy.grid_size();

        if grid_size > self.max_grid_size {
            tracing::warn!(
                "StrategyOptimizer[{}]: grid size {} exceeds max {} — reduce parameter space",
                self.strategy.name,
                grid_size,
                self.max_grid_size,
            );
            return None;
        }

        tracing::info!(
            "StrategyOptimizer[{}]: {} params, grid_size={}",
            self.strategy.name,
            self.strategy.params.len(),
            grid_size,
        );

        // Generate all parameter combinations via Cartesian product
        let combinations = self.generate_grid();
        let mut best: Option<StrategyEvaluation> = None;

        for (i, params) in combinations.iter().enumerate() {
            let start = std::time::Instant::now();
            let (score, metrics) = scorer(params).await;
            let duration_ms = start.elapsed().as_millis() as u64;

            let eval = StrategyEvaluation {
                params: params.clone(),
                score,
                metrics,
                duration_ms,
            };

            let is_better = best.as_ref().map_or(true, |b| eval.score > b.score);

            if is_better {
                tracing::info!(
                    "  [{}/{}] new best: score={:.4} (params: {:?})",
                    i + 1,
                    grid_size,
                    eval.score,
                    eval.params,
                );
                self.strategy.best_score = eval.score;
                self.strategy.best_params = eval.params.clone();
                for p in &mut self.strategy.params {
                    if let Some(&v) = eval.params.get(&p.name) {
                        p.current = v;
                    }
                }
                best = Some(eval.clone());
            }

            self.strategy.history.push(eval);
        }

        // Apply best params to strategy
        if let Some(ref b) = best {
            for (name, val) in &b.params {
                if let Some(param) = self.strategy.params.iter_mut().find(|p| &p.name == name) {
                    param.set_current_from_grid(*val);
                }
            }
        }

        tracing::info!(
            "StrategyOptimizer[{}]: done. best_score={:.4}",
            self.strategy.name,
            self.strategy.best_score,
        );

        best
    }

    /// Generate the full Cartesian product grid.
    fn generate_grid(&self) -> Vec<HashMap<String, f64>> {
        let mut result = vec![HashMap::new()];

        for param in &self.strategy.params {
            let values = param.grid_values();
            let mut new_result = Vec::with_capacity(result.len() * values.len());

            for combo in &result {
                for &val in &values {
                    let mut new_combo = combo.clone();
                    new_combo.insert(param.name.clone(), val);
                    new_result.push(new_combo);
                }
            }

            result = new_result;
        }

        result
    }
}

// ============================================================================
// AUTO FEW-SHOT SELECTOR
// ============================================================================

/// Automatically select the best few-shot examples from history.
pub struct AutoFewShot {
    /// Pool of all available examples
    pub pool: Vec<FewShotExample>,
    /// Maximum examples to select
    pub max_examples: usize,
    /// Minimum quality threshold (0.0–1.0)
    pub min_quality: f64,
}

impl AutoFewShot {
    pub fn new() -> Self {
        Self {
            pool: Vec::new(),
            max_examples: 5,
            min_quality: 0.5,
        }
    }

    /// Add an example to the pool.
    pub fn add(&mut self, example: FewShotExample) {
        self.pool.push(example);
    }

    /// Select the best examples for a given task description.
    ///
    /// Selection criteria (in priority order):
    /// 1. Quality >= min_quality
    /// 2. Tag relevance (if task has matching tags)
    /// 3. Recency (newer = better)
    /// 4. Diversity (avoid picking similar examples)
    ///
    /// Returns up to `max_examples` examples sorted by selection score.
    pub fn select(&self, task_description: &str, task_tags: &[String]) -> Vec<FewShotExample> {
        if self.pool.is_empty() {
            return Vec::new();
        }

        // Filter by quality
        let mut candidates: Vec<&FewShotExample> = self
            .pool
            .iter()
            .filter(|e| e.quality >= self.min_quality)
            .collect();

        if candidates.is_empty() {
            candidates = self.pool.iter().collect();
        }

        // Score each candidate
        let task_lower = task_description.to_lowercase();
        let mut scored: Vec<(f64, FewShotExample)> = candidates
            .into_iter()
            .map(|ex| {
                let mut score = ex.quality;

                // Tag relevance: +0.2 per matching tag
                for tag in &ex.tags {
                    if task_tags.contains(tag) || task_lower.contains(&tag.to_lowercase()) {
                        score += 0.2;
                    }
                }

                // Recency boost (newer = higher)
                let age_hours = (chrono::Utc::now().timestamp() as u64)
                    .saturating_sub(ex.timestamp) as f64 / 3600.0;
                let recency = 1.0 / (1.0 + age_hours / 24.0); // decays over days
                score += recency * 0.1;

                (score, ex.clone())
            })
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // Pick top N, ensuring diversity (skip near-duplicates)
        let mut selected = Vec::new();
        for (_, ex) in scored {
            if selected.len() >= self.max_examples {
                break;
            }

            // Diversity check: skip if too similar to already selected
            let is_duplicate = selected.iter().any(|s: &FewShotExample| {
                similarity(&s.input, &ex.input) > 0.8
            });

            if !is_duplicate {
                selected.push(ex.clone());
            }
        }

        selected
    }
}

impl Default for AutoFewShot {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple Jaccard similarity between two strings.
fn similarity(a: &str, b: &str) -> f64 {
    let a_words: std::collections::HashSet<&str> =
        a.split_whitespace().collect();
    let b_words: std::collections::HashSet<&str> =
        b.split_whitespace().collect();

    if a_words.is_empty() && b_words.is_empty() {
        return 1.0;
    }

    let intersection = a_words.intersection(&b_words).count();
    let union = a_words.union(&b_words).count();

    if union == 0 {
        return 0.0;
    }

    intersection as f64 / union as f64
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_prompt_optimizer_basic() {
        let mut opt = PromptOptimizer::new("test-task");
        opt.add_candidate(PromptCandidate::new("short", "Hi"));
        opt.add_candidate(PromptCandidate::new("long", "Be very helpful and detailed"));

        let inputs = vec!["hello".to_string()];

        let best = opt
            .optimize(&inputs, |candidate, input| {
                let len = candidate.system_prompt.len() as f64;
                let in_len = input.len() as f64;
                async move { len + in_len * 0.1 }
            })
            .await;

        assert!(best.is_some());
        assert_eq!(best.unwrap().name, "long");
        assert!(best.unwrap().evaluations >= 3);
    }

    #[test]
    fn test_strategy_param_grid_int() {
        let p = StrategyParam::int("lookback", 10, 30, 10, 20);
        let vals = p.grid_values();
        assert_eq!(vals, vec![10.0, 20.0, 30.0]);
    }

    #[test]
    fn test_strategy_param_grid_float() {
        let p = StrategyParam::float("threshold", 0.0, 1.0, 0.5, 0.5);
        let vals = p.grid_values();
        assert_eq!(vals.len(), 3);
    }

    #[test]
    fn test_strategy_param_choice() {
        let p = StrategyParam::choice(
            "side",
            vec!["long".into(), "short".into(), "both".into()],
            "long",
        );
        assert_eq!(p.current_display(), "long");
        let vals = p.grid_values();
        assert_eq!(vals.len(), 3);
    }

    #[test]
    fn test_strategy_grid_size() {
        let s = StrategyConfig::new(
            "test",
            vec![
                StrategyParam::int("a", 1, 3, 1, 1),
                StrategyParam::int("b", 10, 20, 10, 10),
            ],
        );
        // a: [1,2,3] = 3, b: [10,20] = 2, total = 6
        assert_eq!(s.grid_size(), 6);
    }

    #[test]
    fn test_auto_few_shot_selection() {
        let mut afs = AutoFewShot::new();
        afs.max_examples = 2;
        afs.min_quality = 0.3;

        afs.add(
            FewShotExample::new("Buy BTC at support", "Entered long at 42500")
                .with_quality(0.9)
                .with_tags(vec!["trading".into(), "BTC".into()]),
        );
        afs.add(
            FewShotExample::new("Sell ETH at resistance", "Exited at 3200")
                .with_quality(0.7)
                .with_tags(vec!["trading".into()]),
        );
        afs.add(
            FewShotExample::new("Weather in London", "Sunny 22C")
                .with_quality(0.2),
        );

        let selected = afs.select("crypto trading bot", &["trading".into()]);
        assert_eq!(selected.len(), 2);
        // Best quality should be first
        assert!(selected[0].quality >= selected[1].quality);
    }

    #[test]
    fn test_similarity() {
        let s = similarity("buy bitcoin at market price", "buy BTC at market");
        assert!(s > 0.3); // "buy", "at", "market" overlap

        let s2 = similarity("buy bitcoin", "sell ethereum");
        assert!(s2 < 0.3);
    }
}
