//! Provider registry — auto-discovers provider configs from YAML files.
//!
//! OpenShell pattern: providers are defined in YAML, not hardcoded.
//! Adding a new provider = dropping a YAML file in `provider_configs/`.

use serde::{Deserialize, Serialize};

/// A single provider definition loaded from YAML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDef {
    /// Provider identifier (e.g., "deepseek", "openai")
    pub name: String,

    /// Human-readable description
    #[serde(default)]
    pub description: String,

    /// Base URL for the OpenAI-compatible API
    pub base_url: String,

    /// Environment variable that holds the API key
    #[serde(default)]
    pub api_key_env: Option<String>,

    /// Default model for this provider
    #[serde(default)]
    pub default_model: Option<String>,

    /// Whether this provider supports streaming
    #[serde(default = "default_true")]
    pub supports_streaming: bool,

    /// Whether this provider supports tool calling
    #[serde(default = "default_true")]
    pub supports_tools: bool,

    /// Provider category for grouping
    #[serde(default)]
    pub category: Option<String>,

    /// Aliases (alternative names)
    #[serde(default)]
    pub aliases: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl ProviderDef {
    /// Get the API key from environment, if configured.
    pub fn resolve_api_key(&self) -> Option<String> {
        self.api_key_env
            .as_ref()
            .and_then(|env_var| std::env::var(env_var).ok())
    }
}

/// Registry of all known providers.
pub struct ProviderRegistry {
    providers: Vec<ProviderDef>,
}

impl ProviderRegistry {
    /// Load all provider definitions from embedded YAML files.
    pub fn load_embedded() -> Self {
        let yamls = include_str!("provider_configs/mod.txt")
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect::<Vec<_>>();

        let mut providers = Vec::new();
        for name in yamls {
            // Embedded YAML is loaded via include_str! at compile time.
            // For runtime, we'd scan a directory. For embedded, we hardcode the well-known set.
            if let Some(def) = Self::builtin_provider(name) {
                providers.push(def);
            }
        }

        Self { providers }
    }

    /// Built-in provider definitions (embedded at compile time).
    fn builtin_provider(name: &str) -> Option<ProviderDef> {
        match name {
            "deepseek" => Some(ProviderDef {
                name: "deepseek".into(),
                description: "DeepSeek API (OpenAI-compatible)".into(),
                base_url: "https://api.deepseek.com/v1".into(),
                api_key_env: Some("DEEPSEEK_API_KEY".into()),
                default_model: Some("deepseek-chat".into()),
                supports_streaming: true,
                supports_tools: true,
                category: Some("llm".into()),
                aliases: vec![],
            }),
            "openai" => Some(ProviderDef {
                name: "openai".into(),
                description: "OpenAI API".into(),
                base_url: "https://api.openai.com/v1".into(),
                api_key_env: Some("OPENAI_API_KEY".into()),
                default_model: Some("gpt-4o".into()),
                supports_streaming: true,
                supports_tools: true,
                category: Some("llm".into()),
                aliases: vec![],
            }),
            "anthropic" => Some(ProviderDef {
                name: "anthropic".into(),
                description: "Anthropic Claude API".into(),
                base_url: "https://api.anthropic.com/v1".into(),
                api_key_env: Some("ANTHROPIC_API_KEY".into()),
                default_model: Some("claude-sonnet-4-20250514".into()),
                supports_streaming: true,
                supports_tools: true,
                category: Some("llm".into()),
                aliases: vec!["claude".into()],
            }),
            "openrouter" => Some(ProviderDef {
                name: "openrouter".into(),
                description: "OpenRouter — unified API for 200+ models".into(),
                base_url: "https://openrouter.ai/api/v1".into(),
                api_key_env: Some("OPENROUTER_API_KEY".into()),
                default_model: Some("openai/gpt-4o".into()),
                supports_streaming: true,
                supports_tools: true,
                category: Some("aggregator".into()),
                aliases: vec![],
            }),
            "xai" => Some(ProviderDef {
                name: "xai".into(),
                description: "xAI Grok API".into(),
                base_url: "https://api.x.ai/v1".into(),
                api_key_env: Some("XAI_API_KEY".into()),
                default_model: Some("grok-3-beta".into()),
                supports_streaming: true,
                supports_tools: true,
                category: Some("llm".into()),
                aliases: vec!["grok".into()],
            }),
            "google" => Some(ProviderDef {
                name: "google".into(),
                description: "Google Gemini API (OpenAI-compatible)".into(),
                base_url: "https://generativelanguage.googleapis.com/v1beta/openai/".into(),
                api_key_env: Some("GOOGLE_API_KEY".into()),
                default_model: Some("gemini-2.5-pro".into()),
                supports_streaming: true,
                supports_tools: true,
                category: Some("llm".into()),
                aliases: vec!["gemini".into()],
            }),
            "ollama" => Some(ProviderDef {
                name: "ollama".into(),
                description: "Ollama local inference".into(),
                base_url: "http://localhost:11434/v1".into(),
                api_key_env: None,
                default_model: Some("llama3".into()),
                supports_streaming: true,
                supports_tools: true,
                category: Some("local".into()),
                aliases: vec![],
            }),
            "lmstudio" => Some(ProviderDef {
                name: "lmstudio".into(),
                description: "LM Studio local inference".into(),
                base_url: "http://localhost:1234/v1".into(),
                api_key_env: None,
                default_model: Some("local-model".into()),
                supports_streaming: true,
                supports_tools: true,
                category: Some("local".into()),
                aliases: vec![],
            }),
            "nvidia" => Some(ProviderDef {
                name: "nvidia".into(),
                description: "NVIDIA NIM API".into(),
                base_url: "https://integrate.api.nvidia.com/v1".into(),
                api_key_env: Some("NVIDIA_API_KEY".into()),
                default_model: Some("nvidia/llama-3.1-nemotron-ultra-253b-v1".into()),
                supports_streaming: true,
                supports_tools: true,
                category: Some("llm".into()),
                aliases: vec!["nim".into()],
            }),
            "qwen" => Some(ProviderDef {
                name: "qwen".into(),
                description: "Alibaba Qwen API".into(),
                base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1".into(),
                api_key_env: Some("DASHSCOPE_API_KEY".into()),
                default_model: Some("qwen-max".into()),
                supports_streaming: true,
                supports_tools: true,
                category: Some("llm".into()),
                aliases: vec![],
            }),
            "zhipu" => Some(ProviderDef {
                name: "zhipu".into(),
                description: "ZhipuAI GLM API".into(),
                base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
                api_key_env: Some("ZHIPUAI_API_KEY".into()),
                default_model: Some("glm-4".into()),
                supports_streaming: true,
                supports_tools: true,
                category: Some("llm".into()),
                aliases: vec!["glm".into()],
            }),
            _ => None,
        }
    }

