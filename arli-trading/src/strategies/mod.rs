//! Built-in trading strategies.
//!
//! Each strategy implements the `Strategy` trait and can be instantiated
//! by the agent factory via `StrategyRegistry`.

mod mean_reversion;
mod trend_following;

pub use mean_reversion::MeanReversionStrategy;
pub use trend_following::TrendFollowingStrategy;

use crate::strategy::StrategyRegistry;

/// Register all built-in strategies into the registry.
pub fn register_builtin_strategies(registry: &mut StrategyRegistry) {
    registry.register(|| Box::new(MeanReversionStrategy::default()));
    registry.register(|| Box::new(TrendFollowingStrategy::default()));
}
