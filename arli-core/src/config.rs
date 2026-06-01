use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

fn arli_home() -> PathBuf {
    std::env::var("ARLI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".arli")
        })
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
    /// Load config from ~/.arli/config.toml, then override with env vars.
    /// Env vars take precedence over config file.
    pub fn from_env() -> Result<Self, crate::error::Error> {
        let mut config = Self::default();

        // 1. Load from config.toml if it exists
        let config_path = arli_home().join("config.toml");
        if config_path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&config_path) {
                if let Ok(file_config) = toml::from_str::<ConfigFile>(&contents) {
                    config.provider.name = file_config.provider.name;
                    config.provider.api_key = file_config.provider.api_key;
                    if let Some(url) = file_config.provider.base_url {
                        config.provider.base_url = Some(url);
                    }
                    if !file_config.model.is_empty() {
                        config.model = file_config.model;
                    }
                    config.max_iterations = file_config.max_iterations;
                }
            }
        }

        // 2. Override with env vars (env takes precedence)

        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            config.provider.api_key = key;
            config.provider.name = "openai".to_string();
            config.provider.base_url = Some("https://api.openai.com/v1".to_string());
        }

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

        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            config.provider.api_key = key;
            config.provider.name = "anthropic".to_string();
            config.provider.base_url = Some("https://api.anthropic.com/v1".to_string());
            if config.model == "gpt-4o" {
                config.model = "claude-sonnet-4-20250514".to_string();
            }
        }

        if let Ok(key) = std::env::var("GOOGLE_API_KEY") {
            config.provider.api_key = key;
            config.provider.name = "google".to_string();
            config.provider.base_url = Some("https://generativelanguage.googleapis.com/v1beta/openai/".to_string());
            if config.model == "gpt-4o" {
                config.model = "gemini-2.5-flash".to_string();
            }
        }

        if let Ok(key) = std::env::var("XAI_API_KEY") {
            config.provider.api_key = key;
            config.provider.name = "xai".to_string();
            config.provider.base_url = Some("https://api.x.ai/v1".to_string());
            if config.model == "gpt-4o" {
                config.model = "grok-4".to_string();
            }
        }

        if let Ok(key) = std::env::var("GITHUB_TOKEN") {
            // Only use GitHub token if no other provider is explicitly set
            if config.provider.api_key.is_empty()
                || config.provider.name == "openai"
            {
                config.provider.api_key = key;
                config.provider.name = "copilot".to_string();
                config.provider.base_url = Some("https://api.githubcopilot.com".to_string());
                if config.model == "gpt-4o" {
                    config.model = "gpt-4o".to_string();
                }
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
                "No API key found. Run 'arli setup' or set DEEPSEEK_API_KEY / OPENAI_API_KEY / ANTHROPIC_API_KEY / GOOGLE_API_KEY / XAI_API_KEY / GITHUB_TOKEN".into(),
            ));
        }

        // Set base_url from provider name if not set
        if config.provider.base_url.is_none() {
            config.provider.base_url = match config.provider.name.as_str() {
                "openai" => Some("https://api.openai.com/v1".to_string()),
                "deepseek" => Some("https://api.deepseek.com/v1".to_string()),
                "anthropic" => Some("https://api.anthropic.com/v1".to_string()),
                "google" => Some("https://generativelanguage.googleapis.com/v1beta/openai/".to_string()),
                "xai" => Some("https://api.x.ai/v1".to_string()),
                "copilot" => Some("https://api.githubcopilot.com".to_string()),
                _ => None,
            };
        }

        Ok(config)
    }
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    model: String,
    provider: ProviderFile,
    #[serde(default = "default_max_iterations")]
    max_iterations: usize,
}

#[derive(Debug, Deserialize)]
struct ProviderFile {
    #[serde(default)]
    name: String,
    #[serde(default)]
    api_key: String,
    base_url: Option<String>,
}

fn default_max_iterations() -> usize {
    20
}
