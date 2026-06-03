use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use crate::brokering::BrokeringConfig;
pub use crate::x402::X402Config;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub model: String,
    pub provider: ProviderConfig,
    pub max_iterations: usize,
    pub agent_name: String,
    #[serde(default)]
    pub session_reset: SessionResetConfig,
    #[serde(default = "default_tool_progress")]
    pub tool_progress: String,
    #[serde(default = "default_compression_threshold")]
    pub compression_threshold: f32,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub feedback: FeedbackConfig,
    #[serde(default)]
    pub terminal: TerminalConfig,
    #[serde(default)]
    pub browser: BrowserConfig,
    #[serde(default)]
    pub x402: X402Config,
    #[serde(default)]
    pub brokering: BrokeringConfig,

    /// Time-traveling stream rules — regex-based response filtering.
    #[serde(default)]
    pub stream_rules: crate::stream_rules::StreamRules,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub api_key: String,
    pub base_url: Option<String>,
    pub context_window: Option<usize>,
}

/// Session reset policy configuration.
///
/// Controls when the agent should auto-reset its session to prevent
/// stale context from degrading performance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResetConfig {
    /// Reset mode: "inactivity_daily", "inactivity", "daily", "never"
    #[serde(default = "default_reset_mode")]
    pub mode: String,
    /// Inactivity timeout in minutes (default: 1440 = 24 hours)
    #[serde(default = "default_inactivity_minutes")]
    pub inactivity_minutes: u32,
    /// Hour of day (0-23) for daily reset (default: 4 AM)
    #[serde(default = "default_daily_reset_hour")]
    pub daily_reset_hour: u8,
}

fn default_reset_mode() -> String {
    "inactivity_daily".to_string()
}

fn default_inactivity_minutes() -> u32 {
    1440
}

fn default_daily_reset_hour() -> u8 {
    4
}

impl Default for SessionResetConfig {
    fn default() -> Self {
        Self {
            mode: default_reset_mode(),
            inactivity_minutes: default_inactivity_minutes(),
            daily_reset_hour: default_daily_reset_hour(),
        }
    }
}

fn default_tool_progress() -> String {
    "all".to_string()
}

fn default_compression_threshold() -> f32 {
    0.5
}

/// Search provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Search provider: "duckduckgo", "brave", "searxng", "tavily",
    /// "firecrawl", "exa", "parallel", "xai"
    #[serde(default = "default_search_provider")]
    pub provider: String,
    /// API key for the search provider
    #[serde(default)]
    pub api_key: String,
    /// Base URL (for self-hosted SearXNG / Firecrawl)
    #[serde(default)]
    pub base_url: String,
}

fn default_search_provider() -> String {
    "duckduckgo".to_string()
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            provider: default_search_provider(),
            api_key: String::new(),
            base_url: String::new(),
        }
    }
}

/// Memory provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Memory provider: "builtin", "mem0", "chroma", "qdrant",
    /// "byterover", "hindsight", "holographic", "honcho",
    /// "openviking", "retaindb", "supermemory", "agentmemory"
    #[serde(default = "default_memory_provider")]
    pub provider: String,
    /// API key for the memory provider
    #[serde(default)]
    pub api_key: String,
    /// Base URL for the memory provider
    #[serde(default)]
    pub base_url: String,
}

fn default_memory_provider() -> String {
    "builtin".to_string()
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            provider: default_memory_provider(),
            api_key: String::new(),
            base_url: String::new(),
        }
    }
}

/// Feedback loop configuration for user corrections.
///
/// When the user corrects the agent (e.g., "no, do X instead"),
/// the correction is stored in memory with target="correction"
/// for future learning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackConfig {
    /// Whether the feedback loop is enabled
    #[serde(default = "default_feedback_enabled")]
    pub enabled: bool,
    /// Whether to auto-save corrections to memory
    #[serde(default = "default_auto_learn")]
    pub auto_learn: bool,
    /// Maximum number of corrections to store
    #[serde(default = "default_max_corrections")]
    pub max_corrections: usize,
}

fn default_feedback_enabled() -> bool {
    true
}

