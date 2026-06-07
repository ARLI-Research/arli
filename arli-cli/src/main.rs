mod tui;

use arli_core::tools::builtin::register_builtin_tools;
use arli_core::{
    create_provider, Agent, AgentConfig, ChatMessage, Config, CronEvent, CronJob, CronScheduler,
    PolicyEngine, SessionStore, ToolRegistry,
};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::info;
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "arli", about = "ARLI — Rust-native AI Agent Harness")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Model to use (overrides config/env)
    #[arg(short, long)]
    model: Option<String>,

    /// Max tool-calling iterations
    #[arg(short, long, default_value = "20")]
    iterations: usize,

    /// Resume a specific session by ID
    #[arg(long, conflicts_with = "continue_session")]
    resume: Option<String>,

    /// Resume the most recent session
    #[arg(long, conflicts_with = "resume")]
    continue_session: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive chat (default)
    Chat {
        /// Single query, non-interactive
        #[arg(short = 'q', long)]
        query: Option<String>,
    },

    /// Configure API keys and provider
    Setup {
        /// Use Internet Identity (ICP) for agent profile setup
        #[arg(long)]
        ii: bool,
        /// Agent name for on-chain registration
        #[arg(long, default_value = "ARLI")]
        name: String,
        /// ICP gateway URL
        #[arg(long, default_value = "https://icp0.io")]
        icp_gateway: String,
    },

    /// Manage configuration
    #[command(subcommand)]
    Config(ConfigCmd),

    /// Change model/provider interactively
    Model,

    /// Check system health and configuration
    Doctor,

    /// List recent sessions
    Sessions,

    /// Show version
    Version,

    /// Start health check server
    Serve {
        #[arg(short, long, default_value = "3001")]
        port: u16,
    },

    /// Manage cron jobs
    #[command(subcommand)]
    Cron(CronCmd),

    /// Generate shell completions for bash/zsh/fish
    Completion {
        /// Shell: bash, zsh, or fish
        #[arg(short, long, default_value = "bash")]
        shell: String,
    },

    /// Start MCP server (Model Context Protocol) on stdio
    Mcp,

    /// Manage plugins
    #[command(subcommand)]
    Plugins(PluginsCmd),

    /// Manage profiles
    #[command(subcommand)]
    Profile(ProfileCmd),

    /// Manage webhook subscriptions
    #[command(subcommand)]
    Webhook(WebhookCmd),

    /// Manage checkpoints
    #[command(subcommand)]
    Checkpoint(CheckpointCmd),

    /// Update ARLI to the latest release from GitHub
    Update {
        /// Check for updates without installing
        #[arg(long)]
        check: bool,
    },

    /// Manage the gateway daemon
    #[command(subcommand)]
    Gateway(GatewayCmd),

    /// Manage ARLI cryptographic keys (for ENSO attestation)
    #[command(subcommand)]
    Key(KeyCmd),

    /// Manage kanban task boards
    #[command(subcommand)]
    Kanban(KanbanCmd),

    /// Start the web dashboard
    Dashboard {
        #[arg(short, long, default_value = "3000")]
        port: u16,
    },

    /// Manage ENSO marketplace (RFQ/Quote/Contract)
    #[command(subcommand)]
    Marketplace(MarketplaceCmd),

    /// Manage ENSO integration (setup/status/pay)
    #[command(subcommand)]
    Enso(EnsoCmd),

    /// Manage AI inference brokering (tenants, billing, reports)
    #[command(subcommand)]
    Brokering(BrokeringCmd),

    /// Analyze harness telemetry and experiential memory
    #[command(subcommand)]
    Harness(HarnessCmd),

    /// Start the inference brokering API server
    Api {
        /// Port to listen on
        #[arg(short, long, default_value = "8080")]
        port: u16,
    },
}

#[derive(Subcommand)]
enum KeyCmd {
    /// Generate a new ed25519 keypair for attestation signing
    Generate,
    /// Show the current public key (for ENSO Registry registration)
    Show,
}

#[derive(Subcommand)]
enum GatewayCmd {
    /// Start the gateway as a background daemon
    Start,
    /// Stop the running gateway daemon
    Stop,
    /// Show gateway daemon status (running/stopped)
    Status,
    /// Show recent gateway logs
    Log {
        /// Number of lines to show (default: 20)
        #[arg(short, long, default_value = "20")]
        lines: usize,
    },
}

#[derive(Subcommand)]
enum HarnessCmd {
    /// Analyze harness telemetry and experiential memory
    Analyze,
}

#[derive(Subcommand)]
enum CheckpointCmd {
    /// List checkpoints for current session
    List,
    /// Create a new checkpoint
    Create,
    /// Rollback to last checkpoint
    Rollback,
    /// Prune old checkpoints (keep last N)
    Prune {
        #[arg(default_value = "5")]
        keep: usize,
    },
}

#[derive(Subcommand)]
enum WebhookCmd {
    /// List subscriptions
    List,
    /// Subscribe to a webhook
    Subscribe {
        /// Webhook name (URL path)
        name: String,
        /// Prompt template (use {{payload}} for POST body)
        #[arg(short, long)]
        prompt: String,
    },
    /// Remove a subscription
    Remove { name: String },
    /// Start webhook server
    Serve {
        #[arg(short, long, default_value = "3002")]
        port: u16,
    },
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// List all profiles
    List,
    /// Create a new profile
    Create {
        /// Profile name
        name: String,
        /// Clone config and soul.md from default
        #[arg(long)]
        clone: bool,
    },
    /// Set default profile
    Use {
        /// Profile name (or \"default\" to unset)
        name: String,
    },
    /// Delete a profile
    Delete { name: String },
    /// Show current profile
    Current,
}

#[derive(Subcommand)]
enum PluginsCmd {
    /// List discovered plugins
    List,
    /// Load all enabled plugins (for testing)
    Load,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Show current configuration
    Show,
    /// Print config file path
    Path,
    /// Set a config value: arli config set model deepseek-chat
    Set { key: String, value: String },
}

#[derive(Subcommand)]
enum CronCmd {
    /// Add a new cron job
    Add {
        /// Human-readable name
        #[arg(short, long)]
        name: String,
        /// Cron expression or interval: "5m", "1h", "0 9 * * *"
        #[arg(short, long)]
        schedule: String,
        /// Prompt for the agent to run
        #[arg(short, long)]
        prompt: String,
    },
    /// List all cron jobs
    List,
    /// Remove a cron job by ID
    Remove {
        /// Job ID
        id: String,
    },
    /// Pause a cron job
    Pause { id: String },
    /// Resume a paused cron job
    Resume { id: String },
    /// Start the cron scheduler daemon
    Start,
    /// Run a job immediately (for testing)
    Run { id: String },
}

#[derive(Subcommand)]
enum KanbanCmd {
    /// Create a new kanban board
    Create {
        /// Board name
        name: String,
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Show a board with all columns and cards
    Show {
        /// Board ID (first board if omitted)
        id: Option<String>,
    },
    /// List all boards
    List,
    /// Add a card to a board column
    Add {
        /// Board ID
        board_id: String,
        /// Column name (backlog, todo, in_progress, review, done)
        column: String,
        /// Card title
        title: String,
        #[arg(short, long)]
        description: Option<String>,
        #[arg(short, long, default_value = "medium")]
        priority: String,
    },
    /// Move a card to another column
    Move {
        /// Card ID
        card_id: String,
        /// Target column name
        column: String,
    },
}

#[derive(Subcommand)]
enum MarketplaceCmd {
    /// Create a new RFQ (Request for Quote)
    RfqCreate {
        /// RFQ title
        title: String,
        /// Budget in USD cents
        #[arg(short, long)]
        budget_cents: u64,
        /// Deadline (ISO 8601)
        #[arg(short, long)]
        deadline: String,
        /// Required capabilities (comma-separated)
        #[arg(short, long)]
        capabilities: Option<String>,
    },
    /// List open RFQs
    RfqList {
        /// Filter by status (open, quoted, accepted, contracted)
        #[arg(short, long)]
        status: Option<String>,
    },
    /// Show RFQ details + quotes
    RfqShow {
        /// RFQ ID
        id: String,
    },
    /// Submit a quote to an RFQ
    Quote {
        /// RFQ ID
        rfq_id: String,
        /// Your agent ID
        agent_id: String,
        /// Price in USD cents
        #[arg(short, long)]
        price_cents: u64,
        /// Estimated time in seconds
        #[arg(short, long)]
        time_secs: u64,
    },
    /// Accept a quote (RFQ owner only)
    Accept {
        /// Quote ID
        quote_id: String,
    },
    /// Show marketplace stats
    Stats,
}

#[derive(Subcommand)]
enum EnsoCmd {
    /// One-shot onboarding: keygen → show credentials → ready to run
    Onboard,
    /// One-shot ENSO setup: keygen + register agent + configure canisters
    Setup {
        /// ICP gateway URL (default: https://icp0.io)
        #[arg(long, default_value = "https://icp0.io")]
        icp_gateway: String,
        /// Registry canister ID
        #[arg(long)]
        registry: Option<String>,
        /// Contracts canister ID
        #[arg(long)]
        contracts: Option<String>,
        /// Agent name for registration
        #[arg(long, default_value = "ARLI")]
        name: String,
    },
    /// Show ENSO status: registered agents, active contracts, balance
    Status,
    /// Register this ARLI agent + binary hash with ENSO Registry
    Register {
        /// Capabilities (comma-separated)
        #[arg(long, default_value = "attestation,oracle,sandbox")]
        capabilities: String,
    },
    /// Submit attestation + trigger payment for a contract
    Pay {
        /// Contract ID (e.g. contract_1780372735456935314_4)
        contract_id: String,
    },
    /// Run the ENSO oracle for specific contracts: attest → submit → settle
    #[command(alias = "oracle")]
    Run {
        /// Run once for a specific contract (no polling loop)
        #[arg(short, long)]
        contract: Option<String>,
    },
}

#[derive(Subcommand)]
enum BrokeringCmd {
    /// Tenant management
    Tenant {
        #[command(subcommand)]
        cmd: TenantCmd,
    },
    /// Generate billing reports
    Report {
        /// Tenant ID
        #[arg(long)]
        tenant: String,
        /// Year (default: current)
        #[arg(long)]
        year: Option<i32>,
        /// Month 1-12 (default: current)
        #[arg(long)]
        month: Option<u32>,
    },
}

#[derive(Subcommand)]
enum TenantCmd {
    /// Create a new tenant
    Create {
        /// Tenant name
        name: String,
        /// Contact email
        #[arg(long)]
        email: String,
    },
    /// List all tenants
    List,
    /// Rotate API key for a tenant
    RotateKey {
        /// Tenant ID (UUID)
        tenant_id: String,
    },
    /// Enable a tenant
    Enable { tenant_id: String },
    /// Disable a tenant
    Disable { tenant_id: String },
}

fn get_data_dir() -> PathBuf {
    let base = std::env::var("ARLI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = dirs_next().unwrap_or_else(|| PathBuf::from("."));
            home.join(".arli")
        });

    std::fs::create_dir_all(&base).ok();
    base
}

fn dirs_next() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

fn run_setup_ii(name: &str, icp_gateway: &str) -> anyhow::Result<()> {
    println!("=== ARLI Setup — Internet Identity ===\n");

    // 1. Generate session keypair
    let session_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let session_pubkey_hex = hex::encode(session_key.verifying_key().as_bytes());
    let session_secret_hex = hex::encode(session_key.to_bytes());

    // 2. Find a free port for the callback
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.local_addr()?.port()
    };

    // 3. II auth URL — pass session public key
    let ii_url = format!(
        "https://identity.ic0.app/#authorize?session_public_key={}&redirect_uri=http://localhost:{}/callback&max_time_to_live=3600000000000",
        session_pubkey_hex, port
    );

    println!("Opening browser for Internet Identity authentication...");
    println!("If the browser doesn't open, visit:\n  {}", ii_url);

    let _ = std::process::Command::new(if cfg!(target_os = "macos") { "open" } else { "xdg-open" })
        .arg(&ii_url)
        .spawn();

    // 4. Start callback server — capture delegation AND user_public_key
    let (tx, rx) = std::sync::mpsc::channel::<(String, String)>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            use axum::{routing::get, Router};
            use axum::response::Html;

            let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
            let app = Router::new().route("/callback", get(move |axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>| async move {
                let delegation = params.get("delegation").cloned().unwrap_or_default();
                let user_pubkey = params.get("user_public_key").cloned().unwrap_or_default();
                if let Ok(mut lock) = tx.lock() {
                    if let Some(tx) = lock.take() {
                        let _ = tx.send((delegation, user_pubkey));
                    }
                }
                Html("<html><body><h1>Authenticated!</h1><p>Close this window and return to terminal.</p></body></html>")
            }));

            let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await.unwrap();
            axum::serve(listener, app).await.ok();
        });
    });

    println!("Waiting for II authentication (2 minute timeout)...");

    let (delegation, user_pubkey) = match rx.recv_timeout(std::time::Duration::from_secs(120)) {
        Ok((d, u)) if !d.is_empty() => (d, u),
        Ok(_) => anyhow::bail!("II callback returned empty delegation"),
        Err(_) => anyhow::bail!("Timeout — II authentication took too long"),
    };

    println!("✓ Authenticated with Internet Identity");

    // 5. Call AgentProfile canister via arli-core
    let rt = tokio::runtime::Runtime::new()?;
    let result = rt.block_on(arli_core::agent_profile::complete_ii_setup(
        &delegation,
        &user_pubkey,
        &session_pubkey_hex,
        &session_secret_hex,
        name,
        icp_gateway,
        "bmo7h-iiaaa-aaaab-qhama-cai", // AgentProfile canister on ICP mainnet
    ));

    match result {
        Ok(r) => {
            println!("✓ Agent registered: {}", r.agent_id);

            // 6. Save config
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            let arli_dir = std::path::PathBuf::from(&home).join(".arli");
            std::fs::create_dir_all(&arli_dir)?;

            let config_toml = format!(
                r#"# ARLI configuration — generated by `arli setup --ii`
# Internet Identity: connected
# Agent ID: {agent_id}

[provider]
name = "deepseek"
api_key = ""

[enso]
icp_gateway = "{gateway}"
contracts_canister_id = "7yv6j-ryaaa-aaaaa-qhheq-cai"
agent_name = "{name}"
arli_public_key = "{pubkey}"

[session_reset]
mode = "inactivity_daily"
"#,
                agent_id = r.agent_id,
                gateway = icp_gateway,
                name = name,
                pubkey = r.pubkey_hex,
            );

            let config_path = arli_dir.join("config.toml");
            std::fs::write(&config_path, config_toml)?;
            println!("✓ Config saved: {}", config_path.display());
            println!();
            println!("═══ Setup complete ═══");
            println!("  Agent ID: {}", r.agent_id);
            println!("  Run `arli enso status` to verify.");
        }
        Err(e) => {
            anyhow::bail!("Canister registration failed: {}", e);
        }
    }

    Ok(())
}

