# Antigravity Proxy

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
git clone https://github.com/your-repo/antigravity-proxy.git
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
