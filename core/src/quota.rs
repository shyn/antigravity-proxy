//! Quota query module
//! Extracted from src-tauri/src/modules/quota.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::account::QuotaData;

const QUOTA_API_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels";
const CLOUD_CODE_BASE_URL: &str = "https://cloudcode-pa.googleapis.com";
const USER_AGENT: &str = "antigravity/1.11.9 Darwin/arm64";

#[derive(Debug, Serialize, Deserialize)]
pub struct QuotaResponse {
    pub models: HashMap<String, ModelInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ModelInfo {
    #[serde(rename = "quotaInfo")]
    pub quota_info: Option<QuotaInfoRaw>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QuotaInfoRaw {
    #[serde(rename = "remainingFraction")]
    pub remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    pub reset_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoadProjectResponse {
    #[serde(rename = "cloudaicompanionProject")]
    project_id: Option<String>,
    #[serde(rename = "currentTier")]
    current_tier: Option<Tier>,
    #[serde(rename = "paidTier")]
    paid_tier: Option<Tier>,
}

#[derive(Debug, Deserialize)]
struct Tier {
    id: Option<String>,
    #[serde(rename = "quotaTier")]
    #[allow(dead_code)]
    quota_tier: Option<String>,
    #[allow(dead_code)]
    name: Option<String>,
}

/// Model quota detail for display
#[derive(Debug, Clone)]
pub struct ModelQuotaDetail {
    pub model_name: String,
    pub remaining_pct: i32,
    pub used_pct: i32,
    pub reset_time: Option<String>,
}

/// Create HTTP client
fn create_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(USER_AGENT)
        .build()
        .expect("Failed to create HTTP client")
}

/// Fetch project ID and subscription tier
pub async fn fetch_project_id(access_token: &str, _email: &str) -> (Option<String>, Option<String>) {
    let client = create_client();
    let url = format!("{}/v1internal:loadCodeAssist", CLOUD_CODE_BASE_URL);
    
    let meta = serde_json::json!({"metadata": {"ideType": "ANTIGRAVITY"}});
    
    let response = client
        .post(&url)
        .bearer_auth(access_token)
        .header("Content-Type", "application/json")
        .json(&meta)
        .send()
        .await;
    
    match response {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<LoadProjectResponse>().await {
                Ok(data) => {
                    let project_id = data.project_id.clone();
                    let subscription_tier = data.paid_tier
                        .and_then(|t| t.id)
                        .or_else(|| data.current_tier.and_then(|t| t.id));
                    
                    if let Some(ref tier) = subscription_tier {
                        tracing::info!("Subscription tier: {}", tier);
                    }
                    
                    (project_id, subscription_tier)
                }
                Err(e) => {
                    tracing::debug!("Failed to parse loadCodeAssist response: {}", e);
                    (None, None)
                }
            }
        }
        Ok(resp) => {
            tracing::debug!("loadCodeAssist returned {}", resp.status());
            (None, None)
        }
        Err(e) => {
            tracing::debug!("loadCodeAssist request failed: {}", e);
            (None, None)
        }
    }
}

/// Fetch quota information with detailed per-model data
pub async fn fetch_quota_detailed(access_token: &str, email: &str) -> anyhow::Result<(Option<String>, Vec<ModelQuotaDetail>)> {
    let client = create_client();
    
    // First get project ID and tier
    let (project_id, tier) = fetch_project_id(access_token, email).await;
    
    let final_project_id = project_id.as_deref().unwrap_or("bamboo-precept-lgxtn");
    
    let payload = serde_json::json!({
        "project": final_project_id
    });
    
    let response = client
        .post(QUOTA_API_URL)
        .bearer_auth(access_token)
        .header("Content-Type", "application/json")
        .header("User-Agent", USER_AGENT)
        .json(&payload)
        .send()
        .await?;
    
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("Quota API returned {}: {}", status, text);
    }
    
    let quota_resp: QuotaResponse = response.json().await?;
    
    // Collect all models with quota info
    let mut models: Vec<ModelQuotaDetail> = Vec::new();
    
    for (name, info) in &quota_resp.models {
        let name_lower = name.to_lowercase();
        // Only include claude and gemini models
        if !name_lower.contains("claude") && !name_lower.contains("gemini") {
            continue;
        }

        if let Some(quota_info) = &info.quota_info {
            // Default to 0 if remaining_fraction is missing (usually means used up or resetting)
            let remaining_pct = quota_info.remaining_fraction
                .map(|f| (f * 100.0) as i32)
                .unwrap_or(0);
            
            models.push(ModelQuotaDetail {
                model_name: name.clone(),
                remaining_pct,
                used_pct: 100 - remaining_pct,
                reset_time: quota_info.reset_time.clone(),
            });
        }
    }
    
    // Sort by model name
    models.sort_by(|a, b| a.model_name.cmp(&b.model_name));
    
    Ok((tier, models))
}

/// Fetch quota information (simplified, for backward compatibility)
pub async fn fetch_quota(access_token: &str, email: &str) -> anyhow::Result<(QuotaData, Option<String>)> {
    let client = create_client();
    
    let (project_id, tier) = fetch_project_id(access_token, email).await;
    
    let final_project_id = project_id.as_deref().unwrap_or("bamboo-precept-lgxtn");
    
    let payload = serde_json::json!({
        "project": final_project_id
    });
    
    let response = client
        .post(QUOTA_API_URL)
        .bearer_auth(access_token)
        .header("Content-Type", "application/json")
        .header("User-Agent", USER_AGENT)
        .json(&payload)
        .send()
        .await?;
    
    if !response.status().is_success() {
        let status = response.status();
        if status == reqwest::StatusCode::FORBIDDEN {
            tracing::warn!("Account forbidden (403)");
            let mut quota_data = QuotaData::default();
            quota_data.subscription_tier = tier;
            return Ok((quota_data, project_id));
        }
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("Quota API returned {}: {}", status, text);
    }
    
    let quota_resp: QuotaResponse = response.json().await?;
    
    let mut gemini_used = 0i64;
    let mut gemini_total = 100i64;
    let mut claude_used = 0i64;
    let mut claude_total = 100i64;
    
    for (name, info) in &quota_resp.models {
        if let Some(quota_info) = &info.quota_info {
            let remaining_pct = quota_info.remaining_fraction
                .map(|f| (f * 100.0) as i64)
                .unwrap_or(100);
            let used_pct = 100 - remaining_pct;
            
            let name_lower = name.to_lowercase();
            if name_lower.contains("gemini") {
                gemini_used = gemini_used.max(used_pct);
            } else if name_lower.contains("claude") {
                claude_used = claude_used.max(used_pct);
            }
        }
    }
    
    let quota_data = QuotaData {
        subscription_tier: tier,
        gemini_quota: Some(crate::account::QuotaInfo {
            used: gemini_used,
            total: gemini_total,
            reset_time: None,
        }),
        claude_quota: Some(crate::account::QuotaInfo {
            used: claude_used,
            total: claude_total,
            reset_time: None,
        }),
        last_updated: Some(chrono::Utc::now().timestamp()),
    };
    
    Ok((quota_data, project_id))
}