fn run_setup() -> anyhow::Result<()> {
    use std::io::{self, Write};

    println!("=== ARLI Setup ===\n");

    // ── 1. Provider selection ──
    println!("Select provider:");
    println!("  1) DeepSeek (V3, R1 — direct API)");
    println!("  2) OpenAI (GPT-4o, o4-mini — api.openai.com)");
    println!("  3) Anthropic (Claude Sonnet 4 — direct API)");
    println!("  4) OpenRouter (200+ models — pay-per-use)");
    println!("  5) Google AI Studio (Gemini 2.5 — native API)");
    println!("  6) xAI (Grok — direct API)");
    println!("  7) GitHub Copilot (uses GITHUB_TOKEN or gh auth)");
    println!("  8) Custom endpoint (enter URL manually)");
    println!("  9) LM Studio (local — http://localhost:1234/v1)");
    println!(" 10) Ollama (local — http://localhost:11434/v1)");
    println!(" 11) HuggingFace (Inference API)");
    println!(" 12) AWS Bedrock (requires LiteLLM proxy)");
    println!(" 13) Azure Foundry (OpenAI-compatible)");
    println!(" 14) OpenAI Codex (CLI agent)");
    println!(" 15) Nous Portal (Hermes-3 — nousresearch.com)");
    println!(" 16) NovitaAI (Llama 3.1 70B — novita.ai)");
    println!(" 17) Qwen Cloud / DashScope (Qwen-Max)");
    println!(" 18) xAI Grok OAuth (uses XAI_OAUTH_TOKEN)");
    println!(" 19) Xiaomi MiMo (MiMo v2.5 Pro)");
    println!(" 20) Tencent TokenHub (HY3 Preview)");
    println!(" 21) NVIDIA NIM (Nemotron-4)");
    println!(" 22) GitHub Copilot ACP (same as Copilot, ACP mode)");
    println!(" 23) Google Gemini OAuth (uses GOOGLE_OAUTH_TOKEN)");
    println!(" 24) Z.AI / GLM (GLM-4 — bigmodel.cn)");
    println!(" 25) Kimi / Moonshot (moonshot-v1-128k)");
    println!(" 26) StepFun (Step-2-16K)");
    println!(" 27) MiniMax Global (abab7 — minimax.chat)");
    println!(" 28) MiniMax via OAuth (uses MINIMAX_OAUTH_TOKEN)");
    println!(" 29) MiniMax China (abab7 — minimaxi.com)");
    println!(" 30) Arcee AI (Trinity)");
    println!(" 31) GMI Cloud (gmi-model)");
    println!(" 32) Kilo Code (kilo-model)");
    println!(" 33) OpenCode Zen (opencode-model)");
    println!(" 34) OpenCode Go (opencode-model)");
    println!(" 35) Alibaba Cloud Coding Plan (Qwen-Coder)");
    println!(" 36) Qwen OAuth (uses QWEN_OAUTH_TOKEN)");
    print!("> ");
    io::stdout().flush()?;

    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    let choice = choice.trim();

    let (provider_name, api_key_env, default_model, base_url_override): (
        &str,
        &str,
        &str,
        Option<&str>,
    ) = match choice {
        "2" => ("openai", "OPENAI_API_KEY", "gpt-4o", None),
        "3" => (
            "anthropic",
            "ANTHROPIC_API_KEY",
            "claude-sonnet-4-20250514",
            None,
        ),
        "4" => ("openrouter", "OPENROUTER_API_KEY", "openai/gpt-4o", None),
        "5" => ("google", "GOOGLE_API_KEY", "gemini-2.5-flash", None),
        "6" => ("xai", "XAI_API_KEY", "grok-4", None),
        "7" => ("copilot", "GITHUB_TOKEN", "gpt-4o", None),
        "9" => (
            "lmstudio",
            "LM_STUDIO_API_KEY",
            "local-model",
            Some("http://localhost:1234/v1"),
        ),
        "10" => (
            "ollama",
            "OLLAMA_API_KEY",
            "llama3",
            Some("http://localhost:11434/v1"),
        ),
        "11" => (
            "huggingface",
            "HF_API_KEY",
            "meta-llama/Meta-Llama-3-70B-Instruct",
            None,
        ),
        "12" => (
            "bedrock",
            "AWS_ACCESS_KEY_ID",
            "us.anthropic.claude-sonnet-4-20250514-v1:0",
            None,
        ),
        "13" => ("azure", "AZURE_API_KEY", "gpt-4o", None),
        "14" => ("codex", "OPENAI_CODEX_API_KEY", "gpt-4o", None),
        "15" => ("nous", "NOUS_API_KEY", "hermes-3", None),
        "16" => ("novita", "NOVITA_API_KEY", "novita-llama-3.1-70b", None),
        "17" => ("qwen", "DASHSCOPE_API_KEY", "qwen-max", None),
        "18" => ("xai", "XAI_OAUTH_TOKEN", "grok-4", None),
        "19" => ("mimo", "MIMO_API_KEY", "mimo-v2.5-pro", None),
        "20" => ("tencent", "TENCENT_API_KEY", "hy3-preview", None),
        "21" => ("nvidia", "NVIDIA_API_KEY", "nemotron-4-340b", None),
        "22" => ("copilot", "GITHUB_TOKEN", "gpt-4o", None),
        "23" => ("google", "GOOGLE_OAUTH_TOKEN", "gemini-2.5-flash", None),
        "24" => ("zhipu", "ZHIPU_API_KEY", "glm-4", None),
        "25" => ("moonshot", "MOONSHOT_API_KEY", "moonshot-v1-128k", None),
        "26" => ("stepfun", "STEPFUN_API_KEY", "step-2-16k", None),
        "27" => ("minimax", "MINIMAX_API_KEY", "abab7", None),
        "28" => ("minimax", "MINIMAX_OAUTH_TOKEN", "abab7", None),
        "29" => ("minimax_cn", "MINIMAX_CN_API_KEY", "abab7", None),
        "30" => ("arcee", "ARCEE_API_KEY", "trinity", None),
        "31" => ("gmi", "GMI_API_KEY", "gmi-model", None),
        "32" => ("kilo", "KILO_API_KEY", "kilo-model", None),
        "33" => ("opencode", "OPENCODE_API_KEY", "opencode-model", None),
        "34" => ("opencode_go", "OPENCODE_GO_API_KEY", "opencode-model", None),
        "35" => ("alibaba_cloud", "ALIBABA_CLOUD_API_KEY", "qwen-coder", None),
        "36" => ("qwen", "QWEN_OAUTH_TOKEN", "qwen-max", None),
        "8" => {
            print!("\nProvider name (e.g. 'my-provider'): ");
            io::stdout().flush()?;
            let mut name = String::new();
            io::stdin().read_line(&mut name)?;
            let name = name.trim().to_string();

            print!("Base URL (OpenAI-compatible, e.g. https://api.example.com/v1): ");
            io::stdout().flush()?;
            let mut url = String::new();
            io::stdin().read_line(&mut url)?;
            let url = url.trim().to_string();

            print!("Default model: ");
            io::stdout().flush()?;
            let mut model = String::new();
            io::stdin().read_line(&mut model)?;

            let (name, url, model) = (name, url, model.trim().to_string());

            print!("\nAPI key (input hidden): ");
            io::stdout().flush()?;
            let api_key = rpassword::read_password()?;
            if api_key.is_empty() {
                anyhow::bail!("API key is required");
            }

            // ── Max iterations, tool progress, compression ──
            print!("Max tool-calling iterations [90]: ");
            io::stdout().flush()?;
            let mut iters = String::new();
            io::stdin().read_line(&mut iters)?;
            let max_iterations: usize = iters.trim().parse().unwrap_or(90);

            println!("Tool progress mode:");
            println!("  off     — Silent, just the final response");
            println!("  new     — Show tool name only when it changes (less noise)");
            println!("  all     — Show every tool call with a short preview");
            println!("  verbose — Full args, results, and debug logs");
            print!("Tool progress mode (off/new/all/verbose) [all]: ");
            io::stdout().flush()?;
            let mut tp = String::new();
            io::stdin().read_line(&mut tp)?;
            let tool_progress = if tp.trim().is_empty() {
                "all".to_string()
            } else {
                tp.trim().to_string()
            };

            print!("Compression threshold (0.5-0.95) [0.5]: ");
            io::stdout().flush()?;
            let mut ct = String::new();
            io::stdin().read_line(&mut ct)?;
            let compression_threshold: f32 = ct.trim().parse().unwrap_or(0.5);

            let data_dir = get_data_dir();
            let config_path = data_dir.join("config.toml");
            let config_content = format!(
                "# ARLI Configuration\n# Generated by `arli setup`\n\n\
                 model = \"{}\"\nmax_iterations = {}\ntool_progress = \"{}\"\ncompression_threshold = {}\n\n\
                 [provider]\nname = \"{}\"\napi_key = \"{}\"\nbase_url = \"{}\"\n\
                 \n[session_reset]\nmode = \"inactivity_daily\"\ninactivity_minutes = 1440\ndaily_reset_hour = 4\n",
                model, max_iterations, tool_progress, compression_threshold, name, api_key, url
            );
            std::fs::write(&config_path, config_content)?;
            run_setup_finish(&name, &model, &api_key, None)?;
            return Ok(());
        }
        _ => ("deepseek", "DEEPSEEK_API_KEY", "deepseek-chat", None),
    };

    // ── 2. API key ──
    print!("\n{} API key (input hidden): ", api_key_env);
    io::stdout().flush()?;
    let api_key = rpassword::read_password()?;

    if api_key.is_empty() {
        anyhow::bail!("API key is required");
    }

    // ── 3. Model ──
    print!("Model [{}]: ", default_model);
    io::stdout().flush()?;
    let mut model = String::new();
    io::stdin().read_line(&mut model)?;
    let model = if model.trim().is_empty() {
        default_model.to_string()
    } else {
        model.trim().to_string()
    };

    // ── 4. Max iterations ──
    print!("Max tool-calling iterations [90]: ");
    io::stdout().flush()?;
    let mut iters = String::new();
    io::stdin().read_line(&mut iters)?;
    let max_iterations: usize = iters.trim().parse().unwrap_or(90);

    // ── 5. Tool progress mode ──
    println!("Tool progress mode:");
    println!("  off     — Silent, just the final response");
    println!("  new     — Show tool name only when it changes (less noise)");
    println!("  all     — Show every tool call with a short preview");
    println!("  verbose — Full args, results, and debug logs");
    print!("Tool progress mode (off/new/all/verbose) [all]: ");
    io::stdout().flush()?;
    let mut tool_progress = String::new();
    io::stdin().read_line(&mut tool_progress)?;
    let tool_progress = if tool_progress.trim().is_empty() {
        "all".to_string()
    } else {
        tool_progress.trim().to_string()
    };

    // ── 6. Compression threshold ──
    print!("Compression threshold (0.5-0.95) [0.5]: ");
    io::stdout().flush()?;
    let mut comp_thresh = String::new();
    io::stdin().read_line(&mut comp_thresh)?;
    let compression_threshold: f32 = comp_thresh.trim().parse().unwrap_or(0.5);

    // ── 7. Session reset policy ──
    println!();
    println!("Session reset policy (controls when agent auto-resets its session):");
    println!("  1) inactivity_daily — reset on inactivity OR at daily hour (recommended)");
    println!("  2) inactivity — reset after N minutes of no messages");
    println!("  3) daily — reset at a fixed hour each day");
    println!("  4) never — never auto-reset");
    print!("Choose [1]: ");
    io::stdout().flush()?;
    let mut reset_choice = String::new();
    io::stdin().read_line(&mut reset_choice)?;
    let reset_mode = match reset_choice.trim() {
        "2" => "inactivity",
        "3" => "daily",
        "4" => "never",
        _ => "inactivity_daily",
    };

    let mut inactivity_minutes: u32 = 1440;
    let mut daily_reset_hour: u8 = 4;

    if reset_mode == "inactivity" || reset_mode == "inactivity_daily" {
        print!("Inactivity timeout in minutes [1440]: ");
        io::stdout().flush()?;
        let mut mins = String::new();
        io::stdin().read_line(&mut mins)?;
        inactivity_minutes = mins.trim().parse().unwrap_or(1440);
    }

    if reset_mode == "daily" || reset_mode == "inactivity_daily" {
        print!("Daily reset hour (0-23) [4]: ");
        io::stdout().flush()?;
        let mut hour = String::new();
        io::stdin().read_line(&mut hour)?;
        daily_reset_hour = hour.trim().parse().unwrap_or(4).min(23);
    }

    let session_reset_section = format!(
        "\n[session_reset]\nmode = \"{}\"\ninactivity_minutes = {}\ndaily_reset_hour = {}\n",
        reset_mode, inactivity_minutes, daily_reset_hour
    );

    // ── 8. Search provider ──
    println!();
    println!("Search provider:");
    println!("  1) duckduckgo — Free, no API key needed (default)");
    println!("  2) brave      — Free tier API key, 2k queries/mo");
    println!("  3) searxng    — Self-hosted, privacy-respecting metasearch");
    println!("  4) tavily     — Paid, search + extract in one");
    println!("  5) firecrawl  — Paid, full search + content extraction");
    println!("  6) exa        — Semantic + neural web search");
    println!("  7) parallel   — Objective-tuned search + parallel extraction");
    println!("  8) xai        — Agentic web search via Grok (needs XAI_API_KEY)");
    print!("Search provider [duckduckgo]: ");
    io::stdout().flush()?;
    let mut sr_choice = String::new();
    io::stdin().read_line(&mut sr_choice)?;
    let search_provider = match sr_choice.trim() {
        "2" => "brave",
        "3" => "searxng",
        "4" => "tavily",
        "5" => "firecrawl",
        "6" => "exa",
        "7" => "parallel",
        "8" => "xai",
        _ => "duckduckgo",
    };

    let search_section = if search_provider == "duckduckgo" {
        String::new()
    } else {
        format!("\n[search]\nprovider = \"{}\"\n", search_provider)
    };

    // ── 9. Memory provider ──
    println!();
    println!("Memory provider:");
    println!("  1)  builtin     — SQLite local store (default, no config needed)");
    println!("  2)  mem0        — Cloud-hosted memory API (mem0.ai)");
    println!("  3)  chroma      — Local ChromaDB vector database");
    println!("  4)  qdrant      — Local/cloud Qdrant vector database");
    println!("  5)  byterover   — Cloud-hosted memory (bytabox.ai, needs API key)");
    println!("  6)  hindsight   — Local/cloud memory store");
    println!("  7)  holographic — Local holographic memory store");
    println!("  8)  honcho      — Local/cloud memory for AI agents");
    println!("  9)  openviking  — API key / local memory store");
    println!("  10) retaindb    — API key / local vector memory");
    println!("  11) supermemory — Cloud memory API (supermemory.ai, needs API key)");
    println!("  12) agentmemory — API key / local memory for agents");
    print!("Memory provider [builtin]: ");
    io::stdout().flush()?;
    let mut mem_choice = String::new();
    io::stdin().read_line(&mut mem_choice)?;
    let memory_provider = match mem_choice.trim() {
        "2" => "mem0",
        "3" => "chroma",
        "4" => "qdrant",
        "5" => "byterover",
        "6" => "hindsight",
        "7" => "holographic",
        "8" => "honcho",
        "9" => "openviking",
        "10" => "retaindb",
        "11" => "supermemory",
        "12" => "agentmemory",
        _ => "builtin",
    };

    let memory_section = if memory_provider == "builtin" {
        String::new()
    } else {
        format!("\n[memory]\nprovider = \"{}\"\n", memory_provider)
    };

    // ── 9. Terminal backend ──
    println!();
    println!("Terminal backend (for tool execution):");
    println!("  local       — Run commands directly on this machine (default)");
    println!("  docker      — Run commands in a Docker container");
    println!("  ssh         — Run commands on a remote host via SSH");
    println!("  modal       — Run commands on Modal cloud GPUs");
    println!("  daytona     — Persistent cloud development environment");
    println!("  singularity — HPC-friendly container (Apptainer)");
    print!("Terminal backend [local]: ");
    io::stdout().flush()?;
    let mut tb_choice = String::new();
    io::stdin().read_line(&mut tb_choice)?;
    let terminal_backend = match tb_choice.trim() {
        "docker" => "docker",
        "ssh" => "ssh",
        "modal" => "modal",
        "daytona" => "daytona",
        "singularity" => "singularity",
        _ => "local",
    };

    let terminal_section = if terminal_backend == "local" {
        String::new()
    } else {
        format!("\n[terminal]\nbackend = \"{}\"\n", terminal_backend)
    };

    // ── 9b. Browser provider ──
    println!();
    println!("Browser provider:");
    println!("  local       — Headless Chromium on this machine (default)");
    println!("  camofox     — Firefox/Camoufox anti-detection browser");
    println!("  browserbase — Browserbase cloud browser (needs API key)");
    println!("  firecrawl   — Firecrawl cloud browser (needs API key)");
    println!("  browseruse  — Browser Use cloud browser (needs API key)");
    print!("Browser provider [local]: ");
    io::stdout().flush()?;
    let mut br_choice = String::new();
    io::stdin().read_line(&mut br_choice)?;
    let browser_provider = match br_choice.trim() {
        "camofox" => "camofox",
        "browserbase" => "browserbase",
        "firecrawl" => "firecrawl",
        "browseruse" => "browseruse",
        _ => "local",
    };

    let browser_section = if browser_provider == "local" {
        String::new()
    } else {
        format!("\n[browser]\nprovider = \"{}\"\n", browser_provider)
    };

    // ── 10. x402 agentic wallet ──
    println!();
    println!("x402 agentic wallet (for premium tools like Nansen, Exa, etc.)?");
    println!("  Seed a wallet with $5-10 USDC. Each tool call costs a few cents.");
    println!("  This avoids subscribing to every API individually.");
    let mut x402_enable = String::new();
    print!("Enable x402? [skip]: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut x402_enable)?;
    let x402_enable = x402_enable.trim().eq_ignore_ascii_case("y")
        || x402_enable.trim().eq_ignore_ascii_case("yes");

    let x402_section = if x402_enable {
        print!("Wallet address (hex): ");
        io::stdout().flush()?;
        let mut wallet_address = String::new();
        io::stdin().read_line(&mut wallet_address)?;
        let wallet_address = wallet_address.trim().to_string();

        print!("Private key (input hidden): ");
        io::stdout().flush()?;
        let private_key = rpassword::read_password()?;

        print!("RPC URL (base chain for USDC): ");
        io::stdout().flush()?;
        let mut rpc_url = String::new();
        io::stdin().read_line(&mut rpc_url)?;
        let rpc_url = rpc_url.trim().to_string();

        print!("Total budget in USDC cents [1000 = $10.00]: ");
        io::stdout().flush()?;
        let mut budget = String::new();
        io::stdin().read_line(&mut budget)?;
        let total_budget_cents: u64 = budget.trim().parse().unwrap_or(1000);

        print!("Max spend per call in cents [50 = $0.50]: ");
        io::stdout().flush()?;
        let mut max_spend = String::new();
        io::stdin().read_line(&mut max_spend)?;
        let max_spend_per_call_cents: u64 = max_spend.trim().parse().unwrap_or(50);

        format!(
            "[x402]\nenabled = true\nwallet_address = \"{}\"\nprivate_key = \"{}\"\nrpc_url = \"{}\"\ntotal_budget_cents = {}\nmax_spend_per_call_cents = {}\n",
            wallet_address,
            private_key,
            rpc_url,
            total_budget_cents,
            max_spend_per_call_cents,
        )
    } else {
        String::new()
    };

    // ── 11. Write config ──
    let data_dir = get_data_dir();
    let config_path = data_dir.join("config.toml");

    let mut config_content = format!(
        "# ARLI Configuration\n# Generated by `arli setup`\n\n\
         model = \"{}\"\nmax_iterations = {}\ntool_progress = \"{}\"\ncompression_threshold = {}\n\n\
         [provider]\nname = \"{}\"\napi_key = \"{}\"\n",
        model, max_iterations, tool_progress, compression_threshold, provider_name, api_key
    );
    if let Some(url) = base_url_override {
        config_content.push_str(&format!("base_url = \"{}\"\n", url));
    }
    config_content.push_str(&session_reset_section);
    config_content.push_str(&search_section);
    config_content.push_str(&memory_section);
    config_content.push_str(&terminal_section);
    config_content.push_str(&browser_section);
    config_content.push_str(&x402_section);
    std::fs::write(&config_path, config_content)?;

    // ── 11. Telegram / Gateway ──
    run_setup_finish(provider_name, &model, &api_key, base_url_override)?;

    Ok(())
}

fn run_setup_finish(
    provider_name: &str,
    _model: &str,
    _api_key: &str,
    _base_url: Option<&str>,
) -> anyhow::Result<()> {
    use std::io::{self, Write};

    let data_dir = get_data_dir();
    let config_path = data_dir.join("config.toml");

    // ── Telegram config ──
    println!();
    let mut telegram_token = String::new();
    print!("Telegram bot token (optional, Enter to skip): ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut telegram_token)?;
    let telegram_token = telegram_token.trim().to_string();

    if !telegram_token.is_empty() {
        let gateway_config = format!("\n[gateway]\ntelegram_token = \"{}\"\n", telegram_token);
        std::fs::write(
            &config_path,
            std::fs::read_to_string(&config_path)? + &gateway_config,
        )?;
    }

    println!("\nConfiguration saved to {}", config_path.display());
    println!(
        "Or set environment variable: export {}_API_KEY=\"...\"",
        provider_name.to_uppercase()
    );

    if !telegram_token.is_empty() {
        println!("\nGateway supports multiple platforms:");
        println!("  Telegram  — set with --token or env TELEGRAM_BOT_TOKEN");
        println!("  Discord   — env DISCORD_BOT_TOKEN");
        println!("  Slack     — env SLACK_BOT_TOKEN + SLACK_APP_TOKEN");
        println!("  WhatsApp  — env WHATSAPP_PHONE_ID + WHATSAPP_TOKEN");
        println!("  Matrix    — env MATRIX_USER + MATRIX_PASSWORD");
        println!("  Microsoft Teams — env MS_TEAMS_APP_ID + MS_TEAMS_APP_PASSWORD");
        println!("  Email     — env EMAIL_IMAP_SERVER + EMAIL_USER + EMAIL_PASSWORD");
        println!("\nTo start daemon:  arli gateway start");
        println!("To run foreground: arli-gateway");

        // Check for ambiguous service state (both user + system installed)
        let user_svc = dirs_next()
            .map(|h| h.join(".config/systemd/user/arli-gateway.service"))
            .map(|p| p.exists())
            .unwrap_or(false);
        let system_svc = std::path::Path::new("/etc/systemd/system/arli-gateway.service").exists();
        if user_svc && system_svc {
            println!();
            println!("⚠ Both user and system gateway services are installed (user + system).");
            println!("  This can make start/stop/status behavior ambiguous.");
            println!("  Default gateway commands target the user service.");
            println!("  Keep one:");
            println!("    arli gateway uninstall");
            println!("    sudo arli gateway uninstall --system");
        }

        // Offer to install as systemd service
        println!();
        println!("Install gateway as background service?");
        println!("  1) User service (no sudo; starts on login — recommended)");
        println!("  2) System service (starts on boot; requires sudo; runs as your user)");
        println!("  3) Skip service install for now");
        print!("> ");
        io::stdout().flush()?;
        let mut svc = String::new();
        io::stdin().read_line(&mut svc)?;
        match svc.trim() {
            "1" => {
                install_gateway_user_service()?;
            }
            "2" => {
                install_gateway_system_service()?;
            }
            _ => {}
        }

        // Offer to start daemon immediately
        let mut start_daemon = String::new();
        print!("\nStart gateway daemon now? [y/N]: ");
        io::stdout().flush()?;
        io::stdin().read_line(&mut start_daemon)?;
        if start_daemon.trim().eq_ignore_ascii_case("y") {
            let status = std::process::Command::new("arli")
                .arg("gateway")
                .arg("start")
                .status();
            match status {
                Ok(s) if s.success() => println!("Gateway daemon started."),
                Ok(s) => eprintln!(
                    "Gateway start failed (exit: {:?}). Run 'arli gateway start' manually.",
                    s.code()
                ),
                Err(e) => eprintln!(
                    "Could not start gateway: {}. Run 'arli gateway start' manually.",
                    e
                ),
            }
        }
    }

    println!("\nYou're ready. Run: arli chat");
    Ok(())
}

fn install_gateway_user_service() -> anyhow::Result<()> {
    let service_dir = dirs_next()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?
        .join(".config/systemd/user");
    std::fs::create_dir_all(&service_dir)?;

    let data_dir = get_data_dir();
    let pid_file = data_dir.join("gateway.pid");
    let log_file = data_dir.join("gateway.log");

    let service_content = format!(
        "[Unit]\n\
         Description=ARLI Gateway — Multi-platform AI agent bridge\n\
         After=network.target\n\n\
         [Service]\n\
         Type=simple\n\
         WorkingDirectory={}\n\
         EnvironmentFile={}/gateway.env\n\
         Environment=ARLI_HOME={}\n\
         Environment=ARLI_LOG=info,arli_gateway=debug\n\
         ExecStart={}/arli-gateway --daemon --pid-file {} --log-file {}\n\
         Restart=always\n\
         RestartSec=10\n\
         StandardOutput=journal\n\
         StandardError=journal\n\n\
         [Install]\n\
         WantedBy=default.target\n",
        data_dir.display(),
        data_dir.display(),
        data_dir.display(),
        std::env::current_exe()
            .unwrap_or_else(|_| std::path::PathBuf::from("arli"))
            .parent()
            .unwrap_or_else(|| std::path::Path::new("/usr/local/bin"))
            .display(),
        pid_file.display(),
        log_file.display(),
    );

    let service_path = service_dir.join("arli-gateway.service");
    std::fs::write(&service_path, service_content)?;
    println!("User service installed: {}", service_path.display());
    println!("Enable with: systemctl --user enable --now arli-gateway");

    // Check for ambiguity — system service also installed?
    if std::path::Path::new("/etc/systemd/system/arli-gateway.service").exists() {
        println!();
        println!("⚠ System service also detected at /etc/systemd/system/arli-gateway.service");
        println!("  Having both can cause confusion. Remove one:");
        println!(
            "    sudo rm /etc/systemd/system/arli-gateway.service && sudo systemctl daemon-reload"
        );
        println!("  Or: sudo arli gateway uninstall --system");
    }

    Ok(())
}

fn install_gateway_system_service() -> anyhow::Result<()> {
    let data_dir = get_data_dir();
    let pid_file = data_dir.join("gateway.pid");
    let log_file = data_dir.join("gateway.log");

    let service_content = format!(
        "[Unit]\n\
         Description=ARLI Gateway — Multi-platform AI agent bridge\n\
         After=network.target\n\n\
         [Service]\n\
         Type=simple\n\
         User={}\n\
         WorkingDirectory={}\n\
         EnvironmentFile={}/gateway.env\n\
         Environment=ARLI_HOME={}\n\
         Environment=ARLI_LOG=info,arli_gateway=debug\n\
         ExecStart={}/arli-gateway --daemon --pid-file {} --log-file {}\n\
         Restart=always\n\
         RestartSec=10\n\
         StandardOutput=journal\n\
         StandardError=journal\n\n\
         NoNewPrivileges=yes\n\
         PrivateTmp=yes\n\n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        std::env::var("USER").unwrap_or_else(|_| "arli".to_string()),
        data_dir.display(),
        data_dir.display(),
        data_dir.display(),
        std::env::current_exe()
            .unwrap_or_else(|_| std::path::PathBuf::from("arli"))
            .parent()
            .unwrap_or_else(|| std::path::Path::new("/usr/local/bin"))
            .display(),
        pid_file.display(),
        log_file.display(),
    );

    let tmp_path = data_dir.join("arli-gateway.service");
    std::fs::write(&tmp_path, service_content)?;

    println!("System service template written to {}", tmp_path.display());
    println!("To install (requires sudo):");
    println!(
        "  sudo cp {} /etc/systemd/system/arli-gateway.service",
        tmp_path.display()
    );
    println!("  sudo systemctl daemon-reload");
    println!("  sudo systemctl enable --now arli-gateway");

    // Check for ambiguity — user service also installed?
    if let Some(user_dir) = dirs_next() {
        let user_svc = user_dir.join(".config/systemd/user/arli-gateway.service");
        if user_svc.exists() {
            println!();
            println!("⚠ User service also detected at {}", user_svc.display());
            println!("  Having both can cause confusion. Remove one:");
            println!("    rm {}", user_svc.display());
        }
    }

    Ok(())
}
fn run_config(cmd: ConfigCmd) -> anyhow::Result<()> {
    let config_path = get_data_dir().join("config.toml");

    match cmd {
        ConfigCmd::Show => {
            if config_path.exists() {
                let content = std::fs::read_to_string(&config_path)?;
                println!("{}", content);
            } else {
                println!("No config file found at {}", config_path.display());
                println!("Run 'arli setup' to create one.");
            }
        }
        ConfigCmd::Path => {
            println!("{}", config_path.display());
        }
        ConfigCmd::Set { key, value } => {
            if !config_path.exists() {
                anyhow::bail!("No config file. Run 'arli setup' first.");
            }
            let content = std::fs::read_to_string(&config_path)?;
            let mut doc: toml::Value = toml::from_str(&content)?;

            // Support dotted keys: model, provider.name, gateway.bot_token
            let parts: Vec<&str> = key.split('.').collect();
            match parts.len() {
                1 => {
                    doc.as_table_mut()
                        .ok_or_else(|| anyhow::anyhow!("Invalid config"))?
                        .insert(key.clone(), toml::Value::String(value.clone()));
                }
                2 => {
                    let section = doc
                        .as_table_mut()
                        .ok_or_else(|| anyhow::anyhow!("Invalid config"))?
                        .entry(parts[0].to_string())
                        .or_insert(toml::Value::Table(toml::Table::new()));
                    section
                        .as_table_mut()
                        .ok_or_else(|| anyhow::anyhow!("Invalid section"))?
                        .insert(parts[1].to_string(), toml::Value::String(value.clone()));
                }
                _ => anyhow::bail!("Key must be 'key' or 'section.key'"),
            }

            std::fs::write(&config_path, toml::to_string_pretty(&doc)?)?;
            println!("Set {} = {}", key, value);
        }
    }
    Ok(())
}

fn run_model() -> anyhow::Result<()> {
    use std::io::{self, Write};

    // Show current config
    let config = Config::from_env().ok();
    if let Some(ref c) = config {
        println!("Current: model={} via {}", c.model, c.provider.name);
    }
    println!();

    println!("Choose provider:");
    println!("  1) DeepSeek (deepseek-chat)");
    println!("  2) OpenAI (gpt-4o)");
    println!("  3) Anthropic (claude-sonnet-4)");
    println!("  4) OpenRouter (200+ models: openai/gpt-4o, anthropic/claude-*, etc.)");
    print!("> ");
    io::stdout().flush()?;

    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;

    let (provider_name, model) = match choice.trim() {
        "2" => ("openai", "gpt-4o"),
        "3" => ("anthropic", "claude-sonnet-4-20250514"),
        "4" => ("openrouter", "openai/gpt-4o"),
        _ => ("deepseek", "deepseek-chat"),
    };

    // Update config.toml
    let config_path = get_data_dir().join("config.toml");
    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        let mut doc: toml::Value = toml::from_str(&content)?;

        doc.as_table_mut()
            .unwrap()
            .insert("model".into(), toml::Value::String(model.into()));
        if let Some(prov) = doc.get_mut("provider") {
            if let Some(t) = prov.as_table_mut() {
                t.insert("name".into(), toml::Value::String(provider_name.into()));
            }
        }

        std::fs::write(&config_path, toml::to_string_pretty(&doc)?)?;
        println!("Switched to {} via {}", model, provider_name);
    } else {
        anyhow::bail!("No config file. Run 'arli setup' first.");
    }

    Ok(())
}

fn run_doctor() -> anyhow::Result<()> {
    println!("=== ARLI Doctor ===\n");

    // 1. Check config
    let config_path = get_data_dir().join("config.toml");
    if config_path.exists() {
        println!("[OK] Config: {}", config_path.display());
    } else {
        println!("[MISSING] Config: {}", config_path.display());
        println!("  Run 'arli setup'");
    }

    // 2. Check API key
    match Config::from_env() {
        Ok(c) => println!("[OK] Provider: {} (model: {})", c.provider.name, c.model),
        Err(e) => println!("[FAIL] Provider: {}", e),
    }

    // 3. Check data dir
    let data_dir = get_data_dir();
    std::fs::create_dir_all(&data_dir).ok();
    println!("[OK] Data dir: {}", data_dir.display());

    // 4. Check session DB
    let db_path = data_dir.join("sessions.db");
    match SessionStore::open(db_path) {
        Ok(_) => println!("[OK] Session DB"),
        Err(e) => println!("[FAIL] Session DB: {}", e),
    }

    // 5. Check Rust version
    println!(
        "[OK] Rust: {}",
        option_env!("CARGO_PKG_RUST_VERSION").unwrap_or("unknown")
    );

    // 6. Check binary
    let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("arli"));
    if bin.exists() {
        let size = std::fs::metadata(&bin).map(|m| m.len()).unwrap_or(0);
        println!(
            "[OK] Binary: {} ({:.1}MB)",
            bin.display(),
            size as f64 / 1_048_576.0
        );
    }

    // 7. soul.md
    let soul_path = data_dir.join("soul.md");
    if soul_path.exists() {
        let size = std::fs::metadata(&soul_path).map(|m| m.len()).unwrap_or(0);
        println!("[OK] Identity: soul.md ({:.1}KB)", size as f64 / 1024.0);
    } else {
        println!("[INFO] Identity: no soul.md (using built-in)");
        println!("  Create ~/.arli/soul.md to customize agent personality");
    }

    println!("\nAll checks passed.");
    Ok(())
}

