//! OAuth module for Google authentication
//! Implements Google OAuth2 PKCE flow for Antigravity authentication

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::oneshot;
use sha2::{Sha256, Digest};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use crate::account::{TokenData, Account, save_account};

// Google OAuth configuration
const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "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";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v1/userinfo?alt=json";

// OAuth callback configuration
const REDIRECT_URI: &str = "http://localhost:51121/oauth-callback";
const REDIRECT_PORT: u16 = 51121;

// Default project ID when Antigravity doesn't return one
const DEFAULT_PROJECT_ID: &str = "rising-fact-p41fc";

// OAuth scopes
const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/cclog",
    "https://www.googleapis.com/auth/experimentsandconfigs",
];

// Antigravity endpoints for project ID lookup
const LOAD_CODE_ASSIST_ENDPOINTS: &[&str] = &[
    "https://cloudcode-pa.googleapis.com",
    "https://daily-cloudcode-pa.sandbox.googleapis.com",
    "https://autopush-cloudcode-pa.sandbox.googleapis.com",
];

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub expires_in: i64,
    #[serde(default)]
    pub token_type: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserInfo {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
}

#[derive(Debug)]
pub struct AuthResult {
    pub account: Account,
}

// ============================================================================
// PKCE Implementation
// ============================================================================

/// PKCE challenge and verifier
#[derive(Debug, Clone)]
pub struct PKCEChallenge {
    pub verifier: String,
    pub challenge: String,
}

/// Generate a random string for PKCE verifier (64 characters)
fn generate_random_string(length: usize) -> String {
    use rand::Rng;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::rng();
    (0..length)
        .map(|_| {
            let idx = rng.random_range(0..CHARS.len());
            CHARS[idx] as char
        })
        .collect()
}

/// Generate PKCE challenge (SHA-256 hash, base64url encoded)
pub fn generate_pkce() -> PKCEChallenge {
    let verifier = generate_random_string(64);
    
    // SHA-256 hash
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    
    // Base64url encode (no padding)
    let challenge = URL_SAFE_NO_PAD.encode(&hash);
    
    PKCEChallenge { verifier, challenge }
}

// ============================================================================
// State Encoding (verifier + projectId in state parameter)
// ============================================================================

#[derive(Debug, Serialize, Deserialize)]
struct AuthState {
    verifier: String,
    project_id: String,
}

/// Encode state object into base64url string
fn encode_state(verifier: &str, project_id: &str) -> String {
    let state = AuthState {
        verifier: verifier.to_string(),
        project_id: project_id.to_string(),
    };
    let json = serde_json::to_string(&state).unwrap();
    URL_SAFE_NO_PAD.encode(json.as_bytes())
}

/// Decode state parameter back to object
fn decode_state(state: &str) -> anyhow::Result<AuthState> {
    let bytes = URL_SAFE_NO_PAD.decode(state)?;
    let json = String::from_utf8(bytes)?;
    let parsed: AuthState = serde_json::from_str(&json)?;
    if parsed.verifier.is_empty() {
        anyhow::bail!("Missing PKCE verifier in state");
    }
    Ok(parsed)
}

// ============================================================================
// HTTP Client
// ============================================================================

/// Create HTTP client with timeout
fn create_client(timeout_secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .expect("Failed to create HTTP client")
}

// ============================================================================
// OAuth Flow
// ============================================================================

/// Build the Google OAuth authorization URL with PKCE challenge
pub fn build_authorization_url(pkce: &PKCEChallenge, project_id: &str) -> String {
    let scopes_str = SCOPES.join(" ");
    let state = encode_state(&pkce.verifier, project_id);
    
    format!(
        "{}?client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        AUTH_URL,
        CLIENT_ID,
        urlencoding::encode(REDIRECT_URI),
        urlencoding::encode(&scopes_str),
        urlencoding::encode(&pkce.challenge),
        urlencoding::encode(&state)
    )
}

