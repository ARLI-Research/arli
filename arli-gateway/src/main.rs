//! ARLI Gateway — multi-platform messaging bridge.
//!
//! Runs multiple platform adapters (Telegram, Discord, Slack, WhatsApp)
//! in parallel. Each platform routes messages to session-scoped Agents.
//!
//! Configuration (env vars or config.toml [gateway] section):
//!   TELEGRAM_BOT_TOKEN   — Telegram bot token
//!   DISCORD_BOT_TOKEN    — Discord bot token
//!   SLACK_BOT_TOKEN      — Slack bot token (xoxb-...)
//!   SLACK_APP_TOKEN      — Slack app token (xapp-...) for Socket Mode
//!   WHATSAPP_PHONE_ID    — WhatsApp Cloud API phone number ID
//!   WHATSAPP_TOKEN       — WhatsApp Cloud API access token
//!   WHATSAPP_VERIFY      — WhatsApp webhook verify token
//!   WHATSAPP_PORT        — WhatsApp webhook port (default: 3000)
//!
//! Daemon mode:
//!   arli-gateway --daemon           Start in background (writes PID to ~/.arli/gateway.pid)
//!   arli-gateway --daemon --pid-file /path/to/pid

mod discord;
mod slack;
mod telegram;
mod whatsapp;

use clap::Parser;
use std::fs;
use std::path::PathBuf;
use tracing::info;

/// ARLI Gateway — multi-platform AI agent messaging bridge
#[derive(Parser)]
#[command(name = "arli-gateway", version, about)]
struct Cli {
    /// Run as a background daemon (fork, detach, write PID file)
    #[arg(long)]
    daemon: bool,

    /// PID file path (default: ~/.arli/gateway.pid)
    #[arg(long, default_value = "")]
    pid_file: String,

    /// Log file path for daemon mode (default: ~/.arli/gateway.log)
    #[arg(long, default_value = "")]
    log_file: String,
}

fn arli_data_dir() -> PathBuf {
    std::env::var("ARLI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".arli"))
                .unwrap_or_else(|_| PathBuf::from(".arli"))
        })
}

fn resolve_token(env_var: &str, config_key: &str) -> Option<String> {
    std::env::var(env_var).ok().or_else(|| {
        let config_path = arli_data_dir().join("config.toml");
        if config_path.exists() {
            std::fs::read_to_string(&config_path).ok().and_then(|s| {
                toml::from_str::<toml::Value>(&s).ok().and_then(|v| {
                    let gateway = v.get("gateway")?;
                    // Try the canonical key first, then legacy bot_token
                    gateway.get(config_key)
                        .or_else(|| gateway.get("bot_token"))
                        .and_then(|val| val.as_str().map(String::from))
                })
            })
        } else {
            None
        }
    })
}

/// Fork and detach the process as a daemon.
///
/// Returns the PID of the child process from the parent's perspective.
/// The child continues running; the parent exits after printing the PID.
fn daemonize(pid_file: &str, log_file: &str) -> anyhow::Result<()> {
    // First fork: parent exits, child continues
    match unsafe { libc::fork() } {
        -1 => anyhow::bail!("fork failed: {}", std::io::Error::last_os_error()),
        0 => {
            // Child continues
        }
        pid => {
            // Parent: write PID and exit
            println!("Gateway daemon started (PID: {})", pid);
            if !pid_file.is_empty() {
                fs::write(pid_file, pid.to_string())?;
            }
            std::process::exit(0);
        }
    }

    // Create new session (become session leader, detach from terminal)
    if unsafe { libc::setsid() } == -1 {
        anyhow::bail!("setsid failed: {}", std::io::Error::last_os_error());
    }

    // Second fork: prevent re-acquiring a controlling terminal
    match unsafe { libc::fork() } {
        -1 => anyhow::bail!("second fork failed: {}", std::io::Error::last_os_error()),
        0 => {
            // Grandchild continues as daemon
        }
        _ => {
            std::process::exit(0);
        }
    }

    // Change working directory to root
    unsafe { libc::chdir(b"/\0".as_ptr() as *const _) };

    // Set file creation mask
    unsafe { libc::umask(0o022) };

    // Redirect stdio to log file (or /dev/null)
    let log_target = if log_file.is_empty() {
        "/dev/null".to_string()
    } else {
        log_file.to_string()
    };

    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_target)?;

    unsafe {
        libc::dup2(log.as_raw_fd(), 0); // stdin
        libc::dup2(log.as_raw_fd(), 1); // stdout
        libc::dup2(log.as_raw_fd(), 2); // stderr
    }

    Ok(())
}

use std::os::fd::AsRawFd;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // ── Daemon mode ──
    if cli.daemon {
        let data_dir = arli_data_dir();
        let pid_file = if cli.pid_file.is_empty() {
            data_dir.join("gateway.pid").display().to_string()
        } else {
            cli.pid_file
        };
        let log_file = if cli.log_file.is_empty() {
            data_dir.join("gateway.log").display().to_string()
        } else {
            cli.log_file
        };

        daemonize(&pid_file, &log_file)?;
        // After daemonize, we're the child. Write our PID.
        if !pid_file.is_empty() {
            fs::write(&pid_file, std::process::id().to_string())?;
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("ARLI_LOG")
                .unwrap_or_else(|_| "info,arli_gateway=debug".to_string()),
        )
        .with_writer(std::io::stderr)
        .init();

    let data_dir = arli_data_dir();
    std::fs::create_dir_all(&data_dir)?;

    let mut handles = Vec::new();

    // ── Telegram ──
    if resolve_token("TELEGRAM_BOT_TOKEN", "telegram_token").is_some() {
        info!("Platform: Telegram");
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = telegram::run(dd).await {
                tracing::error!("Telegram gateway died: {}", e);
            }
        }));
    }

    // ── Discord ──
    if let Some(token) = resolve_token("DISCORD_BOT_TOKEN", "discord_token") {
        info!("Platform: Discord");
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = discord::run(token, dd).await {
                tracing::error!("Discord gateway died: {}", e);
            }
        }));
    }

    // ── Slack ──
    let slack_bot = resolve_token("SLACK_BOT_TOKEN", "slack_bot_token");
    let slack_app = resolve_token("SLACK_APP_TOKEN", "slack_app_token");
    if let (Some(bot_token), Some(app_token)) = (slack_bot, slack_app) {
        info!("Platform: Slack (Socket Mode)");
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = slack::run(bot_token, app_token, dd).await {
                tracing::error!("Slack gateway died: {}", e);
            }
        }));
    }

    // ── WhatsApp ──
    let wa_phone = resolve_token("WHATSAPP_PHONE_ID", "whatsapp_phone_id");
    let wa_token = resolve_token("WHATSAPP_TOKEN", "whatsapp_token");
    let wa_verify = resolve_token("WHATSAPP_VERIFY", "whatsapp_verify");
    if let (Some(phone_id), Some(token), Some(verify)) = (wa_phone, wa_token, wa_verify) {
        let port: u16 = std::env::var("WHATSAPP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3000);
        info!("Platform: WhatsApp (port {})", port);
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = whatsapp::run(phone_id, token, verify, port, dd).await {
                tracing::error!("WhatsApp gateway died: {}", e);
            }
        }));
    }

    if handles.is_empty() {
        // Backward compat: if no tokens, default to Telegram
        info!("No platform tokens configured. Defaulting to Telegram...");
        telegram::run(data_dir).await?;
    } else {
        info!("Gateway running {} platform(s)", handles.len());
        for handle in handles {
            let _ = handle.await;
        }
    }

    Ok(())
}
