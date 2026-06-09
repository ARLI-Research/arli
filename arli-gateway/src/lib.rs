//! ARLI Gateway — multi-platform messaging bridge (library).
//!
//! Called by both the standalone `arli-gateway` binary and
//! `arli gateway start` / `arli --__gateway-daemon`.

mod bluebubbles;
mod dingtalk;
mod discord;
mod email;
mod feishu;
mod google_chat;
mod health;
mod irc;
mod line;
mod matrix;
mod ntfy;
mod qq;
mod signal;
mod simplex;
mod slack;
mod sms;
mod teams;
mod telegram;
mod wecom;
mod whatsapp;
mod yuanbao;

use std::fs;
use std::path::PathBuf;
use tracing::info;

pub fn arli_data_dir() -> PathBuf {
    std::env::var("ARLI_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".arli"))
                .unwrap_or_else(|_| PathBuf::from(".arli"))
        })
}

pub fn resolve_token(env_var: &str, config_key: &str) -> Option<String> {
    std::env::var(env_var).ok().or_else(|| {
        let config_path = arli_data_dir().join("config.toml");
        if config_path.exists() {
            std::fs::read_to_string(&config_path).ok().and_then(|s| {
                toml::from_str::<toml::Value>(&s).ok().and_then(|v| {
                    let gateway = v.get("gateway")?;
                    gateway
                        .get(config_key)
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
pub fn daemonize(pid_file: &str, log_file: &str) -> anyhow::Result<()> {
    match unsafe { libc::fork() } {
        -1 => anyhow::bail!("fork failed: {}", std::io::Error::last_os_error()),
        0 => {
            // Child continues
        }
        pid => {
            println!("Gateway daemon started (PID: {})", pid);
            if !pid_file.is_empty() {
                fs::write(pid_file, pid.to_string())?;
            }
            std::process::exit(0);
        }
    }

    if unsafe { libc::setsid() } == -1 {
        anyhow::bail!("setsid failed: {}", std::io::Error::last_os_error());
    }

    match unsafe { libc::fork() } {
        -1 => anyhow::bail!("second fork failed: {}", std::io::Error::last_os_error()),
        0 => {}
        _ => {
            std::process::exit(0);
        }
    }

    #[allow(clippy::manual_c_str_literals)]
    unsafe {
        libc::chdir(b"/\0".as_ptr() as *const _)
    };

    unsafe { libc::umask(0o022) };

    let log_target = if log_file.is_empty() {
        "/dev/null".to_string()
    } else {
        log_file.to_string()
    };

    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_target)?;

    use std::os::fd::AsRawFd;
    unsafe {
        libc::dup2(log.as_raw_fd(), 0);
        libc::dup2(log.as_raw_fd(), 1);
        libc::dup2(log.as_raw_fd(), 2);
    }

    Ok(())
}

/// Main gateway entry point.
///
/// If `daemon` is true, forks into background before starting the async runtime.
/// Call this from:
///   - `arli-gateway --daemon` (standalone binary)
///   - `arli --__gateway-daemon` (merged binary)
///   - `arli gateway start` (via subprocess of self)
pub fn run(daemon: bool, pid_file: &str, log_file: &str) -> anyhow::Result<()> {
    if daemon {
        let data_dir = arli_data_dir();
        let pid = if pid_file.is_empty() {
            data_dir.join("gateway.pid").display().to_string()
        } else {
            pid_file.to_string()
        };
        let log = if log_file.is_empty() {
            data_dir.join("gateway.log").display().to_string()
        } else {
            log_file.to_string()
        };

        daemonize(&pid, &log)?;
        // After daemonize, we're the grandchild. Write our PID.
        if !pid.is_empty() {
            fs::write(&pid, std::process::id().to_string())?;
        }
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("ARLI_LOG").unwrap_or_else(|_| "info,arli_gateway=debug".to_string()),
        )
        .with_writer(std::io::stderr)
        .init();

    let data_dir = arli_data_dir();
    std::fs::create_dir_all(&data_dir)?;

    // ── Health check server ──
    let health_state = health::HealthState::new();
    health::serve(health_state.clone()).await?;

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

    // ── Email ──
    if std::env::var("EMAIL_USER").is_ok() {
        info!("Platform: Email (IMAP/SMTP)");
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = email::run(dd).await {
                tracing::error!("Email gateway died: {}", e);
            }
        }));
    }

    // ── Matrix ──
    if resolve_token("MATRIX_USER", "matrix_user").is_some() {
        info!("Platform: Matrix");
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = matrix::run(dd).await {
                tracing::error!("Matrix gateway died: {}", e);
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

    // ── Microsoft Teams ──
    let teams_app_id = resolve_token("MS_TEAMS_APP_ID", "teams_app_id");
    let teams_app_password = resolve_token("MS_TEAMS_APP_PASSWORD", "teams_app_password");
    if let (Some(app_id), Some(app_password)) = (teams_app_id, teams_app_password) {
        let port: u16 = std::env::var("MS_TEAMS_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3007);
        info!("Platform: Microsoft Teams (port {})", port);
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = teams::run(app_id, app_password, port, dd).await {
                tracing::error!("Teams gateway died: {}", e);
            }
        }));
    }

    // ── Signal ──
    if std::env::var("SIGNAL_PHONE_NUMBER").is_ok() {
        info!("Platform: Signal");
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = signal::run(dd).await {
                tracing::error!("Signal gateway died: {}", e);
            }
        }));
    }

    // ── SMS / Twilio ──
    let twilio_sid = resolve_token("TWILIO_ACCOUNT_SID", "twilio_account_sid");
    let twilio_token = resolve_token("TWILIO_AUTH_TOKEN", "twilio_auth_token");
    let twilio_phone = resolve_token("TWILIO_PHONE_NUMBER", "twilio_phone_number");
    if let (Some(sid), Some(token), Some(phone)) = (twilio_sid, twilio_token, twilio_phone) {
        let port: u16 = std::env::var("SMS_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3008);
        info!("Platform: SMS/Twilio (port {})", port);
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = sms::run(sid, token, phone, port, dd).await {
                tracing::error!("SMS gateway died: {}", e);
            }
        }));
    }

    // ── Google Chat ──
    if resolve_token(
        "GOOGLE_CHAT_VERIFICATION_TOKEN",
        "google_chat_verification_token",
    )
    .is_some()
    {
        let port: u16 = std::env::var("GOOGLE_CHAT_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3009);
        info!("Platform: Google Chat (port {})", port);
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = google_chat::run(port, dd).await {
                tracing::error!("Google Chat gateway died: {}", e);
            }
        }));
    }

    // ── ntfy ──
    if std::env::var("NTFY_TOPIC").is_ok() {
        info!("Platform: ntfy");
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = ntfy::run(dd).await {
                tracing::error!("ntfy gateway died: {}", e);
            }
        }));
    }

    // ── QQ Bot ──
    if std::env::var("QQ_APP_ID").is_ok() {
        info!("Platform: QQ Bot");
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = qq::run(dd).await {
                tracing::error!("QQ Bot gateway died: {}", e);
            }
        }));
    }

    // ── SimpleX Chat ──
    {
        let simplex_path =
            std::env::var("SIMPLEX_CLI_PATH").unwrap_or_else(|_| "simplex-chat".to_string());
        if std::path::Path::new(&simplex_path).exists() || std::env::var("SIMPLEX_CLI_PATH").is_ok()
        {
            info!("Platform: SimpleX Chat");
            let dd = data_dir.clone();
            handles.push(tokio::spawn(async move {
                if let Err(e) = simplex::run(dd).await {
                    tracing::error!("SimpleX gateway died: {}", e);
                }
            }));
        }
    }

    // ── Yuanbao ──
    let yuanbao_app_id = resolve_token("YUANBAO_APP_ID", "yuanbao_app_id");
    let yuanbao_app_secret = resolve_token("YUANBAO_APP_SECRET", "yuanbao_app_secret");
    if let (Some(app_id), Some(app_secret)) = (yuanbao_app_id, yuanbao_app_secret) {
        let port: u16 = std::env::var("YUANBAO_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3014);
        info!("Platform: Yuanbao (port {})", port);
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = yuanbao::run(app_id, app_secret, port, dd).await {
                tracing::error!("Yuanbao gateway died: {}", e);
            }
        }));
    }

    // ── BlueBubbles (iMessage) ──
    if std::env::var("BLUEBUBBLES_PASSWORD").is_ok() {
        info!("Platform: BlueBubbles (iMessage)");
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = bluebubbles::run(dd).await {
                tracing::error!("BlueBubbles gateway died: {}", e);
            }
        }));
    }

    // ── Feishu / Lark ──
    let feishu_app_id = resolve_token("FEISHU_APP_ID", "feishu_app_id");
    let feishu_app_secret = resolve_token("FEISHU_APP_SECRET", "feishu_app_secret");
    let feishu_verify = resolve_token("FEISHU_VERIFICATION_TOKEN", "feishu_verification_token");
    if let (Some(app_id), Some(app_secret), Some(verify)) =
        (feishu_app_id, feishu_app_secret, feishu_verify)
    {
        let port: u16 = std::env::var("FEISHU_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3010);
        info!("Platform: Feishu (port {})", port);
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = feishu::run(app_id, app_secret, verify, port, dd).await {
                tracing::error!("Feishu gateway died: {}", e);
            }
        }));
    }

    // ── DingTalk ──
    let dingtalk_app_key = resolve_token("DINGTALK_APP_KEY", "dingtalk_app_key");
    let dingtalk_app_secret = resolve_token("DINGTALK_APP_SECRET", "dingtalk_app_secret");
    if let (Some(app_key), Some(app_secret)) = (dingtalk_app_key, dingtalk_app_secret) {
        let port: u16 = std::env::var("DINGTALK_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3011);
        info!("Platform: DingTalk (port {})", port);
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = dingtalk::run(app_key, app_secret, port, dd).await {
                tracing::error!("DingTalk gateway died: {}", e);
            }
        }));
    }

    // ── LINE ──
    let line_secret = resolve_token("LINE_CHANNEL_SECRET", "line_channel_secret");
    let line_token = resolve_token("LINE_CHANNEL_ACCESS_TOKEN", "line_channel_access_token");
    if let (Some(secret), Some(access_token)) = (line_secret, line_token) {
        let port: u16 = std::env::var("LINE_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3012);
        info!("Platform: LINE (port {})", port);
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = line::run(secret, access_token, port, dd).await {
                tracing::error!("LINE gateway died: {}", e);
            }
        }));
    }

    // ── IRC ──
    if std::env::var("IRC_NICK").is_ok() {
        info!("Platform: IRC");
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = irc::run(dd).await {
                tracing::error!("IRC gateway died: {}", e);
            }
        }));
    }

    // ── WeCom / Enterprise WeChat ──
    let wecom_corp_id = resolve_token("WECOM_CORP_ID", "wecom_corp_id");
    let wecom_agent_secret = resolve_token("WECOM_AGENT_SECRET", "wecom_agent_secret");
    let wecom_token = resolve_token("WECOM_TOKEN", "wecom_token");
    let wecom_aes_key = resolve_token("WECOM_ENCODING_AES_KEY", "wecom_encoding_aes_key");
    if let (Some(corp_id), Some(agent_secret), Some(token), Some(aes_key)) = (
        wecom_corp_id,
        wecom_agent_secret,
        wecom_token,
        wecom_aes_key,
    ) {
        let port: u16 = std::env::var("WECOM_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(3013);
        info!("Platform: WeCom (port {})", port);
        let dd = data_dir.clone();
        handles.push(tokio::spawn(async move {
            if let Err(e) = wecom::run(corp_id, agent_secret, token, aes_key, port, dd).await {
                tracing::error!("WeCom gateway died: {}", e);
            }
        }));
    }

    if handles.is_empty() {
        info!("No platform tokens configured. Defaulting to Telegram...");
        health_state.mark_ready();
        telegram::run(data_dir).await?;
    } else {
        info!("Gateway running {} platform(s)", handles.len());
        health_state.mark_ready();
        for handle in handles {
            let _ = handle.await;
        }
    }

    Ok(())
}
