use std::path::PathBuf;
use std::sync::Arc;

use antigravity_core::config::{load_config, expand_path, SchedulingMode as CoreSchedulingMode};
use antigravity_core::proxy::{ProxyServer, TokenManager, StickySessionConfig, SchedulingMode};

pub async fn run(config_path: Option<PathBuf>, port_override: Option<u16>) -> anyhow::Result<()> {
    // Load configuration
    let mut config = load_config(config_path)?;
    
    // Apply port override if provided
    if let Some(port) = port_override {
        config.server.port = port;
    }
    
    let accounts_dir = expand_path(&config.accounts.directory);
    
    tracing::info!("Starting Antigravity Proxy...");
    tracing::info!("  Port: {}", config.server.port);
    tracing::info!("  Host: {}", config.server.host);
    tracing::info!("  Accounts directory: {:?}", accounts_dir);
    
    // Initialize token manager
    let token_manager = Arc::new(TokenManager::new(
        accounts_dir.parent().unwrap_or(&accounts_dir).to_path_buf()
    ));
    
    // Load accounts
    let account_count = token_manager.load_accounts().await?;
    if account_count == 0 {
        tracing::warn!("No accounts found. Please add accounts via the GUI or import them.");
        tracing::warn!("The proxy will start but requests will fail without valid accounts.");
    } else {
        tracing::info!("Loaded {} account(s)", account_count);
    }
    
    // Update scheduling config
    let scheduling = StickySessionConfig {
        mode: match config.scheduling.mode {
            CoreSchedulingMode::PerformanceFirst => SchedulingMode::PerformanceFirst,
            CoreSchedulingMode::Balance => SchedulingMode::Balance,
            CoreSchedulingMode::CacheFirst => SchedulingMode::CacheFirst,
        },
        max_wait_seconds: config.scheduling.max_wait_seconds,
    };
    token_manager.update_sticky_config(scheduling).await;
    
    // Create and start server
    let server = ProxyServer::new(
        config.server.host.clone(),
        config.server.port,
        token_manager,
        config.model_mapping.anthropic.clone(),
        config.model_mapping.openai.clone(),
        config.model_mapping.custom.clone(),
        config.timeouts.request_timeout,
        config.auth.mode.clone(),
        config.auth.api_key.clone(),
    );
    
    tracing::info!("Proxy server starting on http://{}:{}", config.server.host, config.server.port);
    tracing::info!("Press Ctrl+C to stop");
    
    // Run server (blocks until shutdown)
    server.run().await?;
    
    Ok(())
}
