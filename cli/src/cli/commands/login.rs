//! Login command - authenticate with Google OAuth

use antigravity_core::oauth::start_auth_flow;

pub async fn run() -> anyhow::Result<()> {
    println!("Starting Google OAuth login...");
    println!("A browser window will open for authorization.");
    println!();
    
    match start_auth_flow().await {
        Ok(result) => {
            println!();
            println!("✓ Login successful!");
            println!("  Email: {}", result.account.email);
            println!("  Account ID: {}", result.account.id);
            println!();
            println!("You can now start the proxy with: antigravity-proxy start");
        }
        Err(e) => {
            eprintln!("Login failed: {}", e);
            return Err(e);
        }
    }
    
    Ok(())
}