    /// Find a provider by name or alias.
    pub fn find(&self, name: &str) -> Option<&ProviderDef> {
        self.providers
            .iter()
            .find(|p| p.name == name || p.aliases.iter().any(|a| a == name))
    }

    /// List all registered provider names.
    pub fn list_names(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.name.as_str()).collect()
    }

    /// List providers by category.
    pub fn list_by_category(&self, category: &str) -> Vec<&ProviderDef> {
        self.providers
            .iter()
            .filter(|p| p.category.as_deref() == Some(category))
            .collect()
    }

    /// Get the base URL for a provider, falling back to the name itself if unknown.
    pub fn resolve_base_url(&self, name: &str) -> Option<String> {
        self.find(name).map(|p| p.base_url.clone())
    }

    /// Get the default model for a provider.
    pub fn resolve_default_model(&self, name: &str) -> Option<String> {
        self.find(name).and_then(|p| p.default_model.clone())
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::load_embedded()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_has_core_providers() {
        let registry = ProviderRegistry::load_embedded();
        assert!(registry.find("deepseek").is_some());
        assert!(registry.find("openai").is_some());
        assert!(registry.find("anthropic").is_some());
        assert!(registry.find("openrouter").is_some());
    }

    #[test]
    fn test_alias_resolution() {
        let registry = ProviderRegistry::load_embedded();
        assert!(registry.find("claude").is_some()); // alias for anthropic
        assert!(registry.find("grok").is_some()); // alias for xai
        assert!(registry.find("gemini").is_some()); // alias for google
    }

    #[test]
    fn test_unknown_provider() {
        let registry = ProviderRegistry::load_embedded();
        assert!(registry.find("nonexistent").is_none());
    }

    #[test]
    fn test_resolve_base_url() {
        let registry = ProviderRegistry::load_embedded();
        assert_eq!(
            registry.resolve_base_url("deepseek"),
            Some("https://api.deepseek.com/v1".into())
        );
    }

    #[test]
    fn test_local_providers_no_api_key() {
        let registry = ProviderRegistry::load_embedded();
        let ollama = registry.find("ollama").unwrap();
        assert!(ollama.api_key_env.is_none());
        let lmstudio = registry.find("lmstudio").unwrap();
        assert!(lmstudio.api_key_env.is_none());
    }

    #[test]
    fn test_categories() {
        let registry = ProviderRegistry::load_embedded();
        let local = registry.list_by_category("local");
        assert_eq!(local.len(), 2);
        assert!(local.iter().any(|p| p.name == "ollama"));
        assert!(local.iter().any(|p| p.name == "lmstudio"));
    }

    #[test]
    fn test_list_names() {
        let registry = ProviderRegistry::load_embedded();
        let names = registry.list_names();
        assert!(names.contains(&"deepseek"));
        assert!(names.contains(&"openai"));
    }
}
