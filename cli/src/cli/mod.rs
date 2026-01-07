pub mod commands;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "antigravity-proxy")]
#[command(author, version, about = "API Proxy CLI - Route OpenAI/Claude requests to Google Gemini")]
pub struct Cli {
    /// Path to config file (checked in order: local config.toml, ~/.config/antigravity-proxy/config.toml)
    #[arg(short, long, global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the proxy server
    Start {
        /// Port to listen on (overrides config)
        #[arg(short, long)]
        port: Option<u16>,
    },
    
    /// Manage accounts
    Accounts {
        #[command(subcommand)]
        command: AccountCommands,
    },
    
    /// Query quota information
    Quota {
        /// Show all accounts
        #[arg(long)]
        all: bool,
        
        /// Specific account email
        #[arg(short, long)]
        account: Option<String>,
    },
    
    /// Show proxy status
    Status,
    
    /// Generate a new API key
    GenerateKey,
}

#[derive(Subcommand)]
pub enum AccountCommands {
    /// List all accounts
    List,
    
    /// Import accounts from a token file
    Import {
        /// Path to token file
        path: PathBuf,
    },
}
