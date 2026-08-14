use std::{env, fs, path::Path, sync::Arc};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use gemini_rust_gateway::{config::Config, gemini::GeminiClient};
use rand::{Rng, distr::Alphanumeric};
use tokio::net::TcpListener;

#[derive(Debug, Parser)]
#[command(
    name = "gemini-rust-gateway",
    version,
    about = "Native Rust Gemini Web gateway",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the HTTP gateway (the default command).
    Serve,
    /// Create persistent local configuration interactively.
    Init {
        /// Replace existing .env and .env.auth.local files.
        #[arg(long)]
        force: bool,
        /// Read credentials from environment variables instead of prompting.
        #[arg(long)]
        non_interactive: bool,
    },
    /// Validate configuration and optionally verify the Gemini session.
    Doctor {
        /// Connect to Gemini and validate the configured cookies.
        #[arg(long)]
        online: bool,
    },
}

pub fn init(force: bool, non_interactive: bool) -> anyhow::Result<()> {
    let root = env::current_dir().context("cannot determine the current directory")?;
    let env_path = root.join(".env");
    let auth_path = root.join(".env.auth.local");
    if !force && (env_path.exists() || auth_path.exists()) {
        bail!("configuration already exists; run with --force only if you intend to replace it");
    }

    let secure_1psid = credential(
        "GEMINI_SECURE_1PSID",
        "Paste __Secure-1PSID, or press Enter to use the Chrome sync extension: ",
        non_interactive,
        false,
    )?;
    let secure_1psidts = credential(
        "GEMINI_SECURE_1PSIDTS",
        "Paste __Secure-1PSIDTS, or press Enter if unavailable (input is hidden): ",
        non_interactive,
        false,
    )?;
    let auth_user = env::var("GEMINI_AUTH_USER")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let api_key = env::var("GEMINI_API_KEY")
        .ok()
        .filter(|value| value.len() >= 8)
        .unwrap_or_else(generate_api_key);
    let admin_token = env::var("GEMINI_ADMIN_TOKEN")
        .ok()
        .filter(|value| value.len() >= 8)
        .unwrap_or_else(generate_admin_token);

    validate_env_value("GEMINI_SECURE_1PSID", &secure_1psid)?;
    validate_env_value("GEMINI_SECURE_1PSIDTS", &secure_1psidts)?;
    validate_env_value("GEMINI_API_KEY", &api_key)?;
    validate_env_value("GEMINI_ADMIN_TOKEN", &admin_token)?;

    fs::write(&env_path, render_env(&api_key, &admin_token)).context("cannot write .env")?;
    fs::write(
        &auth_path,
        render_auth_env(auth_user, &secure_1psid, &secure_1psidts),
    )
    .context("cannot write .env.auth.local")?;
    restrict_auth_file(&auth_path)?;

    println!("Configuration created:");
    println!("  {}", env_path.display());
    println!("  {}", auth_path.display());
    println!("Run `gemini-rust-gateway doctor --online` to verify it.");
    println!("Run `gemini-rust-gateway` to start the server.");
    Ok(())
}

pub async fn doctor(online: bool) -> anyhow::Result<()> {
    println!("Gemini Rust Gateway doctor");
    let config = Config::load().context("configuration could not be loaded")?;
    config.validate().context("configuration is invalid")?;
    println!("[ok] configuration is valid");
    if config.secure_1psid.is_empty() {
        println!("[warn] Gemini cookies are not configured; use the Chrome sync extension");
    } else {
        println!("[ok] Gemini cookies are configured");
    }
    println!("[ok] Google account path: /u/{}", config.auth_user);
    println!(
        "[ok] local API key is configured ({} characters)",
        config.api_key.len()
    );
    println!("[ok] data directory: {}", config.data_root.display());

    match TcpListener::bind((config.host.as_str(), config.port)).await {
        Ok(listener) => {
            drop(listener);
            println!("[ok] {}:{} is available", config.host, config.port);
        }
        Err(error) => {
            println!(
                "[warn] {}:{} cannot be bound: {error}",
                config.host, config.port
            );
        }
    }

    if online {
        print!("[check] validating Gemini session... ");
        GeminiClient::initialize(Arc::new(config))
            .await
            .context("Gemini session validation failed")?;
        println!("ok");
    } else {
        println!("[skip] online session check (use `doctor --online` to enable)");
    }
    Ok(())
}

