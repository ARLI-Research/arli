use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub model: String,
    pub provider: ProviderConfig,
    pub max_iterations: usize,
    pub agent_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub api_key: String,
    pub base_url: Option<String>,
    pub context_window: Option<usize>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: "gpt-4o".to_string(),
            provider: ProviderConfig {
                name: "openai".to_string(),
                api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
                base_url: None,
                context_window: Some(128_000),
            },
            max_iterations: 20,
            agent_name: "ARLI".to_string(),
        }
    }
}

impl Config {
    pub fn from_env() -> Result<Self, crate::error::Error> {
        let mut config = Self::default();

        // OpenRouter compatibility
        if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
            config.provider.api_key = key;
            config.provider.name = "openrouter".to_string();
            config.provider.base_url = Some("https://openrouter.ai/api/v1".to_string());
        }

        if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
            config.provider.api_key = key;
            config.provider.name = "deepseek".to_string();
            config.provider.base_url = Some("https://api.deepseek.com/v1".to_string());
            if config.model == "gpt-4o" {
                config.model = "deepseek-chat".to_string();
            }
        }

        if let Ok(model) = std::env::var("ARLI_MODEL") {
            config.model = model;
        }

        if let Ok(iterations) = std::env::var("ARLI_MAX_ITERATIONS") {
            config.max_iterations = iterations.parse().unwrap_or(20);
        }

        if config.provider.api_key.is_empty() {
            return Err(crate::error::Error::Config(
                "No API key found. Set OPENAI_API_KEY, OPENROUTER_API_KEY, or DEEPSEEK_API_KEY".into(),
            ));
        }

        Ok(config)
    }
}
