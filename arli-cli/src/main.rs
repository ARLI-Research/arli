mod tui;

use clap::{Parser, Subcommand};
use arli_core::{
    Agent, AgentConfig, Config, SessionStore, ToolRegistry,
    OpenAIProvider, PolicyEngine,
};
use arli_core::tools::builtin::register_builtin_tools;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::info;

#[derive(Parser)]
#[command(name = "arli", about = "ARLI — your agent harness")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Model to use (overrides config/env)
    #[arg(short, long)]
    model: Option<String>,

    /// Max tool-calling iterations
    #[arg(short, long, default_value = "20")]
    iterations: usize,
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive chat (default)
    Chat {
        /// Single query, non-interactive
        #[arg(short = 'q', long)]
        query: Option<String>,
    },

    /// List recent sessions
    Sessions,

    /// Show version
    Version,

    /// Start health check server
    Serve {
        #[arg(short, long, default_value = "3001")]
        port: u16,
    },
}

fn get_data_dir() -> PathBuf {
    let base = std::env::var("PROMETHEUS_HOME")
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("PROMETHEUS_LOG")
                .unwrap_or_else(|_| "info,arli_core=debug".to_string()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            println!("ARLI v{}", env!("CARGO_PKG_VERSION"));
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
                println!("No sessions yet.");
            } else {
                println!("Recent sessions:");
                for s in &sessions {
                    println!(
                        "  {}  {}  {}  [{}]",
                        s.id, s.updated_at, s.name, s.status
                    );
                }
            }
        }

        Commands::Chat { query } => {
            // Load config
            let config = Config::from_env()?;
            let model = cli.model.unwrap_or(config.model);

            info!("Using model: {} via {}", model, config.provider.name);

            // Create provider
            let provider = Box::new(OpenAIProvider::new(
                config.provider.api_key.clone(),
                model.clone(),
                config.provider.base_url.clone(),
            ));

            // Create data directory
            let data_dir = get_data_dir();
            let db_path = data_dir.join("sessions.db");
            let memory_path = data_dir.join("memory.db");

            // Create session store
            let store = SessionStore::open(db_path.clone())?;

            // Create memory store
            use arli_core::memory::MemoryStore;
            use std::sync::Arc;
            let memory_store = Arc::new(MemoryStore::open(memory_path)?);

            // Create tool registry (with session_search + memory)
            let mut tools = ToolRegistry::new();
            register_builtin_tools(&mut tools, Some(db_path), Some(memory_store.clone()), None);

            // Session store for agent
            let session = Some(store);

            // Create agent
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
                cli.iterations.max(1),
            );

            match query {
                Some(q) => {
                    // Single-shot mode
                    info!("Processing query: {}", q);
                    let sender = agent.sender();
                    let rx = agent.sender(); // keep one for agent

                    // Run agent in background
                    let handle = tokio::spawn(async move {
                        agent.run(Some(q)).await
                    });

                    // Keep sender alive
                    drop(sender);
                    drop(rx);

                    match handle.await? {
                        Ok(response) => {
                            println!("\n{}\n", response);
                        }
                        Err(e) => {
                            eprintln!("Error: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                None => {
                    // TUI mode
                    let sender = agent.sender();
                    let (response_tx, response_rx) = mpsc::channel::<String>(64);

                    // Background agent loop: keeps processing messages
                    tokio::spawn(async move {
                        loop {
                            match agent.run(None).await {
                                Ok(response) => {
                                    if response_tx.send(response).await.is_err() {
                                        break; // TUI closed
                                    }
                                }
                                Err(e) => {
                                    let _ = response_tx
                                        .send(format!("Error: {}", e))
                                        .await;
                                }
                            }
                        }
                    });

                    // Launch TUI
                    tui::run_tui(sender, response_rx).await?;
                }
            }
        }
    }

    Ok(())
}
