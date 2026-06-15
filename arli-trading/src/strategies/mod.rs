//! Built-in trading strategies.
//!
//! Each strategy implements the `Strategy` trait and can be instantiated
//! by the agent factory via `StrategyRegistry`.
//!
//! `IndicatorStrategy` is the declarative strategy — it reads its
//! configuration from JSON and does NOT use the registry. The
//! `TradingHandler` creates it directly from `job_params`.

mod mean_reversion;
mod trend_following;
pub mod indicator_strategy;

pub use mean_reversion::MeanReversionStrategy;
pub use trend_following::TrendFollowingStrategy;
pub use indicator_strategy::{IndicatorStrategy, IndicatorStrategyConfig};

use crate::strategy::StrategyRegistry;

/// Register all built-in strategies into the registry.
pub fn register_builtin_strategies(registry: &mut StrategyRegistry) {
    registry.register(|| Box::new(MeanReversionStrategy::default()));
    registry.register(|| Box::new(TrendFollowingStrategy::default()));
    // IndicatorStrategy is NOT registered — created on-demand from job_params
}
