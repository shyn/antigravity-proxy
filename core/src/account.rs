//! Account management module
//! Extracted from src-tauri/src/modules/account.rs

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const DATA_DIR: &str = ".antigravity_tools";
const ACCOUNTS_DIR: &str = "accounts";

/// Token data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenData {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub expiry_timestamp: i64,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
}

impl TokenData {
    pub fn new(
        access_token: String,
        refresh_token: String,
        expires_in: i64,
        email: Option<String>,
        project_id: Option<String>,
    ) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            access_token,
            refresh_token,
            expires_in,
            expiry_timestamp: now + expires_in,
            email,
            project_id,
        }
    }
}

/// Quota data structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuotaData {
    #[serde(default)]
    pub subscription_tier: Option<String>,
    #[serde(default)]
    pub gemini_quota: Option<QuotaInfo>,
    #[serde(default)]
    pub claude_quota: Option<QuotaInfo>,
    #[serde(default)]
    pub last_updated: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaInfo {
    #[serde(default)]
    pub used: i64,
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub reset_time: Option<String>,
}

/// Account data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    pub token: TokenData,
    pub quota: Option<QuotaData>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub disabled_reason: Option<String>,
    #[serde(default)]
    pub disabled_at: Option<i64>,
    #[serde(default)]
    pub proxy_disabled: bool,
    #[serde(default)]
    pub proxy_disabled_reason: Option<String>,
    #[serde(default)]
    pub proxy_disabled_at: Option<i64>,
    pub created_at: i64,
    pub last_used: i64,
}

/// Get data directory path
pub fn get_data_dir() -> anyhow::Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let data_dir = home.join(DATA_DIR);
    
    if !data_dir.exists() {
        fs::create_dir_all(&data_dir)?;
    }
    
    Ok(data_dir)
}

/// Get accounts directory path
pub fn get_accounts_dir() -> anyhow::Result<PathBuf> {
    let data_dir = get_data_dir()?;
    let accounts_dir = data_dir.join(ACCOUNTS_DIR);
    
    if !accounts_dir.exists() {
        fs::create_dir_all(&accounts_dir)?;
    }
    
    Ok(accounts_dir)
}

/// List all accounts
pub fn list_accounts() -> anyhow::Result<Vec<Account>> {
    let accounts_dir = get_accounts_dir()?;
    let mut accounts = Vec::new();
    
    if !accounts_dir.exists() {
        return Ok(accounts);
    }
    
    for entry in fs::read_dir(&accounts_dir)? {
        let entry = entry?;
        let path = entry.path();
        
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        
        match load_account_from_path(&path) {
            Ok(account) => accounts.push(account),
            Err(e) => {
                tracing::debug!("Failed to load account {:?}: {}", path, e);
            }
        }
    }
    
    // Sort by last_used descending
    accounts.sort_by(|a, b| b.last_used.cmp(&a.last_used));
    
    Ok(accounts)
}

/// Load account from file path
fn load_account_from_path(path: &PathBuf) -> anyhow::Result<Account> {
    let content = fs::read_to_string(path)?;
    let account: Account = serde_json::from_str(&content)?;
    Ok(account)
}

/// Load account by ID
pub fn load_account(account_id: &str) -> anyhow::Result<Account> {
    let accounts_dir = get_accounts_dir()?;
    let path = accounts_dir.join(format!("{}.json", account_id));
    load_account_from_path(&path)
}

/// Save account to file
pub fn save_account(account: &Account) -> anyhow::Result<()> {
    let accounts_dir = get_accounts_dir()?;
    let path = accounts_dir.join(format!("{}.json", account.id));
    let content = serde_json::to_string_pretty(account)?;
    fs::write(path, content)?;
    Ok(())
}
