# Antigravity Proxy

> [!WARNING]
> **免责声明 (Disclaimer)**: 
> 本项目是基于逆向工程开发的。我们不保证使用本项目不会导致您的账号被封禁，也不保证本项目在未来能持续正常工作。请在遵守相关服务条款的前提下谨慎使用。
> 
> This project is developed based on reverse engineering. We do not guarantee that using this project will not result in your account being banned, nor do we guarantee that this project will continue to function in the future. Please use with caution and in compliance with the relevant terms of service.

Antigravity Proxy is a high-performance Rust-based proxy server designed for API mapping, authentication, and client communication. It consists of a core library and a command-line interface (CLI).

## Features

- **API Mapping**: Transform and route API requests between different protocols.
- **Authentication**: Built-in support for OAuth and configuration-based authentication.
- **Quota Management**: Track and manage usage quotas across different accounts.
- **Proxy Server**: Robust proxy implementation for handling client requests.
- **CLI Tool**: Powerful command-line interface for managing the proxy, accounts, and keys.

## Project Structure

- `core/`: Shared logic, including configuration, account management, and proxy core.
- `cli/`: Command-line interface implementation for interacting with the proxy.

## Getting Started

### Prerequisites

- Rust (latest stable version)
- Cargo

### Installation

Clone the repository and build the project:

```bash
git clone https://github.com/shyn/antigravity-proxy.git
cd antigravity-proxy
cargo build --release
```

### Configuration

Copy the example configuration file and modify it to suit your needs:

```bash
cp cli/config.example.toml config.toml
```

## Usage

The CLI provides several commands to manage the proxy:

- `start`: Start the proxy server.
- `accounts`: Manage proxy accounts.
- `quota`: View and manage usage quotas.
- `status`: Check the status of the proxy.
- `generate-key`: Generate a new API key.

To see all available options, run:

```bash
cargo run --package antigravity-proxy -- --help
```

## License

[Add License Information Here]
