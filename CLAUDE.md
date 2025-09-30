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

### NATS Protocol Guidelines

**Architecture**: merchant_id in BOTH headers AND body

#### Subject Naming (NO merchant_id)
```rust
// ✅ CORRECT - Simple subjects
"hyperswitch.payments.authorize"
"hyperswitch.refunds.execute"
"hyperswitch.disputes.accept"

// ❌ WRONG - Do NOT include merchant_id in subject
"hyperswitch.payments.{merchant_id}.authorize"  // DEPRECATED
```

#### Message Structure (Headers + Body)
```rust
// Headers (NATS observability metadata)
Headers {
    "Nats-Msg-Id": "pay_xyz789_attempt_def456",  // Idempotency
    "X-Merchant-Id": "merchant_123",               // Observability
    "X-Payment-Id": "pay_xyz789",                  // Observability
    "X-Correlation-Id": "uuid-v4"                  // Tracing
}

// Body (Authoritative application data)
{
    "merchant_id": "merchant_123",  // MUST match header
    "payment_id": "pay_xyz789",
    "router_data": { /* full RouterData */ }
}
```

#### Implementation Rules

1. **Publishing Messages**:
   - Subject: Domain + operation only (no merchant_id)
   - Headers: Include X-Merchant-Id, X-Correlation-Id, Nats-Msg-Id
   - Body: Full RouterData with merchant_id field
   - merchant_id MUST be in BOTH headers and body

2. **Consuming Messages** (Workers):
   - Subscribe to simple subjects (no wildcards needed)
   - Headers are OPTIONAL (for observability/logging)
   - Body is AUTHORITATIVE (always use for processing)
   - Validate header/body consistency (recommended)

3. **Worker Subscriptions**:
   ```rust
   // ✅ All merchants, single operation
   subscribe("hyperswitch.payments.authorize")

   // ✅ All merchants, all payment operations
   subscribe("hyperswitch.payments.>")

   // ❌ NO wildcards needed for merchant_id
   subscribe("hyperswitch.payments.*.authorize")  // UNNECESSARY
   ```

4. **Validation Pattern** (Optional but Recommended):
   ```rust
   async fn process_message(msg: Message) -> Result<()> {
       // Optional: Check header (fast)
       let merchant_header = msg.headers.get("X-Merchant-Id");

       // Required: Parse body (authoritative)
       let payment: PaymentMessage = serde_json::from_slice(&msg.payload)?;

       // Optional: Validate consistency
       if let Some(h) = merchant_header {
           if h != &payment.merchant_id {
               log::warn!("Header/body mismatch - using body: {}", payment.merchant_id);
           }
       }

       // ALWAYS use body
       process_payment(&payment.merchant_id, &payment.router_data).await
   }
   ```

5. **JetStream vs Request-Reply**:
   - **JetStream (Persistent)**: Financial ops (authorize, capture, refund)
   - **Request-Reply (Ephemeral)**: Queries (sync, session, validate)

#### Key Benefits of Option D

- ✅ No subject explosion (scales to 1000s of merchants)
- ✅ NATS observability via headers (no body parsing needed)
- ✅ Application logic unchanged (body remains authoritative)
- ✅ Simple worker subscriptions (no wildcards)
- ✅ Industry standard pattern (HTTP, Kafka, RabbitMQ)

#### Development Resources

- **[NATS.md](./NATS.md)** - Complete protocol guide with worker examples
- **[NATS_TODO.md](./NATS_TODO.md)** - Implementation checklist with trait mappings

#### Common Mistakes to Avoid

1. ❌ Including merchant_id in subjects
2. ❌ Using wildcard subscriptions when not needed
3. ❌ Trusting headers over body for processing
4. ❌ Forgetting to add headers (breaks NATS observability)
5. ❌ Using JetStream for read-only queries (use Request-Reply)

#### Testing with NATS CLI

```bash
# Start local NATS with JetStream
nats-server -js

# Create stream
nats stream add HYPERSWITCH_PAYMENTS \
  --subjects "hyperswitch.payments.>" \
  --retention limits --storage file

# Publish test message with Option D
nats pub hyperswitch.payments.authorize \
  --header "X-Merchant-Id:merchant_123" \
  --header "X-Payment-Id:pay_test" \
  '{"merchant_id":"merchant_123","payment_id":"pay_test","router_data":{}}'

# Subscribe with headers visible
nats sub "hyperswitch.payments.>" --headers
```