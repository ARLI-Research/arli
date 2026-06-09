//! ARLI Gateway — multi-platform messaging bridge (standalone binary).
//!
//! Thin wrapper around `arli_gateway::run()`. The same logic is also
//! available via `arli gateway start` / `arli --__gateway-daemon`.

use clap::Parser;

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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    arli_gateway::run(cli.daemon, &cli.pid_file, &cli.log_file)
}
