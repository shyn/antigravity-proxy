//! Token Manager for managing account pool
//! Extracted from src-tauri/src/proxy/token_manager.rs

use dashmap::DashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::proxy::rate_limit::RateLimitTracker;
use crate::proxy::sticky_config::{StickySessionConfig, SchedulingMode};

#[derive(Debug, Clone)]
pub struct ProxyToken {
    pub account_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub timestamp: i64,
    pub email: String,
    pub account_path: PathBuf,
    pub project_id: Option<String>,
    pub subscription_tier: Option<String>,
}

pub struct TokenManager {
    tokens: Arc<DashMap<String, ProxyToken>>,
    current_index: Arc<AtomicUsize>,
    last_used_account: Arc<tokio::sync::Mutex<Option<(String, std::time::Instant)>>>,
    data_dir: PathBuf,
    rate_limit_tracker: Arc<RateLimitTracker>,
    sticky_config: Arc<tokio::sync::RwLock<StickySessionConfig>>,
    session_accounts: Arc<DashMap<String, String>>,
}

impl TokenManager {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            tokens: Arc::new(DashMap::new()),
            current_index: Arc::new(AtomicUsize::new(0)),
            last_used_account: Arc::new(tokio::sync::Mutex::new(None)),
            data_dir,
            rate_limit_tracker: Arc::new(RateLimitTracker::new()),
            sticky_config: Arc::new(tokio::sync::RwLock::new(StickySessionConfig::default())),
            session_accounts: Arc::new(DashMap::new()),
        }
    }
    
    /// Load accounts from directory
    pub async fn load_accounts(&self) -> anyhow::Result<usize> {
        let accounts_dir = self.data_dir.join("accounts");
        
        if !accounts_dir.exists() {
            anyhow::bail!("Accounts directory not found: {:?}", accounts_dir);
        }
        
        self.tokens.clear();
        self.current_index.store(0, Ordering::SeqCst);
        {
            let mut last_used = self.last_used_account.lock().await;
            *last_used = None;
        }
        
        let entries = std::fs::read_dir(&accounts_dir)?;
        let mut count = 0;
        
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            
            match self.load_single_account(&path).await {
                Ok(Some(token)) => {
                    let account_id = token.account_id.clone();
                    self.tokens.insert(account_id, token);
                    count += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!("Failed to load account {:?}: {}", path, e);
                }
            }
        }
        
        Ok(count)
    }
    
    async fn load_single_account(&self, path: &PathBuf) -> anyhow::Result<Option<ProxyToken>> {
        let content = std::fs::read_to_string(path)?;
        let account: serde_json::Value = serde_json::from_str(&content)?;
        
        // Skip disabled accounts
        if account.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Ok(None);
        }
        if account.get("proxy_disabled").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Ok(None);
        }
        
        let account_id = account["id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing id field"))?
            .to_string();
        
        let email = account["email"].as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing email field"))?
            .to_string();
        
        let token_obj = account["token"].as_object()
            .ok_or_else(|| anyhow::anyhow!("Missing token field"))?;
        
        let access_token = token_obj["access_token"].as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing access_token"))?
            .to_string();
        
        let refresh_token = token_obj["refresh_token"].as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing refresh_token"))?
            .to_string();
        
        let expires_in = token_obj["expires_in"].as_i64()
            .ok_or_else(|| anyhow::anyhow!("Missing expires_in"))?;
        
        let timestamp = token_obj["expiry_timestamp"].as_i64()
            .ok_or_else(|| anyhow::anyhow!("Missing expiry_timestamp"))?;
        
        let project_id = token_obj.get("project_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        
        let subscription_tier = account.get("quota")
            .and_then(|q| q.get("subscription_tier"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        
        Ok(Some(ProxyToken {
            account_id,
            access_token,
            refresh_token,
            expires_in,
            timestamp,
            email,
            account_path: path.clone(),
            project_id,
            subscription_tier,
        }))
    }
    
    /// Get a token for use (with load balancing and sticky sessions)
    pub async fn get_token(
        &self,
        quota_group: &str,
        force_rotate: bool,
        session_id: Option<&str>,
    ) -> anyhow::Result<(String, String, String)> {
        let mut tokens_snapshot: Vec<ProxyToken> = self.tokens.iter().map(|e| e.value().clone()).collect();
        let total = tokens_snapshot.len();
        
        if total == 0 {
            anyhow::bail!("Token pool is empty");
        }
        
        // Sort by subscription tier priority
        tokens_snapshot.sort_by(|a, b| {
            let tier_priority = |tier: &Option<String>| match tier.as_deref() {
                Some("ULTRA") => 0,
                Some("PRO") => 1,
                Some("FREE") => 2,
                _ => 3,
            };
            tier_priority(&a.subscription_tier).cmp(&tier_priority(&b.subscription_tier))
        });
        
        let scheduling = self.sticky_config.read().await.clone();
        let mut attempted: HashSet<String> = HashSet::new();
        let mut last_error: Option<String> = None;
        
        for attempt in 0..total {
            let rotate = force_rotate || attempt > 0;
            let mut target_token: Option<ProxyToken> = None;
            
            // Sticky session handling
            if !rotate && session_id.is_some() && scheduling.mode != SchedulingMode::PerformanceFirst {
                let sid = session_id.unwrap();
                
                if let Some(bound_id) = self.session_accounts.get(sid).map(|v| v.clone()) {
                    let reset_sec = self.rate_limit_tracker.get_remaining_wait(&bound_id);
                    if reset_sec > 0 {
                        if scheduling.mode == SchedulingMode::CacheFirst && reset_sec <= scheduling.max_wait_seconds {
                            tokio::time::sleep(std::time::Duration::from_secs(reset_sec)).await;
                            if let Some(found) = tokens_snapshot.iter().find(|t| t.account_id == bound_id) {
                                target_token = Some(found.clone());
                            }
                        } else {
                            self.session_accounts.remove(sid);
                        }
                    } else if !attempted.contains(&bound_id) {
                        if let Some(found) = tokens_snapshot.iter().find(|t| t.account_id == bound_id) {
                            target_token = Some(found.clone());
                        }
                    }
                }
            }
            
            // Global 60s lock for non-image requests
            if target_token.is_none() && !rotate && quota_group != "image_gen" {
                let mut last_used = self.last_used_account.lock().await;
                
                if let Some((account_id, last_time)) = &*last_used {
                    if last_time.elapsed().as_secs() < 60 && !attempted.contains(account_id) {
                        if let Some(found) = tokens_snapshot.iter().find(|t| &t.account_id == account_id) {
                            target_token = Some(found.clone());
                        }
                    }
                }
                
                if target_token.is_none() {
                    let start_idx = self.current_index.fetch_add(1, Ordering::SeqCst) % total;
                    for offset in 0..total {
                        let idx = (start_idx + offset) % total;
                        let candidate = &tokens_snapshot[idx];
                        if attempted.contains(&candidate.account_id) {
                            continue;
                        }
                        if self.rate_limit_tracker.is_rate_limited(&candidate.account_id) {
                            continue;
                        }
                        target_token = Some(candidate.clone());
                        *last_used = Some((candidate.account_id.clone(), std::time::Instant::now()));
                        
                        if let Some(sid) = session_id {
                            if scheduling.mode != SchedulingMode::PerformanceFirst {
                                self.session_accounts.insert(sid.to_string(), candidate.account_id.clone());
                            }
                        }
                        break;
                    }
                }
            } else if target_token.is_none() {
                let start_idx = self.current_index.fetch_add(1, Ordering::SeqCst) % total;
                for offset in 0..total {
                    let idx = (start_idx + offset) % total;
                    let candidate = &tokens_snapshot[idx];
                    if attempted.contains(&candidate.account_id) {
                        continue;
                    }
                    if self.rate_limit_tracker.is_rate_limited(&candidate.account_id) {
                        continue;
                    }
                    target_token = Some(candidate.clone());
                    break;
                }
            }
            
            let mut token = match target_token {
                Some(t) => t,
                None => {
                    let min_wait = tokens_snapshot.iter()
                        .filter_map(|t| self.rate_limit_tracker.get_reset_seconds(&t.account_id))
                        .min()
                        .unwrap_or(60);
                    anyhow::bail!("All accounts are currently limited. Please wait {}s.", min_wait);
                }
            };
            
            // Check token expiry (refresh if < 5 minutes remaining)
            let now = chrono::Utc::now().timestamp();
            if now >= token.timestamp - 300 {
                tracing::debug!("Token for {} expiring soon, refreshing...", token.email);
                
                match crate::oauth::refresh_access_token(&token.refresh_token).await {
                    Ok(response) => {
                        token.access_token = response.access_token.clone();
                        token.expires_in = response.expires_in;
                        token.timestamp = now + response.expires_in;
                        
                        if let Some(mut entry) = self.tokens.get_mut(&token.account_id) {
                            entry.access_token = token.access_token.clone();
                            entry.expires_in = token.expires_in;
                            entry.timestamp = token.timestamp;
                        }
                        
                        // Save refreshed token to disk
                        if let Err(e) = self.save_refreshed_token(&token).await {
                            tracing::warn!("Failed to save refreshed token: {}", e);
                        }
                    }
                    Err(e) => {
                        tracing::error!("Token refresh failed for {}: {}", token.email, e);
                        last_error = Some(format!("Token refresh failed: {}", e));
                        attempted.insert(token.account_id.clone());
                        continue;
                    }
                }
            }
            
            // Ensure we have project_id
            let project_id = if let Some(pid) = &token.project_id {
                pid.clone()
            } else {
                tracing::debug!("Fetching project_id for {}...", token.email);
                match crate::proxy::project_resolver::fetch_project_id(&token.access_token).await {
                    Ok(pid) => {
                        if let Some(mut entry) = self.tokens.get_mut(&token.account_id) {
                            entry.project_id = Some(pid.clone());
                        }
                        if let Err(e) = self.save_project_id(&token.account_id, &pid).await {
                            tracing::warn!("Failed to save project_id: {}", e);
                        }
                        pid
                    }
                    Err(e) => {
                        tracing::error!("Failed to fetch project_id for {}: {}", token.email, e);
                        last_error = Some(format!("Failed to fetch project_id: {}", e));
                        attempted.insert(token.account_id.clone());
                        continue;
                    }
                }
            };
            
            return Ok((token.access_token, project_id, token.email));
        }
        
        Err(anyhow::anyhow!(last_error.unwrap_or_else(|| "All accounts failed".to_string())))
    }
    
    async fn save_refreshed_token(&self, token: &ProxyToken) -> anyhow::Result<()> {
        let mut content: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&token.account_path)?
        )?;
        
        let now = chrono::Utc::now().timestamp();
        content["token"]["access_token"] = serde_json::Value::String(token.access_token.clone());
        content["token"]["expires_in"] = serde_json::Value::Number(token.expires_in.into());
        content["token"]["expiry_timestamp"] = serde_json::Value::Number((now + token.expires_in).into());
        
        std::fs::write(&token.account_path, serde_json::to_string_pretty(&content)?)?;
        Ok(())
    }
    
    async fn save_project_id(&self, account_id: &str, project_id: &str) -> anyhow::Result<()> {
        let entry = self.tokens.get(account_id)
            .ok_or_else(|| anyhow::anyhow!("Account not found"))?;
        
        let path = &entry.account_path;
        let mut content: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        
        content["token"]["project_id"] = serde_json::Value::String(project_id.to_string());
        std::fs::write(path, serde_json::to_string_pretty(&content)?)?;
        
        Ok(())
    }
    
    pub fn len(&self) -> usize {
        self.tokens.len()
    }
    
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
    
    /// Mark account as rate limited
    pub fn mark_rate_limited(
        &self,
        account_id: &str,
        status: u16,
        retry_after_header: Option<&str>,
        error_body: &str,
    ) {
        self.rate_limit_tracker.parse_from_error(account_id, status, retry_after_header, error_body);
    }
    
    pub fn is_rate_limited(&self, account_id: &str) -> bool {
        self.rate_limit_tracker.is_rate_limited(account_id)
    }
    
    pub async fn get_sticky_config(&self) -> StickySessionConfig {
        self.sticky_config.read().await.clone()
    }
    
    pub async fn update_sticky_config(&self, new_config: StickySessionConfig) {
        let mut config = self.sticky_config.write().await;
        *config = new_config;
    }
    
    pub fn clear_all_sessions(&self) {
        self.session_accounts.clear();
    }
}
