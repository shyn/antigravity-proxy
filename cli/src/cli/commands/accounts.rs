use crate::cli::AccountCommands;
use antigravity_core::account::{list_accounts, get_accounts_dir};

pub async fn run(command: AccountCommands) -> anyhow::Result<()> {
    match command {
        AccountCommands::List => {
            list().await?;
        }
        AccountCommands::Import { path } => {
            import(&path).await?;
        }
    }
    Ok(())
}

async fn list() -> anyhow::Result<()> {
    let accounts = list_accounts()?;
    
    if accounts.is_empty() {
        println!("No accounts found.");
        println!("Accounts directory: {:?}", get_accounts_dir()?);
        return Ok(());
    }
    
    println!("{:<40} {:<30} {:<10}", "EMAIL", "NAME", "STATUS");
    println!("{}", "-".repeat(80));
    
    for account in accounts {
        let status = if account.disabled {
            "disabled"
        } else if account.proxy_disabled {
            "proxy-off"
        } else {
            "active"
        };
        
        let name = account.name.unwrap_or_else(|| "-".to_string());
        println!("{:<40} {:<30} {:<10}", account.email, name, status);
    }
    
    Ok(())
}

async fn import(path: &std::path::Path) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("File not found: {:?}", path);
    }
    
    let content = std::fs::read_to_string(path)?;
    let data: serde_json::Value = serde_json::from_str(&content)?;
    
    // Try different import formats
    if let Some(accounts) = data.as_array() {
        // Array of accounts
        for account in accounts {
            import_single_account(account)?;
        }
    } else if data.get("token").is_some() {
        // Single account object
        import_single_account(&data)?;
    } else {
        anyhow::bail!("Unrecognized account format");
    }
    
    println!("Import completed successfully.");
    Ok(())
}

fn import_single_account(data: &serde_json::Value) -> anyhow::Result<()> {
    let email = data.get("email")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing email field"))?;
    
    println!("Importing account: {}", email);
    
    // Save to accounts directory
    let accounts_dir = get_accounts_dir()?;
    let id = data.get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    
    let path = accounts_dir.join(format!("{}.json", id));
    std::fs::write(&path, serde_json::to_string_pretty(&data)?)?;
    
    println!("  Saved to: {:?}", path);
    Ok(())
}