fn default_auto_learn() -> bool {
    true
}

fn default_max_corrections() -> usize {
    100
}

impl Default for FeedbackConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_learn: true,
            max_corrections: 100,
        }
    }
}

/// Terminal backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalConfig {
    /// Terminal backend: "local", "docker", "ssh", "modal", "daytona", "singularity"
    #[serde(default = "default_terminal_backend")]
    pub backend: String,
    /// Docker image (when backend is "docker"), default "rust:latest"
    #[serde(default = "default_docker_image")]
    pub docker_image: String,
    /// SSH host (when backend is "ssh")
    #[serde(default)]
    pub ssh_host: String,
    /// SSH user (when backend is "ssh")
    #[serde(default)]
    pub ssh_user: String,
}

fn default_terminal_backend() -> String {
    "local".to_string()
}

fn default_docker_image() -> String {
    "rust:latest".to_string()
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            backend: default_terminal_backend(),
            docker_image: default_docker_image(),
            ssh_host: String::new(),
            ssh_user: String::new(),
        }
    }
}

/// Browser automation provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    /// Browser provider: "local", "camofox", "browserbase", "firecrawl", "browseruse"
    #[serde(default = "default_browser_provider")]
    pub provider: String,
    /// API key for cloud browser providers (Browserbase, Firecrawl, Browser Use)
    #[serde(default)]
    pub api_key: String,
    /// Base URL for cloud browser endpoints
    #[serde(default)]
    pub base_url: String,
}

fn default_browser_provider() -> String {
    "local".to_string()
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            provider: default_browser_provider(),
            api_key: String::new(),
            base_url: String::new(),
        }
    }
}

pub fn arli_home() -> PathBuf {
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
            max_iterations: 90,
            agent_name: "ARLI".to_string(),
            session_reset: SessionResetConfig::default(),
            tool_progress: "all".to_string(),
            compression_threshold: 0.5,
            search: SearchConfig::default(),
            memory: MemoryConfig::default(),
            feedback: FeedbackConfig::default(),
            terminal: TerminalConfig::default(),
            browser: BrowserConfig::default(),
            x402: X402Config::default(),
            brokering: BrokeringConfig::default(),
            stream_rules: crate::stream_rules::StreamRules::default(),
        }
    }
}

