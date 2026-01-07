use clap::{Parser};

mod cli;

use cli::{Cli, Commands};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("antigravity_proxy=info".parse()?)
                .add_directive("antigravity_core=info".parse()?)
                .add_directive("tower_http=debug".parse()?)
        )
        .init();
    
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Start { port } => {
            cli::commands::start::run(cli.config, port).await?;
        }
        Commands::Accounts { command } => {
            cli::commands::accounts::run(command).await?;
        }
        Commands::Quota { all, account } => {
            cli::commands::quota::run(all, account).await?;
        }
        Commands::Status => {
            cli::commands::status::run().await?;
        }
        Commands::GenerateKey => {
            cli::commands::generate_key::run();
        }
    }
    
    Ok(())
}
