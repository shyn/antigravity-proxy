use antigravity_core::config::{load_config, expand_path, default_config_path};
use antigravity_core::account::list_accounts;

pub async fn run() -> anyhow::Result<()> {
    // For now, just show a simple status message
    // In the future, this could check if a server is running on the configured port
    
    let config = load_config(None)?;
    let accounts_dir = expand_path(&config.accounts.directory);
    
    println!("Antigravity Proxy Status");
    println!("========================");
    println!();
    println!("Configuration:");
    println!("  Config file: {:?}", default_config_path());
    println!("  Accounts dir: {:?}", accounts_dir);
    println!();
    println!("Server settings:");
    println!("  Host: {}", config.server.host);
    println!("  Port: {}", config.server.port);
    println!("  Auth mode: {:?}", config.auth.mode);
    println!();
    
    // Count accounts
    let accounts = list_accounts()?;
    let active = accounts.iter().filter(|a| !a.disabled && !a.proxy_disabled).count();
    let disabled = accounts.iter().filter(|a| a.disabled || a.proxy_disabled).count();
    
    println!("Accounts:");
    println!("  Total: {}", accounts.len());
    println!("  Active: {}", active);
    println!("  Disabled: {}", disabled);
    
    // Check if server is reachable
    println!();
    let url = format!("http://{}:{}/healthz", config.server.host, config.server.port);
    match reqwest::get(&url).await {
        Ok(resp) if resp.status().is_success() => {
            println!("Server: RUNNING ✓");
        }
        _ => {
            println!("Server: NOT RUNNING");
        }
    }
    
    Ok(())
}
