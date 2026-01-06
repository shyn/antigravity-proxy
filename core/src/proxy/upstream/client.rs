//! Upstream client for calling Google Cloud Code API

use reqwest::{header, Client, Response};
use serde_json::Value;
use tokio::time::Duration;

const V1_INTERNAL_BASE_URL_PROD: &str = "https://cloudcode-pa.googleapis.com/v1internal";
const V1_INTERNAL_BASE_URL_DAILY: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal";

const BASE_URL_FALLBACKS: [&str; 2] = [
    V1_INTERNAL_BASE_URL_PROD,
    V1_INTERNAL_BASE_URL_DAILY,
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
            .user_agent("antigravity/1.11.9 cli");
        
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
        if let Some(qs) = query_string {
            format!("{}:{}?{}", base_url, method, qs)
        } else {
            format!("{}:{}", base_url, method)
        }
    }
    
    fn should_try_next_endpoint(status: reqwest::StatusCode) -> bool {
        status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status == reqwest::StatusCode::REQUEST_TIMEOUT
            || status == reqwest::StatusCode::NOT_FOUND
            || status.is_server_error()
    }
    
    /// Call v1internal API with automatic fallback
    pub async fn call_v1_internal(
        &self,
        method: &str,
        access_token: &str,
        body: Value,
        query_string: Option<&str>,
    ) -> Result<Response, String> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {}", access_token))
                .map_err(|e| e.to_string())?,
        );
        
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
                    
                    if has_next && Self::should_try_next_endpoint(status) {
                        tracing::warn!("Upstream {} returned {}, trying next", base_url, status);
                        last_err = Some(format!("Upstream {} returned {}", base_url, status));
                        continue;
                    }
                    
                    return Ok(resp);
                }
                Err(e) => {
                    let msg = format!("Request failed at {}: {}", base_url, e);
                    tracing::debug!("{}", msg);
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
