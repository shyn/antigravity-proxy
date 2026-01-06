//! Proxy configuration
//! Extracted from src-tauri/src/proxy/config.rs (z.ai removed)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProxyAuthMode {
    Off,
    Strict,
    AllExceptHealth,
}

impl Default for ProxyAuthMode {
    fn default() -> Self {
        Self::Off
    }
}

/// Proxy server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub enabled: bool,
    #[serde(default)]
    pub allow_lan_access: bool,
    #[serde(default)]
    pub auth_mode: ProxyAuthMode,
    pub port: u16,
    pub api_key: String,
    #[serde(default)]
    pub anthropic_mapping: HashMap<String, String>,
    #[serde(default)]
    pub openai_mapping: HashMap<String, String>,
    #[serde(default)]
    pub custom_mapping: HashMap<String, String>,
    #[serde(default = "default_request_timeout")]
    pub request_timeout: u64,
    #[serde(default)]
    pub enable_logging: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_lan_access: false,
            auth_mode: ProxyAuthMode::default(),
            port: 8045,
            api_key: format!("sk-{}", uuid::Uuid::new_v4().simple()),
            anthropic_mapping: HashMap::new(),
            openai_mapping: HashMap::new(),
            custom_mapping: HashMap::new(),
            request_timeout: default_request_timeout(),
            enable_logging: true,
        }
    }
}

fn default_request_timeout() -> u64 {
    120
}

impl ProxyConfig {
    pub fn get_bind_address(&self) -> &str {
        if self.allow_lan_access {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        }
    }
}
