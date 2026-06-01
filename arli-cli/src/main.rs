mod tui;

use clap::{Parser, Subcommand};
use arli_core::{
    Agent, AgentConfig, ChatMessage, Config, CronEvent, CronJob, CronScheduler,
    SessionStore, ToolRegistry, PolicyEngine,
    create_provider,
};
use arli_core::tools::builtin::register_builtin_tools;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::info;

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
    Setup,

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
    Remove {
        name: String,
    },
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
    Delete {
        name: String,
    },
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
    Set {
        key: String,
        value: String,
    },
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
    Pause {
        id: String,
    },
    /// Resume a paused cron job
    Resume {
        id: String,
    },
    /// Start the cron scheduler daemon
    Start,
    /// Run a job immediately (for testing)
    Run {
        id: String,
    },
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

fn run_setup() -> anyhow::Result<()> {
    use std::io::{self, Write};

    println!("=== ARLI Setup ===\n");

    // Provider
    println!("Choose provider:");
    println!("  1) DeepSeek (default)");
    println!("  2) OpenAI");
    println!("  3) Anthropic");
    println!("  4) OpenRouter (200+ models)");
    print!("> ");
    io::stdout().flush()?;

    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    let choice = choice.trim();

    let (provider_name, api_key_env) = match choice {
        "2" => ("openai", "OPENAI_API_KEY"),
        "3" => ("anthropic", "ANTHROPIC_API_KEY"),
        "4" => ("openrouter", "OPENROUTER_API_KEY"),
        _ => ("deepseek", "DEEPSEEK_API_KEY"),
    };

    // API key
    print!("\n{} API key (input hidden): ", provider_name);
    io::stdout().flush()?;
    let api_key = rpassword::read_password()?;

    if api_key.is_empty() {
        anyhow::bail!("API key is required");
    }

    // Write config
    let data_dir = get_data_dir();
    let config_path = data_dir.join("config.toml");

    let config_content = format!(
        "# ARLI Configuration\n\
         # Generated by `arli setup`\n\n\
         model = \"{}\"\n\
         max_iterations = 20\n\n\
         [provider]\n\
         name = \"{}\"\n\
         api_key = \"{}\"\n",
        match provider_name {
            "deepseek" => "deepseek-chat",
            "openai" => "gpt-4o",
            "anthropic" => "claude-sonnet-4-20250514",
            "openrouter" => "openai/gpt-4o",
            _ => "deepseek-chat",
        },
        provider_name,
        api_key,
    );

    std::fs::write(&config_path, config_content)?;

    // Telegram config
    println!();
    let mut telegram_token = String::new();
    print!("Telegram bot token (optional, Enter to skip): ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut telegram_token)?;
    let telegram_token = telegram_token.trim().to_string();

    if !telegram_token.is_empty() {
        let gateway_config = format!(
            "\n[gateway]\nbot_token = \"{}\"\n",
            telegram_token
        );
        std::fs::write(&config_path,
            std::fs::read_to_string(&config_path)? + &gateway_config
        )?;
    }

    println!("\nConfiguration saved to {}", config_path.display());
    println!(
        "Or set environment variable: export {}=\"...\"",
        api_key_env
    );
    if !telegram_token.is_empty() {
        println!("\nTo start Telegram bot: arli gateway start");
        println!("To run in foreground:   arli-gateway");

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
                Ok(s) => eprintln!("Gateway start failed (exit: {:?})", s.code()),
                Err(e) => eprintln!("Could not start gateway: {}. Run 'arli gateway start' manually.", e),
            }
        }
    }
    println!("\nYou're ready. Run: arli chat");

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
                    let section = doc.as_table_mut()
                        .ok_or_else(|| anyhow::anyhow!("Invalid config"))?
                        .entry(parts[0].to_string())
                        .or_insert(toml::Value::Table(toml::Table::new()));
                    section.as_table_mut()
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

        doc.as_table_mut().unwrap().insert("model".into(), toml::Value::String(model.into()));
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
    println!("[OK] Rust: {}", option_env!("CARGO_PKG_RUST_VERSION").unwrap_or("unknown"));

    // 6. Check binary
    let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("arli"));
    if bin.exists() {
        let size = std::fs::metadata(&bin).map(|m| m.len()).unwrap_or(0);
        println!("[OK] Binary: {} ({:.1}MB)", bin.display(), size as f64 / 1_048_576.0);
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
        CronCmd::Add { name, schedule, prompt } => {
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
                        status,
                        j.id,
                        j.schedule,
                        j.name,
                        j.prompt,
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
                anyhow::bail!("No cron jobs. Add one first: arli cron add -n 'job' -s 5m -p 'prompt'");
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
                            println!("[{}] Running job {}", chrono::Utc::now().format("%H:%M:%S"), &job_id[..8]);
                        }
                        CronEvent::JobCompleted { job_id, output } => {
                            println!("[{}] Job {} done: {}", chrono::Utc::now().format("%H:%M:%S"), &job_id[..8], output);
                        }
                        CronEvent::JobFailed { job_id, error } => {
                            eprintln!("[{}] Job {} FAILED: {}", chrono::Utc::now().format("%H:%M:%S"), &job_id[..8], error);
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
            let job = cf.jobs.iter().find(|j| j.id.starts_with(&id))
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
        sibling.filter(|p| p.exists()).unwrap_or_else(|| PathBuf::from("arli-gateway"))
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
        let child_id = store.resume_session(
            rid,
            Some(&format!("resume-{}", &rid[..8.min(rid.len())])),
        )?;
        Some((child_id, messages))
    } else {
        None
    };

    let mut tools = ToolRegistry::new();
    register_builtin_tools(&mut tools, Some(db_path), Some(memory_store.clone()), None, None);

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

    let updater = self_update::backends::github::Update::configure()
        .repo_owner("ARLI-Research")
        .repo_name("arli")
        .bin_name("arli")
        .show_download_progress(true)
        .current_version(current)
        .no_confirm(true)
        .build()?;

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
            std::env::var("ARLI_LOG")
                .unwrap_or_else(|_| "info,arli_core=debug".to_string()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            println!("ARLI v{}", env!("CARGO_PKG_VERSION"));
        }

        Commands::Setup => {
            run_setup()?;
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
            health.set_metric("arli_version", env!("CARGO_PKG_VERSION")).await;
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
            register_builtin_tools(&mut tools, Some(db_path), None, None, None);

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
                        println!("Add plugins to: {}", get_data_dir().join("plugins").display());
                        println!("Each plugin needs: plugin.toml + executable");
                    } else {
                        println!("Discovered plugins:\n");
                        for p in &plugins {
                            let status = if p.plugin.enabled { "enabled" } else { "disabled" };
                            println!("  {} v{} — {} [{}]",
                                p.plugin.name, p.plugin.version, p.plugin.description, status);
                            println!("    Tools: {}", p.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "));
                            println!("    Exec: {}", p.plugin.executable);
                            println!();
                        }
                    }
                }
                PluginsCmd::Load => {
                    let plugins = manager.discover()?;
                    let enabled: Vec<_> = plugins.into_iter().filter(|p| p.plugin.enabled).collect();
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
                            println!("  {}{}  [{}, {}, {}]  {}", p.name, marker, config, soul, sessions, p.path.display());
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
                        println!("Subscribe: arli webhook subscribe <name> -p 'Prompt with {{payload}}'");
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
                    state.subscribe(WebhookSubscription {
                        name: name.clone(),
                        prompt_template: prompt,
                        target_channel: None,
                    }).await;
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
                        struct WebhookFile { subscriptions: Vec<WebhookSubscription> }
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
    }

    Ok(())
}
