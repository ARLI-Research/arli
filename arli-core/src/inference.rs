//! Inference routing — LLM provider abstraction layer (OpenShell pattern).
//!
//! Routes chat completion requests to the appropriate provider,
//! stripping caller-supplied credentials and injecting gateway-managed API keys.
//!
//! This moves credential management OUT of agent code and INTO the gateway,
//! following OpenShell's proxy-based inference interception model.

use crate::providers::{ProviderDef, ProviderRegistry};

/// Inference route — maps a provider name to its resolved configuration.
#[derive(Debug, Clone)]
pub struct InferenceRoute {
    /// Provider definition
    pub provider: ProviderDef,
    /// Resolved API key (from gateway-managed secrets, not agent)
    pub api_key: Option<String>,
    /// Resolved base URL
    pub base_url: String,
    /// Model identifier
    pub model: String,
}

/// Inference router — resolves provider requests to backend configurations.
///
/// Strips any caller-supplied API keys and injects gateway-managed credentials.
pub struct InferenceRouter {
    registry: ProviderRegistry,
}

impl InferenceRouter {
    /// Create a new router with the embedded provider registry.
    pub fn new() -> Self {
        Self {
            registry: ProviderRegistry::load_embedded(),
        }
    }

    /// Resolve a route for the given provider and model.
    ///
    /// Returns the provider's base URL and gateway-managed API key.
    /// The caller's own API key (if any) is IGNORED — the gateway
    /// manages credentials centrally.
    pub fn resolve(&self, provider_name: &str, model: Option<&str>) -> Option<InferenceRoute> {
        let provider = self.registry.find(provider_name)?;

        let model = model
            .map(String::from)
            .or_else(|| provider.default_model.clone())
            .unwrap_or_else(|| "default".into());

        let api_key = provider.resolve_api_key();

        Some(InferenceRoute {
            provider: provider.clone(),
            api_key,
            base_url: provider.base_url.clone(),
            model,
        })
    }

    /// List all configured inference backends.
    pub fn list_backends(&self) -> Vec<&str> {
        self.registry.list_names()
    }

    /// Check if a provider supports streaming.
    pub fn supports_streaming(&self, name: &str) -> bool {
        self.registry
            .find(name)
            .map(|p| p.supports_streaming)
            .unwrap_or(false)
    }
}

impl Default for InferenceRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_deepseek() {
        let router = InferenceRouter::new();
        let route = router.resolve("deepseek", None).unwrap();
        assert_eq!(route.base_url, "https://api.deepseek.com/v1");
        assert_eq!(route.model, "deepseek-chat");
    }

    #[test]
    fn test_resolve_with_model_override() {
        let router = InferenceRouter::new();
        let route = router.resolve("openai", Some("gpt-4o-mini")).unwrap();
        assert_eq!(route.model, "gpt-4o-mini");
    }

    #[test]
    fn test_unknown_provider() {
        let router = InferenceRouter::new();
        assert!(router.resolve("nonexistent", None).is_none());
    }

    #[test]
    fn test_list_backends() {
        let router = InferenceRouter::new();
        let backends = router.list_backends();
        assert!(backends.contains(&"deepseek"));
        assert!(backends.contains(&"openai"));
    }
}
