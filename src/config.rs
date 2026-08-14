use std::{collections::HashMap, path::PathBuf, time::Duration};

use crate::error::{GatewayError, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub project_root: PathBuf,
    pub host: String,
    pub port: u16,
    pub api_key: String,
    pub admin_token: String,
    pub secure_1psid: String,
    pub secure_1psidts: Option<String>,
    pub auth_user: usize,
    pub accounts_file: PathBuf,
    pub account_cooldown: Duration,
    pub account_probe_interval: Duration,
    pub proxy: Option<String>,
    pub provider_origin: String,
    pub retry_attempts: usize,
    pub retry_delay: Duration,
    pub cookie_sync_enabled: bool,
    pub remote_admin_enabled: bool,
    pub default_model: String,
    pub advanced_model: String,
    pub enabled_generations: Vec<String>,
    pub data_root: PathBuf,
    pub public_base_url: String,
    pub max_concurrency: usize,
    pub max_account_concurrency: usize,
    pub task_workers: usize,
    pub max_task_queue: usize,
    pub task_retention: Duration,
    pub queue_timeout: Duration,
    pub provider_timeout: Duration,
    pub max_messages: usize,
    pub max_prompt_chars: usize,
    pub max_request_bytes: usize,
    pub cors_origins: Vec<String>,
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.api_key.trim().is_empty() {
            return Err(GatewayError::InvalidRequest(
                "GEMINI_API_KEY must not be empty".into(),
            ));
        }
        if self.api_key.len() < 8 {
            return Err(GatewayError::InvalidRequest(
                "GEMINI_API_KEY must be at least 8 characters".into(),
            ));
        }
        if self.admin_token.len() < 8 {
            return Err(GatewayError::InvalidRequest(
                "GEMINI_ADMIN_TOKEN must be at least 8 characters".into(),
            ));
        }
        if self.secure_1psid.trim().is_empty() && !self.cookie_sync_enabled {
            return Err(GatewayError::InvalidRequest(
                "GEMINI_SECURE_1PSID must not be empty".into(),
            ));
        }
        if self.retry_attempts == 0 {
            return Err(GatewayError::InvalidRequest(
                "GEMINI_RETRY_ATTEMPTS must be >= 1".into(),
            ));
        }
        let valid_kinds = [
            "image",
            "video",
            "audio",
            "music",
            "deep_research",
            "canvas",
        ];
        for kind in &self.enabled_generations {
            if !valid_kinds.contains(&kind.as_str()) {
                return Err(GatewayError::InvalidRequest(format!(
                    "unknown generation kind in GEMINI_ENABLED_GENERATIONS: {kind}"
                )));
            }
        }
        if self.enabled_generations.is_empty() {
            return Err(GatewayError::InvalidRequest(
                "GEMINI_ENABLED_GENERATIONS must contain at least one kind".into(),
            ));
        }
        if url::Url::parse(&self.public_base_url).is_err() {
            return Err(GatewayError::InvalidRequest(format!(
                "GEMINI_PUBLIC_BASE_URL is not a valid URL: {}",
                self.public_base_url
            )));
        }
        let provider_origin = url::Url::parse(&self.provider_origin).map_err(|_| {
            GatewayError::InvalidRequest(format!(
                "GEMINI_PROVIDER_ORIGIN is not a valid URL: {}",
                self.provider_origin
            ))
        })?;
        if !matches!(provider_origin.scheme(), "http" | "https") {
            return Err(GatewayError::InvalidRequest(
                "GEMINI_PROVIDER_ORIGIN must use http or https".into(),
            ));
        }
        if self.max_concurrency == 0 {
            return Err(GatewayError::InvalidRequest(
                "GEMINI_MAX_CONCURRENCY must be >= 1".into(),
            ));
        }
        if self.task_workers == 0 {
            return Err(GatewayError::InvalidRequest(
                "GEMINI_TASK_WORKERS must be >= 1".into(),
            ));
        }
        if self.max_task_queue < self.task_workers {
            return Err(GatewayError::InvalidRequest(format!(
                "GEMINI_MAX_TASK_QUEUE ({}) must be >= GEMINI_TASK_WORKERS ({})",
                self.max_task_queue, self.task_workers
            )));
        }
        Ok(())
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let cwd = std::env::current_dir()?;
        let candidates = [cwd.join(".env"), cwd.join("..").join(".env")];
        let env_path = candidates.into_iter().find(|path| path.is_file());
        let project_root = env_path
            .as_ref()
            .and_then(|path| path.parent())
            .unwrap_or(&cwd)
            .to_path_buf();

        let mut values = HashMap::new();
        if let Some(path) = &env_path {
            load_env_file(path, &mut values)?;
        }

        // Authentication is machine-local and intentionally kept separate
        // from ordinary project settings. Values in this ignored file override
        // .env, so credentials only need to be entered once and survive restarts.
        let auth_path = project_root.join(".env.auth.local");
        if auth_path.is_file() {
            load_env_file(&auth_path, &mut values)?;
        }

        // Process environment has the highest priority. This enables Docker,
        // systemd and secret managers without requiring any local files.
        for (key, value) in std::env::vars().filter(|(key, _)| key.starts_with("GEMINI_")) {
            values.insert(key, value);
        }

        let required = |name: &str| -> Result<String> {
            values
                .get(name)
                .filter(|value| !value.is_empty())
                .cloned()
                .ok_or_else(|| {
                    GatewayError::InvalidRequest(format!(
                        "{name} is missing; run `gemini-rust-gateway init` or set it in the environment"
                    ))
                })
        };
        let setting = |name: &str, default: &str| {
            values
                .get(name)
                .filter(|value| !value.is_empty())
                .cloned()
                .unwrap_or_else(|| default.to_owned())
        };
        let parse = |name: &str, default: &str| -> Result<usize> {
            setting(name, default)
                .parse::<usize>()
                .map_err(|error| GatewayError::InvalidRequest(format!("invalid {name}: {error}")))
        };

        let port = setting("GEMINI_PORT", "8000")
            .parse::<u16>()
            .map_err(|error| {
                GatewayError::InvalidRequest(format!("invalid GEMINI_PORT: {error}"))
            })?;
        let data_setting = PathBuf::from(setting("GEMINI_DATA_DIR", "gateway_data"));
        let data_root = if data_setting.is_absolute() {
            data_setting
        } else {
            project_root.join(data_setting)
        };
        let accounts_file = values
            .get("GEMINI_ACCOUNTS_FILE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    project_root.join(path)
                }
            })
            .unwrap_or_else(|| data_root.join("accounts.local.json"));
        let public_base_url = setting(
            "GEMINI_PUBLIC_BASE_URL",
            &format!("http://127.0.0.1:{port}"),
        )
        .trim_end_matches('/')
        .to_owned();
        let enabled_generations = setting(
            "GEMINI_ENABLED_GENERATIONS",
            "image,video,audio,music,deep_research,canvas",
        )
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();

        let api_key = required("GEMINI_API_KEY")?;
        Ok(Self {
            project_root,
            host: setting("GEMINI_HOST", "127.0.0.1"),
            port,
            admin_token: setting("GEMINI_ADMIN_TOKEN", &api_key),
            api_key,
            secure_1psid: values
                .get("GEMINI_SECURE_1PSID")
                .cloned()
                .unwrap_or_default(),
            secure_1psidts: values
                .get("GEMINI_SECURE_1PSIDTS")
                .filter(|value| !value.is_empty())
                .cloned(),
            auth_user: parse("GEMINI_AUTH_USER", "0")?,
            accounts_file,
            account_cooldown: Duration::from_secs(
                parse("GEMINI_ACCOUNT_COOLDOWN_SECONDS", "60")? as u64
            ),
            account_probe_interval: Duration::from_secs(
                parse("GEMINI_ACCOUNT_PROBE_SECONDS", "60")?.max(5) as u64,
            ),
            proxy: resolve_proxy(&values),
            provider_origin: setting("GEMINI_PROVIDER_ORIGIN", "https://gemini.google.com")
                .trim_end_matches('/')
                .to_owned(),
            retry_attempts: parse("GEMINI_RETRY_ATTEMPTS", "3")?.clamp(1, 10),
            retry_delay: Duration::from_millis(parse("GEMINI_RETRY_DELAY_MS", "800")? as u64),
            cookie_sync_enabled: parse_bool(&setting("GEMINI_COOKIE_SYNC_ENABLED", "false"))?,
            remote_admin_enabled: parse_bool(&setting("GEMINI_REMOTE_ADMIN_ENABLED", "false"))?,
            default_model: setting("GEMINI_MODEL", "gemini-3-flash"),
            advanced_model: setting("GEMINI_ADVANCED_MODEL", "gemini-3-pro"),
            enabled_generations,
            data_root,
            public_base_url,
            max_concurrency: parse("GEMINI_MAX_CONCURRENCY", "1")?.max(1),
            max_account_concurrency: parse("GEMINI_MAX_ACCOUNT_CONCURRENCY", "1")?.max(1),
            task_workers: parse("GEMINI_TASK_WORKERS", "1")?.max(1),
            max_task_queue: parse("GEMINI_MAX_TASK_QUEUE", "20")?.max(1),
            task_retention: Duration::from_secs(
                parse("GEMINI_TASK_RETENTION_HOURS", "168")?.max(1) as u64 * 3600,
            ),
            queue_timeout: Duration::from_secs(parse("GEMINI_QUEUE_TIMEOUT", "30")? as u64),
            provider_timeout: Duration::from_secs(parse("GEMINI_TIMEOUT", "90")? as u64),
            max_messages: parse("GEMINI_MAX_MESSAGES", "50")?.max(1),
            max_prompt_chars: parse("GEMINI_MAX_PROMPT_CHARS", "200000")?.max(1),
            max_request_bytes: parse("GEMINI_MAX_REQUEST_BYTES", "1048576")?.max(1024),
            cors_origins: setting("GEMINI_CORS_ORIGINS", "")
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect(),
        })
    }
}