fn credential(
    env_name: &str,
    prompt: &str,
    non_interactive: bool,
    required: bool,
) -> anyhow::Result<String> {
    let value = if let Ok(value) = env::var(env_name) {
        value
    } else if non_interactive {
        String::new()
    } else {
        rpassword::prompt_password(prompt).context("failed to read hidden input")?
    };
    let value = value.trim().to_owned();
    if required && value.is_empty() {
        bail!("{env_name} is required");
    }
    Ok(value)
}

fn generate_api_key() -> String {
    let random: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(40)
        .map(char::from)
        .collect();
    format!("grg_{random}")
}

fn generate_admin_token() -> String {
    let random: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(40)
        .map(char::from)
        .collect();
    format!("grg_admin_{random}")
}

fn validate_env_value(name: &str, value: &str) -> anyhow::Result<()> {
    if value.contains(['\r', '\n']) {
        bail!("{name} must not contain a newline");
    }
    Ok(())
}

fn render_env(api_key: &str, admin_token: &str) -> String {
    format!(
        "# Generated by gemini-rust-gateway init\n\
GEMINI_API_KEY={api_key}\n\
GEMINI_ADMIN_TOKEN={admin_token}\n\
GEMINI_MODEL=gemini-3-flash\n\
GEMINI_ADVANCED_MODEL=gemini-3-pro\n\
GEMINI_HOST=127.0.0.1\n\
GEMINI_PORT=8000\n\
GEMINI_TIMEOUT=90\n\
GEMINI_RETRY_ATTEMPTS=3\n\
GEMINI_RETRY_DELAY_MS=800\n\
GEMINI_PROVIDER_ORIGIN=https://gemini.google.com\n\
GEMINI_MAX_CONCURRENCY=1\n\
GEMINI_MAX_ACCOUNT_CONCURRENCY=1\n\
GEMINI_ACCOUNT_COOLDOWN_SECONDS=60\n\
GEMINI_ACCOUNT_PROBE_SECONDS=60\n\
GEMINI_TASK_WORKERS=1\n\
GEMINI_MAX_TASK_QUEUE=20\n\
GEMINI_TASK_RETENTION_HOURS=168\n\
GEMINI_DATA_DIR=gateway_data\n\
GEMINI_ACCOUNTS_FILE=gateway_data/accounts.local.json\n\
GEMINI_PUBLIC_BASE_URL=http://127.0.0.1:8000\n\
GEMINI_ENABLED_GENERATIONS=image,audio,music,deep_research,canvas\n\
GEMINI_COOKIE_SYNC_ENABLED=true\n\
GEMINI_REMOTE_ADMIN_ENABLED=false\n"
    )
}

fn render_auth_env(auth_user: usize, secure_1psid: &str, secure_1psidts: &str) -> String {
    format!(
        "# Generated by gemini-rust-gateway init; never commit this file.\n\
GEMINI_AUTH_USER={auth_user}\n\
GEMINI_SECURE_1PSID={secure_1psid}\n\
GEMINI_SECURE_1PSIDTS={secure_1psidts}\n"
    )
}

#[cfg(unix)]
fn restrict_auth_file(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .context("cannot restrict .env.auth.local permissions")
}

#[cfg(not(unix))]
fn restrict_auth_file(_: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_api_key_is_long_enough() {
        let key = generate_api_key();
        assert!(key.starts_with("grg_"));
        assert!(key.len() >= 40);
    }

    #[test]
    fn rendered_files_do_not_mix_credentials() {
        let regular = render_env("api-key-123456", "admin-key-123456");
        let auth = render_auth_env(1, "psid-value", "psidts-value");
        assert!(!regular.contains("psid-value"));
        assert!(auth.contains("GEMINI_SECURE_1PSID=psid-value"));
        assert!(auth.contains("GEMINI_AUTH_USER=1"));
    }

    #[test]
    fn rejects_newlines_in_values() {
        assert!(validate_env_value("TEST", "unsafe\nvalue").is_err());
    }
}