/// Exchange authorization code for tokens (with PKCE verifier)
pub async fn exchange_code(code: &str, state: &str) -> anyhow::Result<(TokenResponse, String)> {
    let auth_state = decode_state(state)?;
    let client = create_client(15);
    
    let params = [
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("code", code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", REDIRECT_URI),
        ("code_verifier", &auth_state.verifier),
    ];
    
    tracing::debug!("Exchanging authorization code for tokens with PKCE verifier...");
    
    let response = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .await?;
    
    if response.status().is_success() {
        let token_data = response.json::<TokenResponse>().await?;
        tracing::debug!("Token exchange successful, expires_in={}s", token_data.expires_in);
        Ok((token_data, auth_state.project_id))
    } else {
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Token exchange failed: {}", error_text)
    }
}

/// Get user info from Google API
pub async fn get_user_info(access_token: &str) -> anyhow::Result<UserInfo> {
    let client = create_client(10);
    
    let response = client
        .get(USERINFO_URL)
        .bearer_auth(access_token)
        .send()
        .await?;
    
    if response.status().is_success() {
        let user_info = response.json::<UserInfo>().await?;
        if let Some(ref email) = user_info.email {
            tracing::debug!("Got user info for: {}", email);
        }
        Ok(user_info)
    } else {
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to get user info: {}", error_text)
    }
}

// ============================================================================
// Project ID Fetching (from Antigravity API)
// ============================================================================

#[derive(Debug, Serialize, Deserialize)]
struct LoadCodeAssistRequest {
    metadata: LoadCodeAssistMetadata,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadCodeAssistMetadata {
    ide_type: String,
    platform: String,
    plugin_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadCodeAssistResponse {
    #[serde(default)]
    cloudaicompanion_project: Option<serde_json::Value>,
}

/// Fetch Antigravity project ID from loadCodeAssist endpoint
pub async fn fetch_project_id(access_token: &str) -> String {
    let client = create_client(10);
    
    let request_body = LoadCodeAssistRequest {
        metadata: LoadCodeAssistMetadata {
            ide_type: "IDE_UNSPECIFIED".to_string(),
            platform: "PLATFORM_UNSPECIFIED".to_string(),
            plugin_type: "GEMINI".to_string(),
        },
    };
    
    for base_endpoint in LOAD_CODE_ASSIST_ENDPOINTS {
        let url = format!("{}/v1internal:loadCodeAssist", base_endpoint);
        
        let result = client
            .post(&url)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .header("User-Agent", "google-api-nodejs-client/9.15.1")
            .header("X-Goog-Api-Client", "google-cloud-sdk vscode_cloudshelleditor/0.1")
            .header("Client-Metadata", r#"{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}"#)
            .json(&request_body)
            .send()
            .await;
        
        match result {
            Ok(response) if response.status().is_success() => {
                if let Ok(data) = response.json::<LoadCodeAssistResponse>().await {
                    if let Some(project) = data.cloudaicompanion_project {
                        // Handle both string and object formats
                        if let Some(id) = project.as_str() {
                            if !id.is_empty() {
                                tracing::debug!("Got project ID: {}", id);
                                return id.to_string();
                            }
                        } else if let Some(obj) = project.as_object() {
                            if let Some(id) = obj.get("id").and_then(|v| v.as_str()) {
                                if !id.is_empty() {
                                    tracing::debug!("Got project ID: {}", id);
                                    return id.to_string();
                                }
                            }
                        }
                    }
                }
            }
            _ => continue,
        }
    }
    
    tracing::debug!("Could not fetch project ID, using default: {}", DEFAULT_PROJECT_ID);
    DEFAULT_PROJECT_ID.to_string()
}

// ============================================================================
// Main Auth Flow
// ============================================================================

/// Start the OAuth authorization flow
/// Opens browser and waits for callback
pub async fn start_auth_flow() -> anyhow::Result<AuthResult> {
    use axum::{routing::get, Router, extract::Query};
    use std::collections::HashMap;
    
    // Generate PKCE challenge
    let pkce = generate_pkce();
    let pkce_verifier = pkce.verifier.clone();
    
    // Channel to receive the authorization result (code + state)
    let (tx, rx) = oneshot::channel::<Result<(String, String), String>>();
    let tx = Arc::new(tokio::sync::Mutex::new(Some(tx)));
    
    // Build callback handler
    let tx_clone = tx.clone();
    let callback_handler = move |Query(params): Query<HashMap<String, String>>| {
        let tx = tx_clone.clone();
        async move {
            let mut tx_guard = tx.lock().await;
            if let Some(tx) = tx_guard.take() {
                // Check for error
                if let Some(error) = params.get("error") {
                    let _ = tx.send(Err(error.clone()));
                    return axum::response::Html(format!(
                        "<html><body><h1>Authorization Failed</h1><p>Error: {}</p><p>You can close this window.</p></body></html>",
                        error
                    ));
                }
                
                // Get authorization code and state
                let code = params.get("code").cloned();
                let state = params.get("state").cloned();
                
                match (code, state) {
                    (Some(code), Some(state)) => {
                        let _ = tx.send(Ok((code, state)));
                        axum::response::Html(
                            "<html><body><h1>Authorization Successful!</h1><p>You can close this window and return to the terminal.</p></body></html>".to_string()
                        )
                    }
                    _ => {
                        let _ = tx.send(Err("Missing code or state parameter".to_string()));
                        axum::response::Html(
                            "<html><body><h1>Authorization Failed</h1><p>Missing parameters.</p><p>You can close this window.</p></body></html>".to_string()
                        )
                    }
                }
            } else {
                axum::response::Html(
                    "<html><body><h1>Already Processed</h1><p>You can close this window.</p></body></html>".to_string()
                )
            }
        }
    };
    
    let app = Router::new()
        .route("/oauth-callback", get(callback_handler));
    
    // Start server
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", REDIRECT_PORT)).await?;
    tracing::info!("OAuth callback server listening on port {}", REDIRECT_PORT);
    
    // Build authorization URL with PKCE and open browser
    let auth_url = build_authorization_url(&pkce, "");
    tracing::info!("Opening browser for authorization...");
    
    if let Err(e) = open::that(&auth_url) {
        tracing::warn!("Failed to open browser automatically: {}", e);
        println!("\nPlease open this URL in your browser:\n{}\n", auth_url);
    }
    
    println!("Waiting for authorization...");
    
    // Spawn server
    let server = axum::serve(listener, app);
    
    // Wait for callback or timeout (5 minutes)
    let (code, state) = tokio::select! {
        result = rx => {
            match result {
                Ok(Ok((code, state))) => (code, state),
                Ok(Err(e)) => anyhow::bail!("Authorization failed: {}", e),
                Err(_) => anyhow::bail!("Authorization cancelled"),
            }
        }
        _ = async {
            let _ = server.await;
        } => {
            anyhow::bail!("Server stopped unexpectedly")
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(300)) => {
            anyhow::bail!("Authorization timed out after 5 minutes")
        }
    };
    
    tracing::info!("Received authorization code, exchanging for tokens...");
    
    // Verify state contains our verifier
    let auth_state = decode_state(&state)?;
    if auth_state.verifier != pkce_verifier {
        anyhow::bail!("PKCE verifier mismatch - possible CSRF attack");
    }
    
    // Exchange code for tokens
    let (token_response, state_project_id) = exchange_code(&code, &state).await?;
    
    let refresh_token = token_response.refresh_token
        .ok_or_else(|| anyhow::anyhow!("No refresh token received. This may happen if you've already authorized this app before."))?;
    
    // Get user info
    let user_info = get_user_info(&token_response.access_token).await?;
    let email = user_info.email.unwrap_or_else(|| "unknown@gmail.com".to_string());
    let user_id = user_info.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    
    // Get project ID (from state or fetch from API)
    let project_id = if !state_project_id.is_empty() {
        state_project_id
    } else {
        fetch_project_id(&token_response.access_token).await
    };
    
    tracing::info!("Project ID: {}", project_id);
    
    // Create account
    let now = chrono::Utc::now().timestamp();
    let account = Account {
        id: user_id,
        email: email.clone(),
        name: user_info.name,
        token: TokenData::new(
            token_response.access_token,
            refresh_token,
            token_response.expires_in,
            Some(email),
            Some(project_id),
        ),
        quota: None,
        disabled: false,
        disabled_reason: None,
        disabled_at: None,
        proxy_disabled: false,
        proxy_disabled_reason: None,
        proxy_disabled_at: None,
        created_at: now,
        last_used: now,
    };
    
    // Save account
    save_account(&account)?;
    tracing::info!("Account saved successfully: {}", account.email);
    
    Ok(AuthResult { account })
}

// ============================================================================
// Token Refresh
// ============================================================================

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
