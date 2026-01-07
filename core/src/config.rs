use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Proxy server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    
    #[serde(default)]
    pub auth: AuthConfig,
    
    #[serde(default)]
    pub accounts: AccountsConfig,
    
    #[serde(default)]
    pub timeouts: TimeoutsConfig,
    
    #[serde(default)]
    pub model_mapping: ModelMappingConfig,
    
    #[serde(default)]
    pub logging: LoggingConfig,
    
    #[serde(default)]
    pub scheduling: SchedulingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    
    #[serde(default = "default_host")]
    pub host: String,
    
    #[serde(default)]
    pub allow_lan_access: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            host: default_host(),
            allow_lan_access: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Off,
    Strict,
    AllExceptHealth,
}

impl Default for AuthMode {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    #[serde(default)]
    pub mode: AuthMode,
    
    #[serde(default)]
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountsConfig {
    #[serde(default = "default_accounts_dir")]
    pub directory: PathBuf,
}

impl Default for AccountsConfig {
    fn default() -> Self {
        Self {
            directory: default_accounts_dir(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutsConfig {
    #[serde(default = "default_request_timeout")]
    pub request_timeout: u64,
}

impl Default for TimeoutsConfig {
    fn default() -> Self {
        Self {
            request_timeout: default_request_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelMappingConfig {
    #[serde(default)]
    pub anthropic: HashMap<String, String>,
    
    #[serde(default)]
    pub openai: HashMap<String, String>,
    
    #[serde(default)]
    pub custom: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    
    #[serde(default)]
    pub enabled: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SchedulingMode {
    PerformanceFirst,
    Balance,
    CacheFirst,
}

impl Default for SchedulingMode {
    fn default() -> Self {
        Self::Balance
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulingConfig {
    #[serde(default)]
    pub mode: SchedulingMode,
    
    #[serde(default = "default_max_wait_seconds")]
    pub max_wait_seconds: u64,
}

impl Default for SchedulingConfig {
    fn default() -> Self {
        Self {
            mode: SchedulingMode::default(),
            max_wait_seconds: default_max_wait_seconds(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            auth: AuthConfig::default(),
            accounts: AccountsConfig::default(),
            timeouts: TimeoutsConfig::default(),
            model_mapping: ModelMappingConfig::default(),
            logging: LoggingConfig::default(),
            scheduling: SchedulingConfig::default(),
        }
    }
}

// Default value functions
fn default_port() -> u16 { 8045 }
fn default_host() -> String { "127.0.0.1".to_string() }
fn default_request_timeout() -> u64 { 120 }
fn default_log_level() -> String { "info".to_string() }
fn default_max_wait_seconds() -> u64 { 30 }

fn default_accounts_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".antigravity_tools")
        .join("accounts")
}

/// Get default config file path
/// Uses ~/.config/antigravity-proxy/config.toml for Unix-like CLI experience
pub fn default_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("antigravity-proxy")
        .join("config.toml")
}

/// Load config from file, or return defaults if not found.
/// 
/// Loading order:
/// 1. Specified path (if provided)
/// 2. ./config.toml (if exists)
/// 3. default_config_path() (usually ~/.config/antigravity-proxy/config.toml)
pub fn load_config(path: Option<PathBuf>) -> anyhow::Result<Config> {
    if let Some(config_path) = path {
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&content)?;
            tracing::info!("Loaded config from specified path {:?}", config_path);
            return Ok(config);
        } else {
            anyhow::bail!("Specified config file not found: {:?}", config_path);
        }
    }

    // Try current directory config.toml
    let local_config = PathBuf::from("config.toml");
    if local_config.exists() {
        match std::fs::read_to_string(&local_config) {
            Ok(content) => {
                match toml::from_str::<Config>(&content) {
                    Ok(config) => {
                        tracing::info!("Loaded config from current directory {:?}", local_config);
                        return Ok(config);
                    }
                    Err(e) => {
                        tracing::error!("Failed to parse ./config.toml: {}. Falling back to default path.", e);
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to read ./config.toml: {}. Falling back to default path.", e);
            }
        }
    }

    let default_path = default_config_path();
    if default_path.exists() {
        let content = std::fs::read_to_string(&default_path)?;
        let config: Config = toml::from_str(&content)?;
        tracing::info!("Loaded config from default path {:?}", default_path);
        Ok(config)
    } else {
        tracing::info!("No config file found, using defaults");
        Ok(Config::default())
    }
}

/// Expand ~ in path to home directory
pub fn expand_path(path: &PathBuf) -> PathBuf {
    if let Some(path_str) = path.to_str() {
        if path_str.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                return home.join(&path_str[2..]);
            }
        }
    }
    path.clone()
}