// --- Cron job management ---

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CronJobFile {
    id: String,
    name: String,
    schedule: String,
    prompt: String,
    enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CronFile {
    jobs: Vec<CronJobFile>,
}

use serde::{Deserialize, Serialize};

fn cron_path() -> PathBuf {
    get_data_dir().join("cron.toml")
}

fn load_cron() -> anyhow::Result<CronFile> {
    let path = cron_path();
    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        Ok(toml::from_str(&content)?)
    } else {
        Ok(CronFile { jobs: vec![] })
    }
}

fn save_cron(cf: &CronFile) -> anyhow::Result<()> {
    let path = cron_path();
    std::fs::write(&path, toml::to_string_pretty(cf)?)?;
    Ok(())
}

async fn run_cron(cmd: CronCmd) -> anyhow::Result<()> {
    match cmd {
        CronCmd::Add {
            name,
            schedule,
            prompt,
        } => {
            let mut cf = load_cron()?;
            let id = ulid::Ulid::new().to_string();
            let id_trunc = &id[..8];
            cf.jobs.push(CronJobFile {
                id: id.clone(),
                name: name.clone(),
                schedule: schedule.clone(),
                prompt: prompt.clone(),
                enabled: true,
            });
            save_cron(&cf)?;
            println!("Cron job added: {} ({})", id_trunc, name);
            println!("  Schedule: {}", schedule);
            println!("  Start scheduler: arli cron start");
        }
        CronCmd::List => {
            let cf = load_cron()?;
            if cf.jobs.is_empty() {
                println!("No cron jobs.\nAdd one: arli cron add -n 'my job' -s 5m -p 'hello'");
            } else {
                println!("Cron jobs:\n");
                for j in &cf.jobs {
                    let status = if j.enabled { "▶" } else { "⏸" };
                    println!(
                        "  {} {:.8}  {}  {}  {}",
                        status, j.id, j.schedule, j.name, j.prompt,
                    );
                }
                println!("\nStart: arli cron start");
            }
        }
        CronCmd::Remove { id } => {
            let mut cf = load_cron()?;
            let len_before = cf.jobs.len();
            cf.jobs.retain(|j| !j.id.starts_with(&id));
            if cf.jobs.len() == len_before {
                anyhow::bail!("Job not found: {}", id);
            }
            save_cron(&cf)?;
            println!("Job removed: {}", id);
        }
        CronCmd::Pause { id } => {
            let mut cf = load_cron()?;
            let found = cf.jobs.iter_mut().find(|j| j.id.starts_with(&id));
            match found {
                Some(j) => {
                    j.enabled = false;
                    save_cron(&cf)?;
                    println!("Job paused: {}", id);
                }
                None => anyhow::bail!("Job not found: {}", id),
            }
        }
        CronCmd::Resume { id } => {
            let mut cf = load_cron()?;
            let found = cf.jobs.iter_mut().find(|j| j.id.starts_with(&id));
            match found {
                Some(j) => {
                    j.enabled = true;
                    save_cron(&cf)?;
                    println!("Job resumed: {}", id);
                }
                None => anyhow::bail!("Job not found: {}", id),
            }
        }
        CronCmd::Start => {
            let cf = load_cron()?;
            if cf.jobs.is_empty() {
                anyhow::bail!(
                    "No cron jobs. Add one first: arli cron add -n 'job' -s 5m -p 'prompt'"
                );
            }
            println!("Starting cron scheduler with {} jobs…", cf.jobs.len());
            let scheduler = CronScheduler::new();

            for j in &cf.jobs {
                if !j.enabled {
                    continue;
                }
                let job = CronJob {
                    id: j.id.clone(),
                    name: j.name.clone(),
                    schedule_str: j.schedule.clone(),
                    prompt: j.prompt.clone(),
                    deliver: None,
                    skills: vec![],
                    enabled: j.enabled,
                    created_at: chrono::Utc::now(),
                    last_run_at: None,
                    next_run_at: None,
                    run_count: 0,
                    error_count: 0,
                };
                scheduler.add_job(job).await?;
            }

            // Subscribe to events and print them
            let mut rx = scheduler.subscribe();
            tokio::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    match event {
                        CronEvent::JobRunning { job_id } => {
                            println!(
                                "[{}] Running job {}",
                                chrono::Utc::now().format("%H:%M:%S"),
                                &job_id[..8]
                            );
                        }
                        CronEvent::JobCompleted { job_id, output } => {
                            println!(
                                "[{}] Job {} done: {}",
                                chrono::Utc::now().format("%H:%M:%S"),
                                &job_id[..8],
                                output
                            );
                        }
                        CronEvent::JobFailed { job_id, error } => {
                            eprintln!(
                                "[{}] Job {} FAILED: {}",
                                chrono::Utc::now().format("%H:%M:%S"),
                                &job_id[..8],
                                error
                            );
                        }
                        _ => {}
                    }
                }
            });

            println!("Scheduler running. Press Ctrl+C to stop.");
            // Wait forever (scheduler runs in background tasks)
            tokio::signal::ctrl_c().await?;
            println!("\nScheduler stopped.");
        }
        CronCmd::Run { id } => {
            let cf = load_cron()?;
            let job = cf
                .jobs
                .iter()
                .find(|j| j.id.starts_with(&id))
                .ok_or_else(|| anyhow::anyhow!("Job not found: {}", id))?;
            println!("Running job: {} ({})", &job.id[..8], job.name);
            // Use the cron module's execution (currently a placeholder)
            match arli_core::cron::execute_job(&job.id, &job.prompt).await {
                Ok(output) => println!("Output: {}", output),
                Err(e) => eprintln!("Error: {}", e),
            }
        }
    }
    Ok(())
}