fn load_env_file(path: &std::path::Path, values: &mut HashMap<String, String>) -> Result<()> {
    for item in dotenvy::from_path_iter(path)? {
        let (key, value) = item?;
        values.insert(key, value);
    }
    Ok(())
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(GatewayError::InvalidRequest(format!(
            "invalid boolean value: {value}"
        ))),
    }
}

fn resolve_proxy(values: &HashMap<String, String>) -> Option<String> {
    if let Some(value) = values.get("GEMINI_PROXY") {
        let value = value.trim();
        if value.eq_ignore_ascii_case("direct") || value.eq_ignore_ascii_case("none") {
            return None;
        }
        if !value.is_empty() {
            return Some(value.to_owned());
        }
    }
    let no_proxy = values
        .get("NO_PROXY")
        .or_else(|| values.get("no_proxy"))
        .cloned()
        .or_else(|| std::env::var("NO_PROXY").ok())
        .or_else(|| std::env::var("no_proxy").ok())
        .unwrap_or_default();
    if no_proxy.split(',').map(str::trim).any(|host| {
        host == "*"
            || host.eq_ignore_ascii_case("google.com")
            || host.eq_ignore_ascii_case(".google.com")
            || host.eq_ignore_ascii_case("gemini.google.com")
    }) {
        return None;
    }
    ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        .into_iter()
        .find_map(|name| {
            values
                .get(name)
                .cloned()
                .or_else(|| std::env::var(name).ok())
                .filter(|value| !value.trim().is_empty())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> Config {
        Config {
            project_root: PathBuf::from("."),
            host: "127.0.0.1".into(),
            port: 8000,
            api_key: "test-key-1234".into(),
            admin_token: "admin-key-1234".into(),
            secure_1psid: "psid".into(),
            secure_1psidts: None,
            auth_user: 0,
            accounts_file: PathBuf::from(".gemini.accounts.local.json"),
            account_cooldown: Duration::from_secs(60),
            account_probe_interval: Duration::from_secs(60),
            proxy: None,
            provider_origin: "https://gemini.google.com".into(),
            retry_attempts: 3,
            retry_delay: Duration::from_millis(10),
            cookie_sync_enabled: false,
            remote_admin_enabled: false,
            default_model: "gemini-3-flash".into(),
            advanced_model: "gemini-3-pro".into(),
            enabled_generations: vec!["image".into()],
            data_root: PathBuf::from("."),
            public_base_url: "http://127.0.0.1:8000".into(),
            max_concurrency: 1,
            max_account_concurrency: 1,
            task_workers: 1,
            max_task_queue: 20,
            task_retention: Duration::from_secs(7 * 24 * 3600),
            queue_timeout: Duration::from_secs(30),
            provider_timeout: Duration::from_secs(90),
            max_messages: 50,
            max_prompt_chars: 200000,
            max_request_bytes: 1048576,
            cors_origins: vec![],
        }
    }

    #[test]
    fn validate_accepts_valid_config() {
        assert!(base_config().validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_api_key() {
        let mut config = base_config();
        config.api_key = String::new();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_short_api_key() {
        let mut config = base_config();
        config.api_key = "short".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_unknown_generation_kind() {
        let mut config = base_config();
        config.enabled_generations = vec!["unknown".into()];
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_generations() {
        let mut config = base_config();
        config.enabled_generations = vec![];
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_invalid_url() {
        let mut config = base_config();
        config.public_base_url = "not a url".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_queue_smaller_than_workers() {
        let mut config = base_config();
        config.task_workers = 5;
        config.max_task_queue = 2;
        assert!(config.validate().is_err());
    }

    #[test]
    fn explicit_proxy_and_direct_mode_take_precedence() {
        let mut values = HashMap::new();
        values.insert("GEMINI_PROXY".into(), "http://127.0.0.1:7890".into());
        assert_eq!(
            resolve_proxy(&values).as_deref(),
            Some("http://127.0.0.1:7890")
        );
        values.insert("GEMINI_PROXY".into(), "direct".into());
        assert_eq!(resolve_proxy(&values), None);
    }
}
