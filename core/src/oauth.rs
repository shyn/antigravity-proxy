//! OAuth module for Google authentication
//! Extracted from src-tauri/src/modules/oauth.rs

use serde::{Deserialize, Serialize};
use crate::account::TokenData;

// Google OAuth configuration
const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "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";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub expires_in: i64,
    #[serde(default)]
    pub token_type: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

/// Create HTTP client with timeout
fn create_client(timeout_secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .expect("Failed to create HTTP client")
}

/// Refresh access token using refresh_token
pub async fn refresh_access_token(refresh_token: &str) -> anyhow::Result<TokenResponse> {
    let client = create_client(15);
    
    let params = [
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    
    tracing::debug!("Refreshing token...");
    
    let response = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .await?;
    
    if response.status().is_success() {
        let token_data = response.json::<TokenResponse>().await?;
        tracing::debug!("Token refresh successful, expires_in={}s", token_data.expires_in);
        Ok(token_data)
    } else {
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Token refresh failed: {}", error_text)
    }
}

/// Check and refresh token if needed
/// Returns updated TokenData if refreshed
pub async fn ensure_fresh_token(current_token: &TokenData) -> anyhow::Result<TokenData> {
    let now = chrono::Utc::now().timestamp();
    
    // If token has more than 5 minutes validity, use it as-is
    if current_token.expiry_timestamp > now + 300 {
        return Ok(current_token.clone());
    }
    
    // Need to refresh
    tracing::info!("Token expiring soon, refreshing...");
    let response = refresh_access_token(&current_token.refresh_token).await?;
    
    Ok(TokenData::new(
        response.access_token,
        current_token.refresh_token.clone(),
        response.expires_in,
        current_token.email.clone(),
        current_token.project_id.clone(),
    ))
}