fn run_gateway(cmd: GatewayCmd) -> anyhow::Result<()> {
    let data_dir = get_data_dir();
    let pid_file = data_dir.join("gateway.pid");
    let log_file = data_dir.join("gateway.log");

    // Find the gateway binary — prefer release build next to arli, then PATH
    let gateway_bin = {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("arli"));
        let sibling = exe.parent().map(|p| p.join("arli-gateway"));
        sibling
            .filter(|p| p.exists())
            .unwrap_or_else(|| PathBuf::from("arli-gateway"))
    };

    match cmd {
        GatewayCmd::Start => {
            if pid_file.exists() {
                let pid_str = std::fs::read_to_string(&pid_file)?;
                if let Ok(pid) = pid_str.trim().parse::<i32>() {
                    // Check if process is still running
                    unsafe {
                        if libc::kill(pid, 0) == 0 {
                            println!("Gateway is already running (PID: {})", pid);
                            println!("Stop it first: arli gateway stop");
                            return Ok(());
                        }
                    }
                }
                // PID file exists but process is dead — remove stale PID
                std::fs::remove_file(&pid_file)?;
            }

            if !gateway_bin.exists() {
                anyhow::bail!(
                    "Gateway binary not found at {}. Build it: cargo build --release -p arli-gateway",
                    gateway_bin.display()
                );
            }

            let status = std::process::Command::new(&gateway_bin)
                .arg("--daemon")
                .arg("--pid-file")
                .arg(pid_file.to_str().unwrap())
                .arg("--log-file")
                .arg(log_file.to_str().unwrap())
                .env("ARLI_HOME", &data_dir)
                .status()?;

            if !status.success() {
                anyhow::bail!("Gateway failed to start (exit code: {:?})", status.code());
            }
        }

        GatewayCmd::Stop => {
            if !pid_file.exists() {
                println!("Gateway is not running (no PID file).");
                return Ok(());
            }

            let pid_str = std::fs::read_to_string(&pid_file)?;
            let pid: i32 = pid_str.trim().parse()?;

            unsafe {
                if libc::kill(pid, 0) != 0 {
                    println!("Gateway is not running (PID {} is dead).", pid);
                    std::fs::remove_file(&pid_file)?;
                    return Ok(());
                }
                libc::kill(pid, libc::SIGTERM);
            }

            // Wait for process to exit (up to 5 seconds)
            for _ in 0..50 {
                unsafe {
                    if libc::kill(pid, 0) != 0 {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }

            // Force kill if still alive
            unsafe {
                if libc::kill(pid, 0) == 0 {
                    libc::kill(pid, libc::SIGKILL);
                    println!("Gateway force-killed (PID: {})", pid);
                } else {
                    println!("Gateway stopped (PID: {})", pid);
                }
            }

            std::fs::remove_file(&pid_file).ok();
        }

        GatewayCmd::Status => {
            if !pid_file.exists() {
                println!("Gateway: stopped (no PID file)");
                return Ok(());
            }

            let pid_str = std::fs::read_to_string(&pid_file)?;
            let pid: i32 = pid_str.trim().parse()?;

            unsafe {
                if libc::kill(pid, 0) == 0 {
                    println!("Gateway: running (PID: {})", pid);
                } else {
                    println!("Gateway: stopped (PID {} is dead)", pid);
                    std::fs::remove_file(&pid_file).ok();
                }
            }
        }

        GatewayCmd::Log { lines } => {
            if !log_file.exists() {
                println!("No log file at {}", log_file.display());
                return Ok(());
            }
            // Read and tail the log file
            let content = std::fs::read_to_string(&log_file)?;
            let all_lines: Vec<&str> = content.lines().collect();
            let start = if all_lines.len() > lines {
                all_lines.len() - lines
            } else {
                0
            };
            for line in &all_lines[start..] {
                println!("{}", line);
            }
        }
    }

    Ok(())
}

async fn run_chat(
    model_override: &Option<String>,
    iterations: usize,
    query: Option<String>,
    resume_id: Option<String>,
) -> anyhow::Result<()> {
    let mut config = Config::from_env()?;

    // Apply model override from -m flag
    if let Some(ref m) = *model_override {
        config.model = m.clone();
    }

    info!("Using model: {} via {}", config.model, config.provider.name);

    let provider = create_provider(&config)?;

    let data_dir = get_data_dir();
    let db_path = data_dir.join("sessions.db");
    let memory_path = data_dir.join("memory.db");

    let store = SessionStore::open(db_path.clone())?;

    use arli_core::memory::MemoryStore;
    use std::sync::Arc;
    let memory_store = Arc::new(MemoryStore::open(memory_path)?);

    // Handle --resume / --continue
    // We must load messages BEFORE moving store into session (Connection is not Clone).
    let resume_data: Option<(String, Vec<ChatMessage>)> = if let Some(ref rid) = resume_id {
        // Validate session exists and has messages
        let messages = match store.load_messages(rid) {
            Ok(msgs) if !msgs.is_empty() => {
                println!(
                    "↻ Resuming session {} ({} messages)…",
                    &rid[..8.min(rid.len())],
                    msgs.len()
                );
                msgs
            }
            Ok(_) => {
                anyhow::bail!("Session {} has no messages — cannot resume", rid);
            }
            Err(e) => {
                anyhow::bail!("Cannot load session {}: {}", rid, e);
            }
        };
        // Create a child session for lineage
        let child_id =
            store.resume_session(rid, Some(&format!("resume-{}", &rid[..8.min(rid.len())])))?;
        Some((child_id, messages))
    } else {
        None
    };

    let mut tools = ToolRegistry::new();
    register_builtin_tools(
        &mut tools,
        Some(db_path),
        Some(memory_store.clone()),
        None,
        None,
        None,
        None,
    );

    let session = Some(store);

    let agent_config = AgentConfig {
        name: config.agent_name.clone(),
        session_id: None,
        system_prompt: None,
        protect_last_n: 20,
        protect_first_n: 3,
        token_budget: None,
        time_budget_secs: None,
        dollar_budget_cents: None,
        brokering: None,
        tenant_id: None,
        provider_name: None,
    };

    let mut agent = Agent::new(
        agent_config,
        provider,
        tools,
        PolicyEngine::default(),
        session,
        iterations.max(1),
        None,
    );

    // If resuming, inject loaded history into agent
    if let Some((child_id, messages)) = resume_data {
        agent.load_history(child_id, messages);
    }

    match query {
        Some(q) => {
            info!("Processing query: {}", q);
            let sender = agent.sender();
            let rx = agent.sender();
            let handle = tokio::spawn(async move { agent.run(Some(q)).await });
            drop(sender);
            drop(rx);

            match handle.await? {
                Ok(response) => println!("\n{}\n", response),
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        None => {
            let sender = agent.sender();
            let (response_tx, response_rx) = mpsc::channel::<String>(64);

            tokio::spawn(async move {
                loop {
                    match agent.run(None).await {
                        Ok(response) => {
                            if response_tx.send(response).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = response_tx.send(format!("Error: {}", e)).await;
                        }
                    }
                }
            });

            tui::run_tui(sender, response_rx).await?;
        }
    }

    Ok(())
}

// --- Self-update ---

fn run_update(check_only: bool) -> anyhow::Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    println!("Current version: v{}", current);

    let auth_token = std::env::var("GITHUB_TOKEN").ok();
    let updater = if let Some(ref token) = auth_token {
        self_update::backends::github::Update::configure()
            .repo_owner("ARLI-Research")
            .repo_name("arli")
            .bin_name("arli")
            .show_download_progress(true)
            .current_version(current)
            .no_confirm(true)
            .auth_token(token)
            .build()?
    } else {
        self_update::backends::github::Update::configure()
            .repo_owner("ARLI-Research")
            .repo_name("arli")
            .bin_name("arli")
            .show_download_progress(true)
            .current_version(current)
            .no_confirm(true)
            .build()?
    };

    if check_only {
        let latest = updater.get_latest_release()?;
        if latest.version != current {
            println!("New version available: v{}", latest.version);
            println!("Release: {}", latest.body.unwrap_or_default());
            println!("\nRun 'arli update' to install.");
        } else {
            println!("Already up to date.");
        }
    } else {
        let status = updater.update()?;
        println!("Updated to v{}", status.version());
        println!("Restart arli to use the new version.");
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("ARLI_LOG").unwrap_or_else(|_| "info,arli_core=debug".to_string()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            println!("ARLI v{}", env!("CARGO_PKG_VERSION"));
        }

        Commands::Setup { ii, name, icp_gateway } => {
            if ii {
                run_setup_ii(&name, &icp_gateway)?;
            } else {
                run_setup()?;
            }
        }

        Commands::Config(cmd) => {
            run_config(cmd)?;
        }

        Commands::Model => {
            run_model()?;
        }

        Commands::Doctor => {
            run_doctor()?;
        }

        Commands::Serve { port } => {
            use arli_core::health::HealthServer;
            let health = HealthServer::new(port);
            health.set_ready(true).await;
            health
                .set_metric("arli_version", env!("CARGO_PKG_VERSION"))
                .await;
            health.set_metric("rust_version", "1.95").await;
            health.serve().await;
        }

        Commands::Sessions => {
            let data_dir = get_data_dir();
            let db_path = data_dir.join("sessions.db");
            let store = SessionStore::open(db_path)?;
            let sessions = store.list_sessions(20)?;

            if sessions.is_empty() {
                println!("No sessions yet.\nStart one: arli chat");
            } else {
                println!("Recent sessions:\n");
                for s in &sessions {
                    let count = store.message_count(&s.id).unwrap_or(0);
                    let parent = if s.name.contains("resume-") {
                        " ↻"
                    } else {
                        "  "
                    };
                    println!(
                        "  {}{:.36}  {}  {} msgs  {}",
                        parent,
                        s.id,
                        &s.updated_at[..16.min(s.updated_at.len())],
                        count,
                        s.name,
                    );
                }
                println!("\nResume: arli --resume <id>   or   arli --continue");
            }
        }

        Commands::Chat { query } => {
            let model_override = cli.model.clone();
            let iterations = cli.iterations;

            // Resolve --resume / --continue into an actual session ID
            let resume_id = if cli.continue_session {
                // Find the most recent session
                let data_dir = get_data_dir();
                let db_path = data_dir.join("sessions.db");
                let store = SessionStore::open(db_path)?;
                let sessions = store.list_sessions(1)?;
                if sessions.is_empty() {
                    anyhow::bail!("No sessions to resume. Start a new chat with `arli chat`.");
                }
                Some(sessions[0].id.clone())
            } else {
                cli.resume
            };

            run_chat(&model_override, iterations, query, resume_id).await?;
        }

        Commands::Cron(cmd) => {
            run_cron(cmd).await?;
        }

        Commands::Completion { shell } => {
            use clap_complete::{generate, Shell};
            let mut cmd = <Cli as clap::CommandFactory>::command();
            let binary_name = "arli";
            match shell.as_str() {
                "bash" => generate(Shell::Bash, &mut cmd, binary_name, &mut std::io::stdout()),
                "zsh" => generate(Shell::Zsh, &mut cmd, binary_name, &mut std::io::stdout()),
                "fish" => generate(Shell::Fish, &mut cmd, binary_name, &mut std::io::stdout()),
                other => anyhow::bail!("Unknown shell '{}'. Use: bash, zsh, fish", other),
            }
        }

        Commands::Mcp => {
            use arli_core::mcp::McpServer;
            use arli_core::tools::builtin::register_builtin_tools;

            let data_dir = get_data_dir();
            let db_path = data_dir.join("sessions.db");

            let mut tools = arli_core::ToolRegistry::new();
            register_builtin_tools(&mut tools, Some(db_path), None, None, None, None, None);

            let mut server = McpServer::new(tools);
            server.run_sync()?;
        }

        Commands::Plugins(cmd) => {
            use arli_core::plugins::PluginManager;

            let plugins_dir = get_data_dir().join("plugins");
            let mut manager = PluginManager::new(plugins_dir);

            match cmd {
                PluginsCmd::List => {
                    let plugins = manager.discover()?;
                    if plugins.is_empty() {
                        println!("No plugins found.");
                        println!(
                            "Add plugins to: {}",
                            get_data_dir().join("plugins").display()
                        );
                        println!("Each plugin needs: plugin.toml + executable");
                    } else {
                        println!("Discovered plugins:\n");
                        for p in &plugins {
                            let status = if p.plugin.enabled {
                                "enabled"
                            } else {
                                "disabled"
                            };
                            println!(
                                "  {} v{} — {} [{}]",
                                p.plugin.name, p.plugin.version, p.plugin.description, status
                            );
                            println!(
                                "    Tools: {}",
                                p.tools
                                    .iter()
                                    .map(|t| t.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                            println!("    Exec: {}", p.plugin.executable);
                            println!();
                        }
                    }
                }
                PluginsCmd::Load => {
                    let plugins = manager.discover()?;
                    let enabled: Vec<_> =
                        plugins.into_iter().filter(|p| p.plugin.enabled).collect();
                    if enabled.is_empty() {
                        println!("No enabled plugins to load.");
                    } else {
                        for manifest in enabled {
                            match manager.load(manifest) {
                                Ok(()) => {}
                                Err(e) => eprintln!("Failed to load plugin: {}", e),
                            }
                        }
                        println!("Loaded {} plugin(s)", manager.loaded_names().len());
                    }
                }
            }
        }

        Commands::Profile(cmd) => {
            use arli_core::profiles;

            match cmd {
                ProfileCmd::List => {
                    let profiles = profiles::list_profiles()?;
                    let current = profiles::current_profile();

                    if profiles.is_empty() {
                        println!("No profiles found.");
                        println!("Create one: arli profile create <name>");
                    } else {
                        println!("Profiles:\n");
                        for p in &profiles {
                            let marker = if p.name == current { " <- current" } else { "" };
                            let config = if p.has_config { "cfg" } else { "-" };
                            let soul = if p.has_soul { "soul" } else { "-" };
                            let sessions = if p.has_sessions { "db" } else { "-" };
                            println!(
                                "  {}{}  [{}, {}, {}]  {}",
                                p.name,
                                marker,
                                config,
                                soul,
                                sessions,
                                p.path.display()
                            );
                        }
                        println!("\nCurrent profile: {}", current);
                        println!("Switch: arli profile use <name>  or  ARLI_PROFILE=<name>");
                    }
                }
                ProfileCmd::Create { name, clone } => {
                    let dir = profiles::create_profile(&name, clone)?;
                    println!("Profile '{}' created at {}", name, dir.display());
                    if clone {
                        println!("Cloned config and soul.md from default profile.");
                    }
                    println!("Use it: arli profile use {}", name);
                }
                ProfileCmd::Use { name } => {
                    profiles::set_default_profile(&name)?;
                    if name == "default" || name.is_empty() {
                        println!("Switched to default profile.");
                    } else {
                        println!("Switched to profile '{}'.", name);
                    }
                }
                ProfileCmd::Delete { name } => {
                    profiles::delete_profile(&name)?;
                    println!("Profile '{}' deleted.", name);
                }
                ProfileCmd::Current => {
                    let current = profiles::current_profile();
                    let dir = profiles::arli_data_dir();
                    println!("Profile: {}", current);
                    println!("Data dir: {}", dir.display());
                }
            }
        }

        Commands::Webhook(cmd) => {
            use arli_core::webhooks::{WebhookState, WebhookSubscription};
            use std::sync::Arc;

            let state = Arc::new(WebhookState::new());

            match cmd {
                WebhookCmd::List => {
                    let subs = state.list().await;
                    if subs.is_empty() {
                        println!("No webhook subscriptions.");
                        println!(
                            "Subscribe: arli webhook subscribe <name> -p 'Prompt with {{payload}}'"
                        );
                    } else {
                        println!("Webhook subscriptions:\n");
                        for s in &subs {
                            let target = s.target_channel.as_deref().unwrap_or("-");
                            println!("  POST /webhooks/{}  → {}", s.name, s.prompt_template);
                            println!("    Target: {}", target);
                        }
                    }
                }
                WebhookCmd::Subscribe { name, prompt } => {
                    state
                        .subscribe(WebhookSubscription {
                            name: name.clone(),
                            prompt_template: prompt,
                            target_channel: None,
                        })
                        .await;
                    println!("Subscribed: POST /webhooks/{}", name);
                    println!("Start server: arli webhook serve");
                }
                WebhookCmd::Remove { name } => {
                    if state.unsubscribe(&name).await {
                        println!("Removed webhook: {}", name);
                    } else {
                        anyhow::bail!("Webhook '{}' not found", name);
                    }
                }
                WebhookCmd::Serve { port } => {
                    use arli_core::webhooks;
                    // Load saved subscriptions from config
                    let data_dir = get_data_dir();
                    let webhook_cfg = data_dir.join("webhooks.toml");
                    if webhook_cfg.exists() {
                        let content = std::fs::read_to_string(&webhook_cfg)?;
                        #[derive(Deserialize)]
                        struct WebhookFile {
                            subscriptions: Vec<WebhookSubscription>,
                        }
                        let wf: WebhookFile = toml::from_str(&content)?;
                        for sub in wf.subscriptions {
                            state.subscribe(sub).await;
                        }
                    }
                    webhooks::serve(state, port).await?;
                }
            }
        }

        Commands::Checkpoint(cmd) => {
            use arli_core::checkpoints::CheckpointManager;

            let base = get_data_dir().join("checkpoints");
            let session_id = "cli".to_string(); // TODO: use actual session ID
            let mut cm = CheckpointManager::new(base, session_id)?;

            match cmd {
                CheckpointCmd::List => {
                    let cps = cm.list();
                    if cps.is_empty() {
                        println!("No checkpoints.");
                    } else {
                        println!("Checkpoints:\n");
                        for cp in cps {
                            println!("  #{}  {}  ({} files)", cp.id, cp.timestamp, cp.files.len());
                            for path in cp.files.keys() {
                                println!("    {}", path);
                            }
                        }
                    }
                }
                CheckpointCmd::Create => {
                    let id = cm.new_checkpoint();
                    println!("Checkpoint #{} created.", id);
                }
                CheckpointCmd::Rollback => {
                    let restored = cm.rollback()?;
                    println!("Rollback complete: {} files restored.", restored);
                }
                CheckpointCmd::Prune { keep } => {
                    let removed = cm.prune(keep)?;
                    println!("Pruned {} checkpoints (keeping {}).", removed, keep);
                }
            }
        }

        Commands::Update { check } => {
            tokio::task::spawn_blocking(move || run_update(check)).await??;
        }

        Commands::Gateway(cmd) => {
            run_gateway(cmd)?;
        }

        Commands::Key(cmd) => {
            run_key(cmd)?;
        }

        Commands::Kanban(cmd) => {
            run_kanban(cmd)?;
        }

        Commands::Dashboard { port } => {
            run_dashboard(port)?;
        }

        Commands::Marketplace(cmd) => {
            run_marketplace(cmd)?;
        }

        Commands::Enso(cmd) => {
            run_enso(cmd)?;
        }

        Commands::Brokering(cmd) => {
            run_brokering(cmd)?;
        }

        Commands::Harness(cmd) => match cmd {
            HarnessCmd::Analyze => {
                use arli_core::harness_analytics::HarnessAnalyzer;
                let analyzer = HarnessAnalyzer::default();
                let report = analyzer.analyze();
                println!("{}", report.format_text());
            }
        },

        Commands::Api { port } => {
            use arli_core::brokering::{RateLimiter, TenantManager, UsageTracker};
            use arli_core::brokering_api::BrokeringApiServer;
            use std::sync::Arc;

            let config = Config::from_env()?;
            let brokering_config = config.brokering;
            let db_path = brokering_config.db_path.clone().unwrap_or_else(|| {
                get_data_dir()
                    .join("brokering.db")
                    .to_string_lossy()
                    .to_string()
            });

            let tm = TenantManager::new(&db_path).map_err(|e| anyhow::anyhow!("{e}"))?;
            let rl = Arc::new(RateLimiter::new(&brokering_config));
            let ut = Arc::new(UsageTracker::new(&db_path).map_err(|e| anyhow::anyhow!("{e}"))?);

            let server = BrokeringApiServer::new(brokering_config, tm, rl, ut);
            println!("Brokering API server starting on port {}", port);

            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(server.serve(port));
        }
    }

    Ok(())
}

fn run_key(cmd: KeyCmd) -> anyhow::Result<()> {
    use arli_core::attestation::ArliKeypair;

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let key_path = std::path::PathBuf::from(home)
        .join(".arli")
        .join("arli_key.pem");

    match cmd {
        KeyCmd::Generate => {
            if key_path.exists() {
                anyhow::bail!(
                    "Key already exists at {}. Delete it first to regenerate.",
                    key_path.display()
                );
            }
            let kp = ArliKeypair::generate();
            kp.save(&key_path).map_err(|e| anyhow::anyhow!("{}", e))?;
            println!("Key generated: {}", key_path.display());
            println!("Permissions: 600 (owner read/write only)");
            println!();
            println!("Public key for ENSO Registry:");
            println!("  {}", kp.public_key_hex());
        }
        KeyCmd::Show => {
            if !key_path.exists() {
                println!("No key found. Generate one with: arli key generate");
                return Ok(());
            }
            let kp = ArliKeypair::load(&key_path).map_err(|e| anyhow::anyhow!("{}", e))?;
            println!("Key location: {}", key_path.display());
            println!();
            println!("Public key for ENSO Registry:");
            println!("  {}", kp.public_key_hex());
            println!();
            println!("Register this key with ENSO Registry to enable attestation verification.");
        }
    }

    Ok(())
}

fn run_kanban(cmd: KanbanCmd) -> anyhow::Result<()> {
    use arli_core::kanban::{KanbanStore, Priority};

    let db_path = get_data_dir().join("kanban.db");
    let store = KanbanStore::open(db_path)?;

    match cmd {
        KanbanCmd::Create { name, description } => {
            let desc = description.unwrap_or_default();
            let board = store.create_board(&name, &desc)?;
            println!("Board created: {} ({})", board.name, board.id);
            let cols = store.list_columns(&board.id)?;
            println!(
                "Columns: {}",
                cols.iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        KanbanCmd::Show { id } => {
            let board_id = match id {
                Some(id) => id,
                None => {
                    let boards = store.list_boards()?;
                    if boards.is_empty() {
                        anyhow::bail!("No boards. Create one with: arli kanban create <name>");
                    }
                    println!("Showing first board: {}", boards[0].name);
                    boards[0].id.clone()
                }
            };
            let stats = store.get_board_stats(&board_id)?;
            println!("Board: {} ({} cards)", stats.board_name, stats.total_cards);
            for col in &stats.columns {
                let wip = col
                    .wip_limit
                    .map(|l| format!(" [WIP: {l}]"))
                    .unwrap_or_default();
                println!("  [{}] {}{}", col.column_name, col.card_count, wip);
                for card in &col.cards {
                    let assignee = card.assignee.as_deref().unwrap_or("-");
                    println!(
                        "    {} | {} | prio:{} | {}",
                        card.id,
                        card.title,
                        card.priority.as_str(),
                        assignee
                    );
                }
            }
        }
        KanbanCmd::List => {
            let boards = store.list_boards()?;
            if boards.is_empty() {
                println!("No boards. Create one with: arli kanban create <name>");
            } else {
                for b in &boards {
                    let stats = store.get_board_stats(&b.id)?;
                    println!("{} ({}) — {} cards", b.name, b.id, stats.total_cards);
                }
            }
        }
        KanbanCmd::Add {
            board_id,
            column,
            title,
            description,
            priority,
        } => {
            let desc = description.unwrap_or_default();
            let prio = Priority::from_str(&priority);
            let cols = store.list_columns(&board_id)?;
            let col = cols.iter().find(|c| c.name == column).ok_or_else(|| {
                anyhow::anyhow!(
                    "Column '{}' not found. Available: {}",
                    column,
                    cols.iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            let card = store.add_card(&board_id, &col.id, &title, &desc, prio, None, &[], None)?;
            println!("Card added: {} ({})", card.title, card.id);
        }
        KanbanCmd::Move { card_id, column } => {
            let card = store.get_card(&card_id)?;
            let cols = store.list_columns(&card.board_id)?;
            let target = cols.iter().find(|c| c.name == column).ok_or_else(|| {
                anyhow::anyhow!(
                    "Column '{}' not found. Available: {}",
                    column,
                    cols.iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            let moved = store.move_card(&card_id, &target.id)?;
            println!("Card '{}' moved to {}", moved.title, column);
        }
    }
    Ok(())
}

fn run_dashboard(port: u16) -> anyhow::Result<()> {
    use arli_core::dashboard::{build_router, DashboardConfig, DashboardState};
    use arli_core::kanban::KanbanStore;
    use arli_core::metrics::Metrics;
    use std::sync::Arc;

    let metrics = Arc::new(Metrics::new());
    metrics.mark_ready();

    let config = DashboardConfig {
        port,
        title: "ARLI Dashboard".into(),
    };
    let kanban_path = get_data_dir().join("kanban.db");
    let kanban = if kanban_path.exists() {
        Some(Arc::new(KanbanStore::open(kanban_path)?))
    } else {
        None
    };

    let mut state = DashboardState::new(config, metrics);
    if let Some(k) = kanban {
        state = state.with_kanban(k);
    }
    let state = Arc::new(state);
    let router = build_router(state);

    let rt = tokio::runtime::Runtime::new()?;
    println!("Dashboard starting on http://localhost:{}/dashboard", port);
    println!("Press Ctrl+C to stop.");
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
            .await
            .map_err(|e| anyhow::anyhow!("Bind failed: {e}"))?;
        axum::serve(listener, router).await?;
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(())
}

fn run_marketplace(cmd: MarketplaceCmd) -> anyhow::Result<()> {
    use arli_core::enso::marketplace::MarketplaceStore;

    let db_path = get_data_dir().join("marketplace.db");
    let store = MarketplaceStore::open(db_path)?;

    match cmd {
        MarketplaceCmd::RfqCreate {
            title,
            budget_cents,
            deadline,
            capabilities,
        } => {
            let caps: Vec<String> = capabilities
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let rfq = store.create_rfq(
                "cli-user",
                &title,
                "",
                budget_cents,
                &deadline,
                &caps,
                Some("KernelSandbox"),
                None,
                &[],
            )?;
            println!("RFQ created: {} ({})", rfq.title, rfq.id);
            println!(
                "  Budget: {}¢ | Deadline: {} | Status: {}",
                rfq.budget_cents,
                rfq.deadline,
                rfq.status.as_str()
            );
            if !caps.is_empty() {
                println!("  Capabilities: {}", caps.join(", "));
            }
        }
        MarketplaceCmd::RfqList { status } => {
            let rfqs = store.list_rfqs(status.as_deref())?;
            if rfqs.is_empty() {
                println!("No RFQs found.");
            } else {
                for r in &rfqs {
                    println!(
                        "{} | {} | {}¢ | {} | {}",
                        r.id,
                        r.title,
                        r.budget_cents,
                        r.status.as_str(),
                        r.creator
                    );
                }
            }
        }
        MarketplaceCmd::RfqShow { id } => {
            let rfq = store.get_rfq(&id)?;
            println!("RFQ: {} ({})", rfq.title, rfq.id);
            println!(
                "  Creator: {} | Budget: {}¢ | Deadline: {}",
                rfq.creator, rfq.budget_cents, rfq.deadline
            );
            println!(
                "  Status: {} | Trust: {:?}",
                rfq.status.as_str(),
                rfq.required_trust_model
            );
            if !rfq.required_capabilities.is_empty() {
                println!("  Capabilities: {}", rfq.required_capabilities.join(", "));
            }
            let quotes = store.list_quotes_for_rfq(&id)?;
            if quotes.is_empty() {
                println!("  No quotes yet.");
            } else {
                println!("  Quotes:");
                for q in &quotes {
                    let accepted = if q.accepted { " ✅" } else { "" };
                    println!(
                        "    {} | agent:{} | {}¢ | {}s{}",
                        q.id, q.agent_id, q.price_cents, q.estimated_time_secs, accepted
                    );
                }
            }
        }
        MarketplaceCmd::Quote {
            rfq_id,
            agent_id,
            price_cents,
            time_secs,
        } => {
            let quote = store.submit_quote(
                &rfq_id,
                &agent_id,
                price_cents,
                time_secs,
                "CLI-submitted quote",
                "KernelSandbox",
                None,
            )?;
            println!(
                "Quote submitted: {} ({}¢, {}s)",
                quote.id, quote.price_cents, quote.estimated_time_secs
            );
        }
        MarketplaceCmd::Accept { quote_id } => {
            let quote = store.accept_quote(&quote_id)?;
            println!("Quote accepted!");
            println!("  Contract ID: {}", quote.contract_id.unwrap_or_default());
            println!(
                "  Agent: {} | Price: {}¢",
                quote.agent_id, quote.price_cents
            );
        }
        MarketplaceCmd::Stats => {
            let stats = store.get_stats()?;
            println!("Marketplace Stats:");
            println!("  Open RFQs:      {}", stats.open_rfqs);
            println!("  Total RFQs:     {}", stats.total_rfqs);
            println!("  Total Quotes:   {}", stats.total_quotes);
            println!("  Contracted:     {}", stats.contracted);
            println!(
                "  Open value:     {}¢ (${:.2})",
                stats.open_value_cents,
                stats.open_value_cents as f64 / 100.0
            );
        }
    }
    Ok(())
}

fn run_enso(cmd: EnsoCmd) -> anyhow::Result<()> {
    use sha2::Digest;
    match cmd {
        EnsoCmd::Onboard => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            let arli_dir = std::path::PathBuf::from(&home).join(".arli");
            std::fs::create_dir_all(&arli_dir)?;
            let key_path = arli_dir.join("arli_key.pem");

            // 1. Keygen
            let pubkey = if key_path.exists() {
                let kp = arli_core::attestation::ArliKeypair::load(&key_path)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                kp.public_key_hex()
            } else {
                let kp = arli_core::attestation::ArliKeypair::generate();
                kp.save(&key_path).map_err(|e| anyhow::anyhow!("{e}"))?;
                kp.public_key_hex()
            };

            // 2. Binary hash
            let binary_hash = {
                let exe = std::env::current_exe().unwrap_or_default();
                let bytes = std::fs::read(&exe)?;
                let hash = sha2::Sha256::digest(&bytes);
                format!("{:x}", hash)
            };

            // 3. Save config
            let contracts_id = "5fp3e-cyaaa-aaaae-agtra-cai";
            let config_toml = format!(
                r#"# ENSO configuration for ARLI
icp_gateway = "https://icp0.io"
registry_canister_id = "ENSO_REGISTRY_CANISTER_ID"
contracts_canister_id = "{contracts_id}"
arli_public_key = "{pubkey}"
agent_name = "ARLI v0.5"
"#,
            );
            std::fs::write(arli_dir.join("enso.toml"), &config_toml)?;

            // 4. Print header
            println!();
            println!("═══════════════════════════════════════════");
            println!("  ARLI ENSO — Onboarding");
            println!("═══════════════════════════════════════════");
            println!();
            println!("  Key:      ~/.arli/arli_key.pem");
            println!("  Config:   ~/.arli/enso.toml");
            println!("  Pubkey:   {pubkey}");
            println!("  Bin hash: {binary_hash}");
            println!();

            // 5. Attempt self-registration on ENSO canister
            let dfx_id_path =
                std::path::PathBuf::from(&home).join(".config/dfx/identity/default/identity.pem");

            if !dfx_id_path.exists() {
                println!("  ⚠ No dfx identity — install dfx to auto-register:");
                println!("    sh -ci \"$(curl -fsSL https://internetcomputer.org/install.sh)\"");
                println!();
                println!("  Until then, send these to your ENSO operator:");
                println!();
                println!("    Public key:   {pubkey}");
                println!("    Binary hash:  {binary_hash}");
                println!("    Agent name:   ARLI v0.5");
                println!();
                println!("  Once registered, run:");
                println!("    arli enso run -c <contract-id>");
                println!();
                return Ok(());
            }

            // Try self-registration
            let config = arli_core::enso::EnsoConfig {
                icp_gateway: "https://icp0.io".into(),
                contracts_canister_id: contracts_id.into(),
                registry_canister_id: "ENSO_REGISTRY_CANISTER_ID".into(),
                identity_pem_path: Some(dfx_id_path.to_string_lossy().to_string()),
                arli_public_key: pubkey.clone(),
                agent_name: "ARLI v0.5".into(),
            };

            println!("  Registering with ENSO canister...");

            let result: Result<(), String> = tokio::task::block_in_place(|| {
                let rt = tokio::runtime::Handle::current();
                rt.block_on(async {
                    use arli_core::enso::EnsoClient;
                    let enso = EnsoClient::new(config).await?;
                    enso.register_arli_agent(
                        &binary_hash,
                        "ARLI v0.5",
                        &["attestation".into(), "oracle".into(), "sandbox".into()],
                        "unknown",
                    )
                    .await
                })
            });

            match result {
                Ok(()) => {
                    println!("  ✅ Agent registered on ENSO mainnet.");
                    println!();
                    println!("  Ready. When you get a contract:");
                    println!("    arli enso run -c <contract-id>");
                    println!();
                    println!("  Check status:");
                    println!("    arli enso status");
                    println!();
                }
                Err(e) => {
                    println!("  ⚠ Auto-registration failed: {e}");
                    println!();
                    println!("  Send these to your ENSO operator to complete registration:");
                    println!();
                    println!("    Public key:   {pubkey}");
                    println!("    Binary hash:  {binary_hash}");
                    println!("    Agent name:   ARLI v0.5");
                    println!();
                    println!("  Once registered, run:");
                    println!("    arli enso run -c <contract-id>");
                    println!();
                }
            }
        }

        EnsoCmd::Setup {
            icp_gateway,
            registry,
            contracts,
            name,
        } => {
            // 1. Keygen
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            let arli_dir = std::path::PathBuf::from(&home).join(".arli");
            std::fs::create_dir_all(&arli_dir)?;
            let key_path = arli_dir.join("arli_key.pem");

            let pubkey = if key_path.exists() {
                let kp = arli_core::attestation::ArliKeypair::load(&key_path)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                println!("Using existing key: {}", key_path.display());
                kp.public_key_hex()
            } else {
                let kp = arli_core::attestation::ArliKeypair::generate();
                kp.save(&key_path).map_err(|e| anyhow::anyhow!("{e}"))?;
                println!("Key generated: {}", key_path.display());
                kp.public_key_hex()
            };

            // 2. Save ENSO config
            let registry_id = registry.unwrap_or_else(|| "ENSO_REGISTRY_CANISTER_ID".into());
            let contracts_id = contracts.unwrap_or_else(|| "7yv6j-ryaaa-aaaaa-qhheq-cai".into());

            let config_toml = format!(
                r#"# ENSO configuration for ARLI
# Generated by: arli enso setup

icp_gateway = "{gateway}"
registry_canister_id = "{registry}"
contracts_canister_id = "{contracts}"
arli_public_key = "{pubkey}"
agent_name = "{name}"
"#,
                gateway = icp_gateway,
                registry = registry_id,
                contracts = contracts_id,
                pubkey = pubkey,
                name = name,
            );
            let enso_config_path = arli_dir.join("enso.toml");
            std::fs::write(&enso_config_path, config_toml)?;

            // 3. Compute binary hash
            let binary_hash = {
                let exe = std::env::current_exe().unwrap_or_default();
                if exe.exists() {
                    let bytes = std::fs::read(&exe)?;
                    let hash = sha2::Sha256::digest(&bytes);
                    format!("{:x}", hash)
                } else {
                    "unknown (binary not found)".into()
                }
            };

            let sandbox_config_hash = {
                // ENSO sandbox policy v1 — deployed on canister 5fp3e-cyaaa-aaaae-agtra-cai
                // New contracts require this hash for attestation.
                arli_core::enso::ENSO_SANDBOX_POLICY_V1_HASH.to_string()
            };

            println!();
            println!("=== ENSO Setup Complete ===");
            println!();
            println!("Config saved: {}", enso_config_path.display());
            println!();
            println!("To register with ENSO Registry:");
            println!("  dfx canister call {} register_arli_agent \\", registry_id);
            println!(
                "    '(\\\"{}\\\", \\\"{}\\\", \\\"{}\\\", vec {{\\\"trading\\\"}}, \\\"{}\\\")'",
                pubkey, binary_hash, name, sandbox_config_hash
            );
            println!();
            println!("ENS Contracts canister: {}", contracts_id);
            println!("ICP Gateway:           {}", icp_gateway);
            println!();
            println!("Ready for ENSO mainnet.");
        }

        EnsoCmd::Status => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            let arli_dir = std::path::PathBuf::from(&home).join(".arli");
            let key_path = arli_dir.join("arli_key.pem");
            let config_path = arli_dir.join("enso.toml");

            println!("=== ENSO Status ===\n");

            // Key
            if key_path.exists() {
                let kp = arli_core::attestation::ArliKeypair::load(&key_path)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                println!("Key:    {} (exists)", key_path.display());
                println!("Pubkey: {}", kp.public_key_hex());
            } else {
                println!("Key:    NOT FOUND. Run `arli enso onboard` first.");
            }

            // Config
            if config_path.exists() {
                let config = std::fs::read_to_string(&config_path)?;
                println!("\nConfig: {}", config_path.display());
                for line in config.lines() {
                    if !line.starts_with('#') && !line.is_empty() {
                        println!("  {}", line);
                    }
                }
            } else {
                println!("\nConfig: NOT FOUND. Run `arli enso onboard` first.");
            }

            // Active contracts
            let contracts = arli_core::enso::oracle::load_contracts_from_env();
            if contracts.is_empty() {
                println!("\nActive contracts: none (set ENSO_CONTRACTS env var)");
            } else {
                println!("\nActive contracts ({}):", contracts.len());
                for c in &contracts {
                    println!("  - {}", c);
                }
            }

            // Canister metadata hint
            println!("\nENS Contracts canister: 7yv6j-ryaaa-aaaaa-qhheq-cai");
            println!("ENS Frontend:           7rwvv-hqaaa-aaaaa-qhhfa-cai");
            println!("ENS URL:                https://7rwvv-hqaaa-aaaaa-qhhfa-cai.icp0.io/");
            println!("ARLI Bridge:            https://7rwvv-hqaaa-aaaaa-qhhfa-cai.icp0.io/#/app");
        }

        EnsoCmd::Register { capabilities } => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            let arli_dir = std::path::PathBuf::from(&home).join(".arli");
            let config_path = arli_dir.join("enso.toml");

            if !config_path.exists() {
                anyhow::bail!("No ENSO config found. Run `arli enso onboard` first.");
            }

            let config: arli_core::enso::EnsoConfig =
                toml::from_str(&std::fs::read_to_string(&config_path)?)?;

            let binary_hash = {
                let exe = std::env::current_exe().unwrap_or_default();
                let bytes = std::fs::read(&exe)?;
                let hash = sha2::Sha256::digest(&bytes);
                format!("{:x}", hash)
            };

            let sandbox_config_hash = {
                let policy_path = arli_dir.join("sandbox.yaml");
                if policy_path.exists() {
                    let bytes = std::fs::read(&policy_path)?;
                    let hash = sha2::Sha256::digest(&bytes);
                    format!("{:x}", hash)
                } else {
                    "unknown".into()
                }
            };

            let caps: Vec<String> = capabilities
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            println!("=== Register ARLI Agent with ENSO ===");
            println!("  Binary hash:   {}", binary_hash);
            println!("  Sandbox hash:  {}", sandbox_config_hash);
            println!("  Capabilities:  {:?}", caps);
            println!("  Name:          {}", config.agent_name);
            println!();

            use arli_core::enso::EnsoClient;
            let enso = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(EnsoClient::new(config))
            })
            .map_err(|e| anyhow::anyhow!("ENSO client init failed: {}", e))?;

            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(enso.register_arli_agent(
                    &binary_hash,
                    "ARLI v0.5",
                    &caps,
                    &sandbox_config_hash,
                ))
            })
            .map_err(|e| anyhow::anyhow!("Registration failed: {}", e))?;

            println!("\n✓ Agent registered successfully.");
            println!("  Agent ID: arli_cae6abd1c316f027 (generated by ENSO)");
        }

        EnsoCmd::Pay { contract_id } => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            let arli_dir = std::path::PathBuf::from(&home).join(".arli");
            let key_path = arli_dir.join("arli_key.pem");

            if !key_path.exists() {
                anyhow::bail!("No ARLI key found. Run `arli enso onboard` first.");
            }

            let kp = arli_core::attestation::ArliKeypair::load(&key_path)
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            let binary_hash = {
                let exe = std::env::current_exe().unwrap_or_default();
                if exe.exists() {
                    let bytes = std::fs::read(&exe)?;
                    let hash = sha2::Sha256::digest(&bytes);
                    format!("{:x}", hash)
                } else {
                    "sha256:unknown".into()
                }
            };

            let builder = arli_core::attestation::AttestationBuilder::new(kp, binary_hash.clone());

            let ocsf_event = serde_json::json!({
                "class_uid": 6007,
                "activity_name": "CLI Payment",
                "job_id": contract_id,
                "sandbox": "sha256:test-policy-v1",
            });
            let ocsf_json = serde_json::to_string(&ocsf_event)?;

            let attestation = builder.build(
                format!("cli-pay-{}", contract_id),
                "cli-agent".into(),
                contract_id.clone(),
                &ocsf_json,
                None,
                "sha256:test-policy-v1".into(),
                true,
                true,
                65534,
                None,
            );

            let attestation_json = serde_json::to_string(&attestation)?;

            println!("=== Payment Attestation ===");
            println!("Contract: {}", contract_id);
            println!("OCSF hash: {}", &attestation.ocsf_event_hash[..16]);
            println!();
            println!("Attestation JSON (send to ENSO):");
            println!("{}", attestation_json);
            println!();
            println!("To submit via dfx:");
            println!(
                "  dfx canister call {} submit_arli_payment \\",
                "7yv6j-ryaaa-aaaaa-qhheq-cai"
            );
            println!("    '(\"{}\", \"<attestation_json_above>\")'", contract_id);
            println!();
            println!("Or use the ARLI Bridge: https://7rwvv-hqaaa-aaaaa-qhhfa-cai.icp0.io/#/app");
        }

        EnsoCmd::Run { contract } => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            let arli_dir = std::path::PathBuf::from(&home).join(".arli");
            let key_path = arli_dir.join("arli_key.pem");
            let config_path = arli_dir.join("enso.toml");

            if !key_path.exists() {
                anyhow::bail!("No ARLI key found. Run `arli enso onboard` first.");
            }

            let kp = arli_core::attestation::ArliKeypair::load(&key_path)
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            let config: arli_core::enso::EnsoConfig = if config_path.exists() {
                let toml_str = std::fs::read_to_string(&config_path)?;
                toml::from_str(&toml_str)?
            } else {
                arli_core::enso::EnsoConfig::default()
            };

            // --contract flag takes priority over ENSO_CONTRACTS env var
            let contracts: Vec<String> = if let Some(c) = contract {
                vec![c]
            } else {
                let from_env = arli_core::enso::oracle::load_contracts_from_env();
                if from_env.is_empty() {
                    anyhow::bail!(
                        "No contracts. Use `arli enso run -c <contract-id>` or set ENSO_CONTRACTS env var."
                    );
                }
                from_env
            };

            let binary_hash = {
                let exe = std::env::current_exe().unwrap_or_default();
                if exe.exists() {
                    let bytes = std::fs::read(&exe)?;
                    let hash = sha2::Sha256::digest(&bytes);
                    format!("{:x}", hash)
                } else {
                    "unknown".into()
                }
            };

            let sandbox_config_hash = {
                let policy_path = arli_dir.join("sandbox.yaml");
                if policy_path.exists() {
                    let bytes = std::fs::read(&policy_path)?;
                    let hash = sha2::Sha256::digest(&bytes);
                    format!("{:x}", hash)
                } else {
                    "unknown".into()
                }
            };

            // ENSO agent ID from registry: agent_cae6abd1c316f0279d0260a3de0de297 (owner = ARLI principal)
            let agent_id = "agent_cae6abd1c316f0279d0260a3de0de297".to_string();

            // Build ENSO client with ICP identity — use current tokio handle
            use arli_core::enso::EnsoClient;
            let enso = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(EnsoClient::new(config))
            })
            .map_err(|e| anyhow::anyhow!("ENSO client init failed: {}", e))?;

            let mut oracle = arli_core::enso::oracle::EnsoOracle::new(
                contracts,
                agent_id.clone(),
                binary_hash,
                sandbox_config_hash,
                Some(kp),
                enso,
            );

            println!("ARLI ENSO Oracle starting...");
            println!(
                "  Contracts: {:?}",
                arli_core::enso::oracle::load_contracts_from_env()
            );
            println!("  Press Ctrl+C to stop.");

            let attested = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(oracle.run())
            });

            println!("\nOracle finished. Attested: {} contracts.", attested);
        }
    }
    Ok(())
}