impl Config {
    /// Load environment variables from ~/.arli/.env file.
    /// Simple KEY=VALUE parser — no external dependency.
    fn load_dotenv() {
        let env_path = arli_home().join(".env");
        if let Ok(contents) = std::fs::read_to_string(&env_path) {
            for line in contents.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = trimmed.split_once('=') {
                    let key = key.trim();
                    let value = value.trim().trim_matches('"').trim_matches('\'');
                    if std::env::var(key).is_err() {
                        std::env::set_var(key, value);
                    }
                }
            }
        }
    }

    /// Load config from ~/.arli/config.toml, then override with env vars.
    /// Env vars take precedence over config file.
    pub fn from_env() -> Result<Self, crate::error::Error> {
        // 0. Load .env file from ~/.arli/.env if it exists
        Self::load_dotenv();

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
                    if !file_config.tool_progress.is_empty() {
                        config.tool_progress = file_config.tool_progress;
                    }
                    if file_config.compression_threshold > 0.0 {
                        config.compression_threshold = file_config.compression_threshold;
                    }
                    if let Some(sr) = file_config.session_reset {
                        config.session_reset.mode = sr.mode;
                        config.session_reset.inactivity_minutes = sr.inactivity_minutes;
                        config.session_reset.daily_reset_hour = sr.daily_reset_hour;
                    }
                    if let Some(sc) = file_config.search {
                        config.search.provider = sc.provider;
                        config.search.api_key = sc.api_key;
                        config.search.base_url = sc.base_url;
                    }
                    // Browser provider
                    if !file_config.browser.provider.is_empty() {
                        config.browser.provider = file_config.browser.provider;
                    }
                    if !file_config.browser.api_key.is_empty() {
                        config.browser.api_key = file_config.browser.api_key;
                    }
                    if !file_config.browser.base_url.is_empty() {
                        config.browser.base_url = file_config.browser.base_url;
                    }
                    // Memory provider
                    if !file_config.memory.provider.is_empty()
                        && file_config.memory.provider != "builtin"
                    {
                        config.memory.provider = file_config.memory.provider;
                    }
                    if !file_config.memory.api_key.is_empty() {
                        config.memory.api_key = file_config.memory.api_key;
                    }
                    if !file_config.memory.base_url.is_empty() {
                        config.memory.base_url = file_config.memory.base_url;
                    }
                    // Terminal backend
                    if !file_config.terminal.backend.is_empty()
                        && file_config.terminal.backend != "local"
                    {
                        config.terminal.backend = file_config.terminal.backend;
                    }
                    if !file_config.terminal.docker_image.is_empty() {
                        config.terminal.docker_image = file_config.terminal.docker_image;
                    }
                    if !file_config.terminal.ssh_host.is_empty() {
                        config.terminal.ssh_host = file_config.terminal.ssh_host;
                    }
                    if !file_config.terminal.ssh_user.is_empty() {
                        config.terminal.ssh_user = file_config.terminal.ssh_user;
                    }
                    // x402 agentic wallet
                    if file_config.x402.enabled {
                        config.x402 = file_config.x402;
                    }
                    // Stream rules (TTSR)
                    if let Some(sr) = file_config.stream_rules {
                        config.stream_rules = sr;
                    }
                    // Feedback loop
                    if !file_config.feedback.enabled {
                        config.feedback.enabled = false;
                    }
                    if !file_config.feedback.auto_learn {
                        config.feedback.auto_learn = false;
                    }
                    if file_config.feedback.max_corrections != 100 {
                        config.feedback.max_corrections = file_config.feedback.max_corrections;
                    }
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

        if let Ok(key) =
            std::env::var("GOOGLE_API_KEY").or_else(|_| std::env::var("GOOGLE_OAUTH_TOKEN"))
        {
            config.provider.api_key = key;
            config.provider.name = "google".to_string();
            config.provider.base_url =
                Some("https://generativelanguage.googleapis.com/v1beta/openai/".to_string());
            if config.model == "gpt-4o" {
                config.model = "gemini-2.5-flash".to_string();
            }
        }

        if let Ok(key) = std::env::var("XAI_API_KEY").or_else(|_| std::env::var("XAI_OAUTH_TOKEN"))
        {
            config.provider.api_key = key;
            config.provider.name = "xai".to_string();
            config.provider.base_url = Some("https://api.x.ai/v1".to_string());
            if config.model == "gpt-4o" {
                config.model = "grok-4".to_string();
            }
        }

        if let Ok(key) = std::env::var("GITHUB_TOKEN") {
            // Only use GitHub token if no other provider is explicitly set
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "copilot".to_string();
                config.provider.base_url = Some("https://api.githubcopilot.com".to_string());
                if config.model == "gpt-4o" {
                    config.model = "gpt-4o".to_string();
                }
            }
        }

        // LM Studio (local desktop app, OpenAI-compatible)
        if let Ok(key) = std::env::var("LM_STUDIO_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "lmstudio".to_string();
                config.provider.base_url = Some("http://localhost:1234/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "local-model".to_string();
                }
            }
        }

        // Ollama (local, OpenAI-compatible)
        if let Ok(key) = std::env::var("OLLAMA_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "ollama".to_string();
                config.provider.base_url = Some("http://localhost:11434/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "llama3".to_string();
                }
            }
        }

        // HuggingFace Inference API (OpenAI-compatible)
        if let Ok(key) = std::env::var("HF_API_KEY") {
            config.provider.api_key = key;
            config.provider.name = "huggingface".to_string();
            config.provider.base_url = Some("https://api-inference.huggingface.co/v1".to_string());
            if config.model == "gpt-4o" {
                config.model = "meta-llama/Meta-Llama-3-70B-Instruct".to_string();
            }
        }

        // AWS Bedrock (requires a proxy like LiteLLM — user must provide base_url)
        if let Ok(key) = std::env::var("AWS_ACCESS_KEY_ID") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "bedrock".to_string();
                if config.model == "gpt-4o" {
                    config.model = "us.anthropic.claude-sonnet-4-20250514-v1:0".to_string();
                }
            }
        }

        // Azure Foundry (OpenAI-compatible, requires AZURE_ENDPOINT for base_url)
        if let Ok(key) = std::env::var("AZURE_API_KEY") {
            config.provider.api_key = key;
            config.provider.name = "azure".to_string();
            if let Ok(endpoint) = std::env::var("AZURE_ENDPOINT") {
                config.provider.base_url = Some(endpoint);
            }
            if config.model == "gpt-4o" {
                config.model =
                    std::env::var("AZURE_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
            }
        }

        // OpenAI Codex (CLI agent)
        {
            let codex_key = std::env::var("OPENAI_CODEX_API_KEY").ok().or_else(|| {
                // Try reading from ~/.codex/auth.json
                let auth_path = std::env::var("HOME")
                    .map(|h| std::path::PathBuf::from(h).join(".codex/auth.json"))
                    .ok()?;
                if auth_path.exists() {
                    std::fs::read_to_string(&auth_path)
                        .ok()
                        .and_then(|contents| {
                            serde_json::from_str::<serde_json::Value>(&contents)
                                .ok()
                                .and_then(|v| v.get("token")?.as_str().map(|s| s.to_string()))
                        })
                } else {
                    None
                }
            });
            if let Some(key) = codex_key {
                if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                    config.provider.api_key = key;
                    config.provider.name = "codex".to_string();
                    config.provider.base_url = Some("https://api.openai.com/v1".to_string());
                    if config.model == "gpt-4o" {
                        config.model = "gpt-4o".to_string();
                    }
                }
            }
        }

        // Nous Portal (Nous Research subscription, OpenAI-compatible)
        if let Ok(key) = std::env::var("NOUS_API_KEY") {
            config.provider.api_key = key;
            config.provider.name = "nous".to_string();
            config.provider.base_url = Some("https://portal.nousresearch.com/api/v1".to_string());
            if config.model == "gpt-4o" {
                config.model = "hermes-3".to_string();
            }
        }

        // NovitaAI (OpenAI-compatible)
        if let Ok(key) = std::env::var("NOVITA_API_KEY") {
            config.provider.api_key = key;
            config.provider.name = "novita".to_string();
            config.provider.base_url = Some("https://api.novita.ai/v3/openai/v1".to_string());
            if config.model == "gpt-4o" {
                config.model = "novita-llama-3.1-70b".to_string();
            }
        }

        // Qwen Cloud / DashScope (OpenAI-compatible, also accepts QWEN_OAUTH_TOKEN)
        if let Ok(key) =
            std::env::var("DASHSCOPE_API_KEY").or_else(|_| std::env::var("QWEN_OAUTH_TOKEN"))
        {
            config.provider.api_key = key;
            config.provider.name = "qwen".to_string();
            config.provider.base_url =
                Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1".to_string());
            if config.model == "gpt-4o" {
                config.model = "qwen-max".to_string();
            }
        }

        // Xiaomi MiMo (OpenAI-compatible, guarded)
        if let Ok(key) = std::env::var("MIMO_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "mimo".to_string();
                config.provider.base_url = Some("https://api.mimo.xiaomi.com/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "mimo-v2.5-pro".to_string();
                }
            }
        }

        // Tencent TokenHub (OpenAI-compatible, guarded)
        if let Ok(key) = std::env::var("TENCENT_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "tencent".to_string();
                config.provider.base_url = Some("https://tokenhub.tencentmaas.com/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "hy3-preview".to_string();
                }
            }
        }

        // NVIDIA NIM (OpenAI-compatible)
        if let Ok(key) = std::env::var("NVIDIA_API_KEY") {
            config.provider.api_key = key;
            config.provider.name = "nvidia".to_string();
            config.provider.base_url = Some("https://integrate.api.nvidia.com/v1".to_string());
            if config.model == "gpt-4o" {
                config.model = "nemotron-4-340b".to_string();
            }
        }

        // Z.AI / GLM (OpenAI-compatible)
        if let Ok(key) = std::env::var("ZHIPU_API_KEY") {
            config.provider.api_key = key;
            config.provider.name = "zhipu".to_string();
            config.provider.base_url = Some("https://open.bigmodel.cn/api/paas/v4".to_string());
            if config.model == "gpt-4o" {
                config.model = "glm-4".to_string();
            }
        }

        // Kimi Coding Plan / Moonshot (OpenAI-compatible, guarded)
        if let Ok(key) = std::env::var("MOONSHOT_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "moonshot".to_string();
                config.provider.base_url = Some("https://api.moonshot.cn/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "moonshot-v1-128k".to_string();
                }
            }
        }

        // Kimi / Moonshot China (OpenAI-compatible, guarded)
        if let Ok(key) = std::env::var("MOONSHOT_CN_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "moonshot_cn".to_string();
                config.provider.base_url = Some("https://api.moonshot.cn/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "moonshot-v1-128k".to_string();
                }
            }
        }

        // StepFun Step Plan (OpenAI-compatible, guarded)
        if let Ok(key) = std::env::var("STEPFUN_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "stepfun".to_string();
                config.provider.base_url = Some("https://api.stepfun.com/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "step-2-16k".to_string();
                }
            }
        }

        // MiniMax (global, OpenAI-compatible, also accepts MINIMAX_OAUTH_TOKEN)
        if let Ok(key) =
            std::env::var("MINIMAX_API_KEY").or_else(|_| std::env::var("MINIMAX_OAUTH_TOKEN"))
        {
            config.provider.api_key = key;
            config.provider.name = "minimax".to_string();
            config.provider.base_url = Some("https://api.minimax.chat/v1".to_string());
            if config.model == "gpt-4o" {
                config.model = "abab7".to_string();
            }
        }

        // MiniMax China (OpenAI-compatible, guarded)
        if let Ok(key) = std::env::var("MINIMAX_CN_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "minimax_cn".to_string();
                config.provider.base_url = Some("https://api.minimaxi.com/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "abab7".to_string();
                }
            }
        }

        // Arcee AI (OpenAI-compatible, guarded)
        if let Ok(key) = std::env::var("ARCEE_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "arcee".to_string();
                config.provider.base_url = Some("https://api.arcee.ai/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "trinity".to_string();
                }
            }
        }

        // GMI Cloud (OpenAI-compatible, guarded)
        if let Ok(key) = std::env::var("GMI_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "gmi".to_string();
                config.provider.base_url = Some("https://api.gmicloud.ai/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "gmi-model".to_string();
                }
            }
        }

        // Kilo Code (OpenAI-compatible, guarded)
        if let Ok(key) = std::env::var("KILO_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "kilo".to_string();
                config.provider.base_url = Some("https://api.kilocode.ai/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "kilo-model".to_string();
                }
            }
        }

        // OpenCode Zen (OpenAI-compatible, guarded)
        if let Ok(key) = std::env::var("OPENCODE_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "opencode".to_string();
                config.provider.base_url = Some("https://api.opencode.ai/zen/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "opencode-model".to_string();
                }
            }
        }

        // OpenCode Go (OpenAI-compatible, guarded)
        if let Ok(key) = std::env::var("OPENCODE_GO_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "opencode_go".to_string();
                config.provider.base_url = Some("https://api.opencode.ai/go/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "opencode-model".to_string();
                }
            }
        }

        // Alibaba Cloud Coding Plan (OpenAI-compatible, guarded)
        if let Ok(key) = std::env::var("ALIBABA_CLOUD_API_KEY") {
            if config.provider.api_key.is_empty() || config.provider.name == "openai" {
                config.provider.api_key = key;
                config.provider.name = "alibaba_cloud".to_string();
                config.provider.base_url =
                    Some("https://dashscope.aliyuncs.com/compatible-mode/v1".to_string());
                if config.model == "gpt-4o" {
                    config.model = "qwen-coder".to_string();
                }
            }
        }

        if let Ok(model) = std::env::var("ARLI_MODEL") {
            config.model = model;
        }

        if let Ok(iterations) = std::env::var("ARLI_MAX_ITERATIONS") {
            config.max_iterations = iterations.parse().unwrap_or(90);
        }

        if let Ok(tp) = std::env::var("ARLI_TOOL_PROGRESS") {
            config.tool_progress = tp;
        }

        if let Ok(ct) = std::env::var("ARLI_COMPRESSION_THRESHOLD") {
            if let Ok(val) = ct.parse::<f32>() {
                config.compression_threshold = val;
            }
        }

        // Search provider
        if let Ok(sp) = std::env::var("ARLI_SEARCH_PROVIDER") {
            config.search.provider = sp;
        }
        if let Ok(sk) = std::env::var("SEARCH_API_KEY") {
            config.search.api_key = sk;
        }
        if let Ok(su) = std::env::var("SEARXNG_URL") {
            config.search.base_url = su;
        }

        // Browser provider
        if let Ok(bp) = std::env::var("BROWSER_PROVIDER") {
            config.browser.provider = bp;
        }
        if let Ok(bk) = std::env::var("BROWSERBASE_API_KEY") {
            config.browser.api_key = bk;
        }
        if let Ok(bu) = std::env::var("FIRECRAWL_API_KEY") {
            if config.browser.api_key.is_empty() {
                config.browser.api_key = bu;
            }
        }

        // Memory provider overrides
        if let Ok(mp) = std::env::var("ARLI_MEMORY_PROVIDER") {
            config.memory.provider = mp;
        }
        if let Ok(mk) = std::env::var("MEM0_API_KEY") {
            config.memory.api_key = mk;
            if config.memory.provider == "builtin" {
                config.memory.provider = "mem0".to_string();
            }
        }
        if let Ok(ck) = std::env::var("CHROMA_URL") {
            config.memory.base_url = ck;
        }
        if let Ok(qk) = std::env::var("QDRANT_API_KEY") {
            config.memory.api_key = qk;
        }
        if let Ok(qu) = std::env::var("QDRANT_URL") {
            config.memory.base_url = qu;
        }

        // Terminal backend overrides
        if let Ok(tb) = std::env::var("ARLI_TERMINAL_BACKEND") {
            config.terminal.backend = tb;
        }
        if let Ok(di) = std::env::var("ARLI_DOCKER_IMAGE") {
            config.terminal.docker_image = di;
        }
        if let Ok(sh) = std::env::var("ARLI_SSH_HOST") {
            config.terminal.ssh_host = sh;
        }
        if let Ok(su) = std::env::var("ARLI_SSH_USER") {
            config.terminal.ssh_user = su;
        }

        // x402 agentic wallet overrides
        if let Ok(xa) = std::env::var("X402_WALLET_ADDRESS") {
            config.x402.wallet_address = xa;
            config.x402.enabled = true;
        }
        if let Ok(xp) = std::env::var("X402_PRIVATE_KEY") {
            config.x402.private_key = xp;
            config.x402.enabled = true;
        }
        if let Ok(xr) = std::env::var("X402_RPC_URL") {
            config.x402.rpc_url = xr;
        }
        if let Ok(xb) = std::env::var("X402_BUDGET_CENTS") {
            if let Ok(b) = xb.parse::<u64>() {
                config.x402.total_budget_cents = b;
            }
        }

        if config.provider.api_key.is_empty() {
            return Err(crate::error::Error::Config(
                "No API key found. Run 'arli setup' or set DEEPSEEK_API_KEY / OPENAI_API_KEY / ANTHROPIC_API_KEY / GOOGLE_API_KEY / XAI_API_KEY / GITHUB_TOKEN / LM_STUDIO_API_KEY / OLLAMA_API_KEY / HF_API_KEY / AWS_ACCESS_KEY_ID / AZURE_API_KEY / OPENAI_CODEX_API_KEY / NOUS_API_KEY / NOVITA_API_KEY / DASHSCOPE_API_KEY / MIMO_API_KEY / TENCENT_API_KEY / NVIDIA_API_KEY / ZHIPU_API_KEY / MOONSHOT_API_KEY / MOONSHOT_CN_API_KEY / STEPFUN_API_KEY / MINIMAX_API_KEY / MINIMAX_CN_API_KEY / ARCEE_API_KEY / GMI_API_KEY / KILO_API_KEY / OPENCODE_API_KEY / OPENCODE_GO_API_KEY / ALIBABA_CLOUD_API_KEY".into(),
            ));
        }

        // Set base_url from provider name if not set
        if config.provider.base_url.is_none() {
            config.provider.base_url = match config.provider.name.as_str() {
                "openai" => Some("https://api.openai.com/v1".to_string()),
                "deepseek" => Some("https://api.deepseek.com/v1".to_string()),
                "anthropic" => Some("https://api.anthropic.com/v1".to_string()),
                "google" => {
                    Some("https://generativelanguage.googleapis.com/v1beta/openai/".to_string())
                }
                "xai" => Some("https://api.x.ai/v1".to_string()),
                "copilot" => Some("https://api.githubcopilot.com".to_string()),
                "lmstudio" => Some("http://localhost:1234/v1".to_string()),
                "ollama" => Some("http://localhost:11434/v1".to_string()),
                "huggingface" => Some("https://api-inference.huggingface.co/v1".to_string()),
                "bedrock" => None, // user must provide base_url via proxy like LiteLLM
                "azure" => None,   // user must provide AZURE_ENDPOINT
                "codex" => Some("https://api.openai.com/v1".to_string()),
                "nous" => Some("https://portal.nousresearch.com/api/v1".to_string()),
                "novita" => Some("https://api.novita.ai/v3/openai/v1".to_string()),
                "qwen" => {
                    Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1".to_string())
                }
                "mimo" => Some("https://api.mimo.xiaomi.com/v1".to_string()),
                "tencent" => Some("https://tokenhub.tencentmaas.com/v1".to_string()),
                "nvidia" => Some("https://integrate.api.nvidia.com/v1".to_string()),
                "zhipu" => Some("https://open.bigmodel.cn/api/paas/v4".to_string()),
                "moonshot" => Some("https://api.moonshot.cn/v1".to_string()),
                "moonshot_cn" => Some("https://api.moonshot.cn/v1".to_string()),
                "stepfun" => Some("https://api.stepfun.com/v1".to_string()),
                "minimax" => Some("https://api.minimax.chat/v1".to_string()),
                "minimax_cn" => Some("https://api.minimaxi.com/v1".to_string()),
                "arcee" => Some("https://api.arcee.ai/v1".to_string()),
                "gmi" => Some("https://api.gmicloud.ai/v1".to_string()),
                "kilo" => Some("https://api.kilocode.ai/v1".to_string()),
                "opencode" => Some("https://api.opencode.ai/zen/v1".to_string()),
                "opencode_go" => Some("https://api.opencode.ai/go/v1".to_string()),
                "alibaba_cloud" => {
                    Some("https://dashscope.aliyuncs.com/compatible-mode/v1".to_string())
                }
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
    #[serde(default = "default_tool_progress")]
    tool_progress: String,
    #[serde(default = "default_compression_threshold")]
    compression_threshold: f32,
    #[serde(default)]
    session_reset: Option<SessionResetFile>,
    #[serde(default)]
    pub search: Option<SearchConfigFile>,
    #[serde(default)]
    pub browser: BrowserConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub feedback: FeedbackConfig,
    #[serde(default)]
    pub terminal: TerminalConfig,
    #[serde(default)]
    pub x402: X402Config,

    /// Stream rules loaded from config TOML.
    #[serde(default)]
    pub stream_rules: Option<crate::stream_rules::StreamRules>,
}

#[derive(Debug, Deserialize)]
struct ProviderFile {
    #[serde(default)]
    name: String,
    #[serde(default)]
    api_key: String,
    base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionResetFile {
    #[serde(default = "default_reset_mode")]
    mode: String,
    #[serde(default = "default_inactivity_minutes")]
    inactivity_minutes: u32,
    #[serde(default = "default_daily_reset_hour")]
    daily_reset_hour: u8,
}

#[derive(Debug, Deserialize)]
struct SearchConfigFile {
    #[serde(default = "default_search_provider")]
    provider: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    base_url: String,
}

fn default_max_iterations() -> usize {
    90
}
