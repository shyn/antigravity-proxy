//! Proxy Server - Axum HTTP server
//! Simplified from src-tauri/src/proxy/server.rs (z.ai removed)

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use axum::{
    extract::DefaultBodyLimit,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{any, get, post},
    Router,
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::proxy::TokenManager;
use crate::config::AuthMode;

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub token_manager: Arc<TokenManager>,
    pub upstream: Arc<crate::proxy::upstream::client::UpstreamClient>,
    pub anthropic_mapping: Arc<RwLock<HashMap<String, String>>>,
    pub openai_mapping: Arc<RwLock<HashMap<String, String>>>,
    pub custom_mapping: Arc<RwLock<HashMap<String, String>>>,
    pub request_timeout: u64,
    pub security_config: Arc<RwLock<SecurityConfig>>,
}

#[derive(Clone)]
pub struct SecurityConfig {
    pub auth_mode: AuthMode,
    pub api_key: String,
}

/// Proxy server instance
pub struct ProxyServer {
    host: String,
    port: u16,
    state: AppState,
}

impl ProxyServer {
    pub fn new(
        host: String,
        port: u16,
        token_manager: Arc<TokenManager>,
        anthropic_mapping: HashMap<String, String>,
        openai_mapping: HashMap<String, String>,
        custom_mapping: HashMap<String, String>,
        request_timeout: u64,
        auth_mode: AuthMode,
        api_key: String,
    ) -> Self {
        let upstream = Arc::new(crate::proxy::upstream::client::UpstreamClient::new(None));
        
        let state = AppState {
            token_manager,
            upstream,
            anthropic_mapping: Arc::new(RwLock::new(anthropic_mapping)),
            openai_mapping: Arc::new(RwLock::new(openai_mapping)),
            custom_mapping: Arc::new(RwLock::new(custom_mapping)),
            request_timeout,
            security_config: Arc::new(RwLock::new(SecurityConfig {
                auth_mode,
                api_key,
            })),
        };
        
        Self { host, port, state }
    }
    
    /// Run the proxy server (blocking)
    pub async fn run(self) -> anyhow::Result<()> {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);
        
        let app = Router::new()
            // Health check
            .route("/healthz", get(health_check_handler))
            .route("/health", get(health_check_handler))
            
            // OpenAI-compatible endpoints
            .route("/v1/chat/completions", post(crate::proxy::handlers::openai::handle_chat_completions))
            .route("/v1/completions", post(crate::proxy::handlers::openai::handle_completions))
            .route("/v1/models", get(crate::proxy::handlers::openai::handle_list_models))
            .route("/v1/images/generations", post(crate::proxy::handlers::openai::handle_images_generations))
            
            // Claude/Anthropic-compatible endpoints
            .route("/v1/messages", post(crate::proxy::handlers::claude::handle_messages))
            
            // Gemini endpoints
            .route("/v1beta/models/:model_action", any(crate::proxy::handlers::gemini::handle_gemini_request))
            
            .layer(DefaultBodyLimit::max(100 * 1024 * 1024)) // 100MB
            .layer(cors)
            .layer(TraceLayer::new_for_http())
            .with_state(self.state);
        
        let addr = format!("{}:{}", self.host, self.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        
        tracing::info!("Proxy server listening on {}", addr);
        
        // Handle graceful shutdown
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;
        
        tracing::info!("Proxy server stopped");
        Ok(())
    }
}

/// Health check handler
async fn health_check_handler() -> Response {
    (StatusCode::OK, Json(serde_json::json!({"status": "ok"}))).into_response()
}

/// Shutdown signal handler
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Shutdown signal received");
}
