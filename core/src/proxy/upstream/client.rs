//! Upstream client for calling Google Cloud Code API

use reqwest::{header, Client, Response};
use serde_json::Value;
use tokio::time::Duration;

// Antigravity API endpoints (matching TypeScript plugin's ANTIGRAVITY_ENDPOINTS)
// Order matters: daily sandbox first, then autopush, then production
const BASE_URL_FALLBACKS: [&str; 3] = [
    "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal",
    "https://autopush-cloudcode-pa.sandbox.googleapis.com/v1internal",
    "https://cloudcode-pa.googleapis.com/v1internal",
];

#[derive(Clone)]
pub struct UpstreamClient {
    http_client: Client,
}

impl UpstreamClient {
    pub fn new(proxy_url: Option<String>) -> Self {
        let mut builder = Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(16)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(60))
            .timeout(Duration::from_secs(600))
            .user_agent("antigravity/1.15.8 darwin/amd64");
        
        if let Some(proxy) = proxy_url {
            if !proxy.is_empty() {
                if let Ok(p) = reqwest::Proxy::all(&proxy) {
                    builder = builder.proxy(p);
                    tracing::info!("Using upstream proxy: {}", proxy);
                }
            }
        }
        
        let http_client = builder.build().expect("Failed to create HTTP client");
        Self { http_client }
    }
    
    fn build_url(base_url: &str, method: &str, query_string: Option<&str>) -> String {
        // base_url already ends with /v1internal, so we just append :method
        if let Some(qs) = query_string {
            format!("{}:{}?{}", base_url, method, qs)
        } else {
            format!("{}:{}", base_url, method)
        }
    }
    
    fn should_try_next_endpoint(status: reqwest::StatusCode) -> bool {
        // [FIX] DO NOT retry 429 on different endpoint - 429 is account-level rate limiting,
        // not endpoint-level. Retrying with the same account on a different endpoint
        // will just get another 429 and waste time/quota.
        // Instead, return immediately so the caller can switch to a different account.
        status == reqwest::StatusCode::REQUEST_TIMEOUT
            || status == reqwest::StatusCode::NOT_FOUND
            || (status.is_server_error() && status != reqwest::StatusCode::SERVICE_UNAVAILABLE)
    }
    
    /// Call v1internal API with automatic fallback
    /// 
    /// # Arguments
    /// * `method` - API method (e.g., "generateContent", "streamGenerateContent")
    /// * `access_token` - OAuth access token
    /// * `body` - Request body as JSON
    /// * `query_string` - Optional query string (e.g., "alt=sse" for streaming)
    /// * `is_thinking_model` - Set to true for Claude thinking models to add anthropic-beta header
    pub async fn call_v1_internal(
        &self,
        method: &str,
        access_token: &str,
        body: Value,
        query_string: Option<&str>,
        is_thinking_model: bool,
    ) -> Result<Response, String> {
        let mut headers = header::HeaderMap::new();
        
        // [FIX] Explicitly set all headers to match TypeScript ANTIGRAVITY_HEADERS exactly
        headers.insert(
            header::USER_AGENT,
            header::HeaderValue::from_static("antigravity/1.15.8 darwin/amd64"),
        );
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {}", access_token))
                .map_err(|e| e.to_string())?,
        );
        // Antigravity-specific headers for Google Cloud Code API (matches TypeScript ANTIGRAVITY_HEADERS)
        headers.insert(
            header::HeaderName::from_static("x-goog-api-client"),
            header::HeaderValue::from_static("google-cloud-sdk vscode_cloudshelleditor/0.1"),
        );
        headers.insert(
            header::HeaderName::from_static("client-metadata"),
            header::HeaderValue::from_static(r#"{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}"#),
        );
        
        // [FIX] Add Accept header for streaming requests (matches TypeScript implementation)
        // This is critical for SSE streaming to work properly
        if query_string.map_or(false, |qs| qs.contains("alt=sse")) {
            headers.insert(
                header::ACCEPT,
                header::HeaderValue::from_static("text/event-stream"),
            );
        }
        
        // [FIX] Add anthropic-beta header for Claude thinking models with interleaved thinking
        // This enables thinking blocks between tool calls and results
        if is_thinking_model {
            headers.insert(
                header::HeaderName::from_static("anthropic-beta"),
                header::HeaderValue::from_static("interleaved-thinking-2025-05-14"),
            );
        }
        
        let mut last_err: Option<String> = None;
        
        for (idx, base_url) in BASE_URL_FALLBACKS.iter().enumerate() {
            let url = Self::build_url(base_url, method, query_string);
            let has_next = idx + 1 < BASE_URL_FALLBACKS.len();
            
            let response = self.http_client
                .post(&url)
                .headers(headers.clone())
                .json(&body)
                .send()
                .await;
            
            match response {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        if idx > 0 {
                            tracing::info!("Upstream fallback succeeded: {} (attempt {})", base_url, idx + 1);
                        }
                        return Ok(resp);
                    }
                    
                    // Error response - log full details
                    let headers = resp.headers().clone();
                    let body_bytes = resp.bytes().await.unwrap_or_default();
                    let body_text = String::from_utf8_lossy(&body_bytes);
                    
                    if has_next && Self::should_try_next_endpoint(status) {
                        tracing::warn!(
                            "Upstream {} returned error {}, trying next.\nHeaders: {:?}\nBody: {}", 
                            base_url, status, headers, body_text
                        );
                        last_err = Some(format!("Upstream error {}: {}", status, body_text));
                        continue;
                    }
                    
                    tracing::error!(
                        "Upstream {} failed with status {}.\nHeaders: {:?}\nBody: {}",
                        base_url, status, headers, body_text
                    );
                    return Err(format!("Upstream error {}: {}", status, body_text));
                }
                Err(e) => {
                    let msg = format!("Request failed at {}: {}", base_url, e);
                    tracing::error!("{}", msg);
                    last_err = Some(msg);
                    
                    if !has_next {
                        break;
                    }
                    continue;
                }
            }
        }
        
        Err(last_err.unwrap_or_else(|| "All endpoints failed".to_string()))
    }
}
