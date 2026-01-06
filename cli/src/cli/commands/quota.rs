use antigravity_core::account::list_accounts;
use antigravity_core::quota::fetch_quota_detailed;
use antigravity_core::oauth::ensure_fresh_token;

pub async fn run(all: bool, account_email: Option<String>) -> anyhow::Result<()> {
    let accounts = list_accounts()?;
    
    if accounts.is_empty() {
        println!("No accounts found.");
        return Ok(());
    }
    
    let accounts_to_check: Vec<_> = if let Some(email) = account_email {
        accounts.into_iter()
            .filter(|a| a.email == email)
            .collect()
    } else if all {
        accounts
    } else {
        // Default: show first active account
        accounts.into_iter()
            .filter(|a| !a.disabled && !a.proxy_disabled)
            .take(1)
            .collect()
    };
    
    if accounts_to_check.is_empty() {
        println!("No matching accounts found.");
        return Ok(());
    }
    
    for account in accounts_to_check {
        // Refresh token if needed
        let token = match ensure_fresh_token(&account.token).await {
            Ok(t) => t,
            Err(e) => {
                println!("\n❌ {} - Token refresh failed: {}", account.email, e);
                continue;
            }
        };
        
        // Fetch quota
        match fetch_quota_detailed(&token.access_token, &account.email).await {
            Ok((tier, models)) => {
                let tier_str = tier.as_deref().unwrap_or("FREE");
                
                println!("\n📊 {} ({})", account.email, tier_str);
                println!("{}", "=".repeat(60));
                
                if models.is_empty() {
                    println!("  No quota data available");
                } else {
                    println!("{:<45} {:>8} {:>8} {:>15}", "MODEL", "USED", "LEFT", "WAIT TIME");
                    println!("{}", "-".repeat(78));
                    
                    for model in &models {
                        // Create a visual bar
                        let bar_width = 10;
                        let filled = (model.used_pct as usize * bar_width / 100).min(bar_width);
                        let bar: String = "█".repeat(filled) + &"░".repeat(bar_width - filled);
                        
                        let wait_time = model.reset_time.as_deref().unwrap_or("-");
                        
                        println!("{:<45} {:>4}% {:>4}%  [{}] {:>15}", 
                            &model.model_name,
                            model.used_pct,
                            model.remaining_pct,
                            bar,
                            wait_time
                        );
                    }
                }
            }
            Err(e) => {
                println!("\n❌ {} - Error: {}", account.email, e);
            }
        }
    }
    
    println!();
    Ok(())
}