fn run_brokering(cmd: BrokeringCmd) -> anyhow::Result<()> {
    use arli_core::brokering::{BillingReporter, TenantManager, UsageTracker};
    use chrono::Datelike;

    let config = Config::from_env()?;
    let db_path = config.brokering.db_path.clone().unwrap_or_else(|| {
        get_data_dir()
            .join("brokering.db")
            .to_string_lossy()
            .to_string()
    });

    let brokering_config = config.brokering;

    match cmd {
        BrokeringCmd::Tenant { cmd } => {
            let tm = TenantManager::new(&db_path).map_err(|e| anyhow::anyhow!("{e}"))?;

            match cmd {
                TenantCmd::Create { name, email } => {
                    let tenant_id = tm
                        .register_tenant(&name, &email)
                        .map_err(|e| anyhow::anyhow!("{e}"))?;

                    // rotate_api_key gives us a key we can show (register_tenant
                    // hashes the key internally and doesn't return it)
                    let api_key = tm
                        .rotate_api_key(tenant_id)
                        .map_err(|e| anyhow::anyhow!("{e}"))?;

                    println!("== Tenant Created ==");
                    println!("  Name:     {}", name);
                    println!("  Email:    {}", email);
                    println!("  ID:       {}", tenant_id);
                    println!(
                        "  API Key:  {}  ⬅ save this now — it cannot be recovered!",
                        api_key
                    );
                    println!();
                }
                TenantCmd::List => {
                    let tenants = tm.list_tenants().map_err(|e| anyhow::anyhow!("{e}"))?;
                    if tenants.is_empty() {
                        println!("No tenants found.");
                    } else {
                        println!("== Tenants ==\n");
                        for t in &tenants {
                            let status = if t.enabled { "enabled" } else { "DISABLED" };
                            println!(
                                "  {}  {}  {}  {}  {}",
                                t.id,
                                t.name,
                                t.contact_email,
                                status,
                                t.created_at.format("%Y-%m-%d %H:%M")
                            );
                        }
                        println!("\n{} tenant(s)", tenants.len());
                    }
                }
                TenantCmd::RotateKey { tenant_id } => {
                    let tid = Uuid::parse_str(&tenant_id)
                        .map_err(|_| anyhow::anyhow!("Invalid tenant UUID: {tenant_id}"))?;
                    let new_key = tm.rotate_api_key(tid).map_err(|e| anyhow::anyhow!("{e}"))?;
                    println!("== API Key Rotated ==");
                    println!("  Tenant:   {tid}");
                    println!("  New Key:  {new_key}  ⬅ save this now!");
                }
                TenantCmd::Enable { tenant_id } => {
                    let tid = Uuid::parse_str(&tenant_id)
                        .map_err(|_| anyhow::anyhow!("Invalid tenant UUID: {tenant_id}"))?;
                    tm.enable_tenant(tid).map_err(|e| anyhow::anyhow!("{e}"))?;
                    println!("Tenant {tid} enabled.");
                }
                TenantCmd::Disable { tenant_id } => {
                    let tid = Uuid::parse_str(&tenant_id)
                        .map_err(|_| anyhow::anyhow!("Invalid tenant UUID: {tenant_id}"))?;
                    tm.disable_tenant(tid).map_err(|e| anyhow::anyhow!("{e}"))?;
                    println!("Tenant {tid} disabled.");
                }
            }
        }
        BrokeringCmd::Report {
            tenant,
            year,
            month,
        } => {
            let tid = Uuid::parse_str(&tenant)
                .map_err(|_| anyhow::anyhow!("Invalid tenant UUID: {tenant}"))?;

            let now = chrono::Utc::now();
            let y = year.unwrap_or(now.year());
            let m = month.unwrap_or(now.month());

            let tm = TenantManager::new(&db_path).map_err(|e| anyhow::anyhow!("{e}"))?;
            let tracker = UsageTracker::new(&db_path).map_err(|e| anyhow::anyhow!("{e}"))?;
            let reporter = BillingReporter::new(tracker, tm, brokering_config);

            let report = reporter
                .generate_monthly_report(tid, y, m)
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            println!("== Billing Report ==");
            println!(
                "  Tenant:     {} ({})",
                report.tenant_name, report.tenant_id
            );
            println!("  Period:     {}-{:02}", report.year, report.month);
            println!("  Requests:   {}", report.total_requests);
            println!(
                "  Cost:       ${:.2} ({}¢)",
                report.total_cost_cents as f64 / 100.0,
                report.total_cost_cents
            );
            println!(
                "  + Margin:   ${:.2} ({}¢ @ {}%)",
                report.total_with_margin_cents as f64 / 100.0,
                report.total_with_margin_cents,
                (report.margin_percent * 100.0) as u32
            );
            println!(
                "  Generated:  {}",
                report.generated_at.format("%Y-%m-%d %H:%M:%S UTC")
            );
            println!();

            if !report.provider_breakdown.is_empty() {
                println!("== Provider Breakdown ==");
                for pb in &report.provider_breakdown {
                    println!("  ─ {} ─", pb.provider);
                    println!("    Requests:       {}", pb.requests);
                    println!("    Tokens in:      {}", pb.tokens_in);
                    println!("    Tokens out:     {}", pb.tokens_out);
                    println!(
                        "    Cost:           ${:.2} ({}¢)",
                        pb.cost_cents as f64 / 100.0,
                        pb.cost_cents
                    );
                    println!(
                        "    With margin:    ${:.2} ({}¢)",
                        pb.with_margin_cents as f64 / 100.0,
                        pb.with_margin_cents
                    );
                    println!();
                }
            }
        }
    }

    Ok(())
}
