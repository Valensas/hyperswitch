# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview
Hyperswitch is a modular, open-source payments infrastructure built in Rust. It provides a unified API for multiple payment processors and includes features like smart routing, retries, vaulting, and observability.

## Development Commands

### Build & Compilation
```bash
# Basic build
cargo build

# Release build (optimized)
cargo build --release

# Check compilation without building
cargo check

# Build specific crate
cargo build -p router
```

### Testing
```bash
# Run all tests
cargo test --all-features

# Run tests with nextest (faster, parallel execution)
cargo nextest run

# Run a single test
cargo test test_name

# Run tests for a specific crate
cargo test -p crate_name
```

### Code Quality
```bash
# Format code (requires nightly)
cargo +nightly fmt --all

# Check formatting without applying
cargo +nightly fmt --all -- --check

# Run clippy lints
cargo clippy --all-features --all-targets -- -D warnings

# Using just commands
just fmt        # Format code
just clippy     # Run clippy for v1 features
just clippy_v2  # Run clippy for v2 features
```

### Local Setup
```bash
# Quick local setup with Docker
scripts/setup.sh

# This provides multiple deployment profiles:
# - Standard: App server + Control Center
# - Full: Includes monitoring + schedulers
# - Minimal: Standalone App server
```

## Architecture

### Workspace Structure
The project uses a Cargo workspace with multiple crates in `/crates/`:

- **router**: Main application server, handles API requests and payment processing
- **api_models**: API request/response models and types
- **diesel_models**: Database models and schema definitions
- **hyperswitch_domain_models**: Core domain models and business logic
- **hyperswitch_connectors**: Payment processor connector implementations
- **hyperswitch_interfaces**: Trait definitions for connectors and services
- **external_services**: Integrations with external services
- **storage_impl**: Database storage implementation layer
- **redis_interface**: Redis caching and session management
- **analytics**: Analytics and reporting functionality
- **drainer**: Background job processing
- **scheduler**: Task scheduling system

### Core Modules Organization
The main router crate (`/crates/router/src/`) contains:
- `core/`: Business logic for payments, refunds, disputes, etc.
- `connector/`: Individual payment processor integrations
- `routes/`: HTTP API endpoints and routing
- `services/`: Authentication, encryption, API client services
- `db/`: Database operations and queries
- `workflows/`: Payment flow orchestration

### Feature Flags
The project uses feature flags to control different versions and capabilities:
- `v1`: Version 1 API (default, stable)
- `v2`: Version 2 API (experimental, mutually exclusive with v1)
- `stripe`: Stripe compatibility layer
- `olap`: Analytics and reporting features

When working with features, note that v1 and v2 are mutually exclusive.

## Key Patterns

### Error Handling
The codebase uses comprehensive error types with the `error-stack` crate. Errors are typically propagated using `?` operator and converted at API boundaries.

### Database Access
Uses Diesel ORM with PostgreSQL. Database models are in `diesel_models` crate, with migrations managed separately.

### API Versioning
Supports multiple API versions through feature flags and conditional compilation. Headers like `X-API-VERSION` control version routing.

### Connector Integration
Payment processors are integrated through a trait-based system defined in `hyperswitch_interfaces`. Each connector implements standard traits for payments, refunds, etc.

## Important Notes

- **Rust Version**: Requires Rust 1.85.0 or later
- **Unsafe Code**: Forbidden by workspace lint configuration
- **Linting**: Strict clippy configuration with many lints set to warn/deny
- **Testing**: Prefer `cargo nextest` for faster test execution
- **Formatting**: Always use nightly toolchain for formatting (`cargo +nightly fmt`)

## NATS Bridge Connector Development

When working with the NATS bridge connector:

### Key Files
- `crates/hyperswitch_connectors/src/connectors/nats_bridge/` - Main connector implementation
- `NATS.md` - Architecture and design documentation
- `NATS_TODO.md` - Implementation checklist with trait mappings

### Running Connector Generator
```bash
# Generate new connector scaffold
sh scripts/add_connector.sh connector_name base_url

# For NATS bridge
sh scripts/add_connector.sh nats_bridge nats://localhost:4222
```

### Connector Testing
```bash
# Test specific connector
cargo test --package router --test connectors -- nats_bridge

# Run with test credentials
export CONNECTOR_AUTH_FILE_PATH=crates/router/tests/connectors/sample_auth.toml
```

### NATS-Specific Development Notes
- The NATS bridge delegates all payment operations to external services via messaging
- Workers can be implemented in any language (Python, Go, Node.js, etc.)
- JetStream provides persistence for critical financial operations
- Request/Reply pattern used for real-time queries and validations
- See `NATS_TODO.md` for complete trait-to-NATS mapping