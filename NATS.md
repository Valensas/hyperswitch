# NATS Bridge Connector for Hyperswitch

The NATS Bridge Connector enables Hyperswitch to delegate all payment operations to external services via NATS messaging. This provides complete decoupling of payment processing from the Hyperswitch core while maintaining full compatibility with the payment lifecycle.

## Key Benefits

- **Language-agnostic**: Workers in Python, Go, Node.js, Java, etc.
- **Scalable**: Horizontal scaling via NATS queue groups
- **Reliable**: JetStream persistence for financial operations
- **Observable**: Headers + body provide full tracing
- **Flexible**: Easy to add/change payment processors

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                  Hyperswitch Platform                   │
│  ┌──────────────┐         ┌─────────────────────────-┐  │
│  │ Router Core  │────────▶│ NATS Bridge Connector    │  │
│  │   (Rust)     │         │ - All payment traits     │  │
│  └──────────────┘         │ - Serialize RouterData   │  │
│                           │ - Publish to NATS        │  │
│                           └────────────-┬────────────┘  │
└────────────────────────────────────────-┼───────────────┘
                                          │
                        Headers + Body    │
                                          ▼
                                  ┌──────────────┐
                                  │ NATS Server  │
                                  │ (JetStream)  │
                                  └──────┬───────┘
                                          │
              ┌───────────────────────────┼──────────────────┐
              │                           │                  │
              ▼                           ▼                  ▼
      ┌───────────────┐           ┌──────────────┐  ┌──────────────┐
      │Stripe Worker  │           │Adyen Worker  │  │Custom Worker │
      │  (Python)     │           │    (Go)      │  │  (Node.js)   │
      └───────────────┘           └──────────────┘  └──────────────┘
```

---

## Protocol Design

### Message Structure

**Subject**: Domain + operation only (no merchant_id)
```
hyperswitch.payments.authorize
hyperswitch.payments.capture
hyperswitch.refunds.execute
```

**Headers**: NATS observability metadata
```
X-Merchant-Id: merchant_123
X-Payment-Id: pay_xyz789
X-Correlation-Id: uuid-v4
Nats-Msg-Id: pay_xyz789_attempt_def456
```

**Body**: Authoritative application data
```json
{
  "merchant_id": "merchant_123",
  "payment_id": "pay_xyz789",
  "router_data": { /* full RouterData */ }
}
```

### Why Headers + Body?

- **Headers**: NATS observability without parsing body
- **Body**: Application logic remains unchanged (authoritative)
- **No subject explosion**: Scales to thousands of merchants
- **Simple subscriptions**: No wildcards needed
- **Industry standard**: Same pattern as HTTP, Kafka, RabbitMQ

---

## Quick Start

### 1. NATS Server Setup

```bash
# Start NATS with JetStream
docker run -p 4222:4222 -p 8222:8222 nats:latest \
  -js \
  -m 8222

# Create streams
nats stream add HYPERSWITCH_PAYMENTS \
  --subjects "hyperswitch.payments.>" \
  --retention limits \
  --storage file \
  --replicas 3 \
  --max-age 30d \
  --duplicate-window 5m
```

### 2. Hyperswitch Configuration

```toml
# config/development.toml
[connectors.nats_bridge]
base_url = "nats://localhost:4222"
jetstream_enabled = true
request_timeout_ms = 30000
connection_name = "hyperswitch-nats-bridge"
```

### 3. Worker Implementation

#### Python Worker Example

```python
import asyncio
import json
from nats.aio.client import Client as NATS

async def handle_authorize(msg):
    # Parse body (authoritative)
    payment_request = json.loads(msg.data)
    merchant_id = payment_request['merchant_id']
    router_data = payment_request['router_data']

    # Optional: Validate header matches body
    merchant_id_header = msg.headers.get('X-Merchant-Id')
    if merchant_id_header and merchant_id_header != merchant_id:
        print(f"Warning: Header/body mismatch - using body: {merchant_id}")

    # Process with actual gateway (e.g., Stripe)
    result = await process_stripe_payment(router_data)

    # Send response
    response = {
        'status': 'success',
        'transaction_id': result['id'],
        'connector_response': result,
        'correlation_id': payment_request['correlation_id']
    }

    await msg.respond(json.dumps(response).encode())

async def main():
    nc = await NATS().connect("nats://localhost:4222")
    js = nc.jetstream()

    # Subscribe to authorization requests (all merchants)
    await js.subscribe(
        "hyperswitch.payments.authorize",
        cb=handle_authorize,
        queue="payment-workers",  # Queue group for load balancing
        durable="payment-processor"
    )

    print("Worker ready - listening on hyperswitch.payments.authorize")

    # Keep running
    while True:
        await asyncio.sleep(1)

if __name__ == '__main__':
    asyncio.run(main())
```

#### Go Worker Example

```go
package main

import (
    "encoding/json"
    "log"
    "github.com/nats-io/nats.go"
)

type PaymentRequest struct {
    MerchantID    string      `json:"merchant_id"`
    PaymentID     string      `json:"payment_id"`
    RouterData    interface{} `json:"router_data"`
    CorrelationID string      `json:"correlation_id"`
}

func handleAuthorize(msg *nats.Msg) {
    // Parse body (authoritative)
    var req PaymentRequest
    json.Unmarshal(msg.Data, &req)

    // Optional: Validate consistency
    merchantIDHeader := msg.Header.Get("X-Merchant-Id")
    if merchantIDHeader != "" && merchantIDHeader != req.MerchantID {
        log.Printf("Warning: Header/body mismatch - using body: %s", req.MerchantID)
    }

    // Process payment with Stripe/Adyen/etc.
    result := processPayment(req.RouterData)

    // Send response
    response, _ := json.Marshal(map[string]interface{}{
        "status":          "success",
        "transaction_id":  result.ID,
        "correlation_id":  req.CorrelationID,
    })

    msg.Respond(response)
}

func main() {
    nc, _ := nats.Connect("nats://localhost:4222")
    js, _ := nc.JetStream()

    // Subscribe with queue group
    js.QueueSubscribe(
        "hyperswitch.payments.authorize",
        "payment-workers",
        handleAuthorize,
        nats.Durable("payment-processor"),
    )

    log.Println("Worker ready - listening on hyperswitch.payments.authorize")
    select {} // Keep running
}
```

---

## Subject Catalog

### Payment Operations
```
hyperswitch.payments.authorize          # JetStream - Financial
hyperswitch.payments.capture            # JetStream - Financial
hyperswitch.payments.void               # JetStream - Financial
hyperswitch.payments.complete_authorize # JetStream - 3DS completion
hyperswitch.payments.sync               # Request-Reply - Status check
hyperswitch.payments.session            # Request-Reply - Token generation
```

### Refund Operations
```
hyperswitch.refunds.execute             # JetStream - Financial
hyperswitch.refunds.sync                # Request-Reply - Status check
```

### Other Operations
```
hyperswitch.mandates.setup              # JetStream - Recurring setup
hyperswitch.mandates.revoke             # JetStream - Cancel recurring
hyperswitch.disputes.accept             # JetStream - Legal/compliance
hyperswitch.disputes.defend             # JetStream - Evidence submission
hyperswitch.webhooks.incoming           # JetStream - Event processing
hyperswitch.auth.access_token           # Request-Reply - Token fetch
```

---

## Message Patterns

### JetStream (Persistent) - For Financial Operations

```rust
// Publisher (Hyperswitch)
let headers = Headers::new()
    .insert("Nats-Msg-Id", &format!("{}_{}", payment_id, attempt_id))
    .insert("X-Merchant-Id", &merchant_id)
    .insert("X-Payment-Id", &payment_id);

jetstream.publish_with_headers(
    "hyperswitch.payments.authorize",
    headers,
    serde_json::to_vec(&message)?
).await?;

// Worker response
msg.ack().await?;  // Acknowledge after processing
```

### Request-Reply (Ephemeral) - For Queries

```rust
// Publisher (Hyperswitch)
let response = client
    .request("hyperswitch.payments.sync", message)
    .timeout(Duration::from_secs(5))
    .await?;

// Worker response
msg.respond(response_data).await?;
```

---

## Worker Subscription Patterns

### Simple Subscription (All Merchants)
```rust
// Single worker handles all merchants
subscribe("hyperswitch.payments.authorize")
```

### Queue Group (Load Balancing)
```rust
// Multiple workers share load
queue_subscribe("hyperswitch.payments.authorize", "payment-workers")
```

### Multiple Operations
```rust
// Handle multiple operations
subscribe("hyperswitch.payments.>")  // All payment operations
subscribe("hyperswitch.refunds.>")   // All refund operations
```

---

## Configuration

### NATS Connection

```rust
let client = async_nats::connect_with_options(
    "nats://localhost:4222",
    ConnectOptions::new()
        .connection_timeout(Duration::from_secs(10))
        .reconnect_buffer_size(256)
        .max_reconnects(None)  // Infinite reconnects
        .ping_interval(Duration::from_secs(30))
        .name("hyperswitch-nats-bridge")
).await?;
```

### JetStream Stream

```yaml
name: "HYPERSWITCH_PAYMENTS"
subjects: ["hyperswitch.payments.>"]
retention: "limits"
max_age: "30d"
max_messages: 10_000_000
storage: "file"
num_replicas: 3
duplicate_window: "5m"  # Idempotency
```

### JetStream Consumer

```yaml
durable_name: "payment-processor"
ack_policy: "explicit"
max_deliver: 5
ack_wait: "30s"
backoff: ["1s", "5s", "15s", "30s", "60s"]
```

---

## Error Handling

### Validation Pattern

```rust
async fn process_message(msg: Message) -> Result<()> {
    // Optional: Check header (observability)
    let merchant_header = msg.headers.get("X-Merchant-Id");

    // Required: Parse body (authoritative)
    let payment: PaymentMessage = serde_json::from_slice(&msg.payload)?;

    // Optional: Validate consistency
    if let Some(header) = merchant_header {
        if header != &payment.merchant_id {
            log::warn!("Header/body mismatch - using body: {}", payment.merchant_id);
        }
    }

    // ALWAYS use body for processing
    process_payment(&payment.merchant_id, &payment.router_data).await
}
```

### Retry Strategy

- **Retryable**: Network errors, timeouts, rate limits
- **Non-retryable**: Invalid card, insufficient funds, validation errors
- **Exponential backoff**: 1s, 5s, 15s, 30s, 60s

---

## Monitoring

### Key Metrics

- Message processing latency (p50, p95, p99)
- Success/failure rates per operation
- Consumer lag (should be < 1000 messages)
- Worker availability
- Header/body validation mismatches (should be 0)

### NATS CLI Commands

```bash
# Stream health
nats stream info HYPERSWITCH_PAYMENTS

# Consumer lag
nats consumer info HYPERSWITCH_PAYMENTS payment-processor

# Monitor messages with headers
nats sub "hyperswitch.payments.>" --headers

# Check for errors
nats stream view HYPERSWITCH_DLQ
```

---

## Security

### Transport Security

- TLS 1.3 for NATS connections
- Certificate-based authentication
- Separate NATS clusters per environment

### Message Security

- Sensitive data in encrypted body
- Headers for routing only (no PII)
- Application-level merchant isolation

### Access Control

- NATS accounts for organization isolation
- Worker credentials per deployment
- Subject-based permissions

---

## Testing

### Unit Tests

```rust
#[tokio::test]
async fn test_payment_authorize() {
    let msg = create_test_message();

    // Validate headers
    assert_eq!(msg.headers.get("X-Merchant-Id"), Some(&"merchant_123"));

    // Validate body
    let payment: PaymentMessage = serde_json::from_slice(&msg.payload)?;
    assert_eq!(payment.merchant_id, "merchant_123");

    // Validate consistency
    assert_eq!(msg.headers.get("X-Merchant-Id").unwrap(), &payment.merchant_id);
}
```

### Integration Tests

- End-to-end payment flows
- Header/body consistency validation
- Worker failover scenarios
- Message replay from JetStream

---

## Troubleshooting

### Common Issues

**Headers and body don't match**:
```bash
# Check for serialization bugs
nats sub "hyperswitch.payments.>" --headers | grep "X-Merchant-Id"
```

**Consumer lag increasing**:
```bash
# Check worker health
nats consumer info HYPERSWITCH_PAYMENTS payment-processor
# Scale workers horizontally
```

**Messages going to DLQ**:
```bash
# Investigate failed messages
nats stream view HYPERSWITCH_DLQ --last
```

---

## Implementation Phases

### Phase 1: MVP (Core Payments)
- Payment operations: Authorize, Capture, Void, Sync
- Refund operations: Execute, Sync
- JetStream setup
- Single worker implementation (Python or Rust)

### Phase 2: Complete Flows
- Complete authorization (3DS)
- Webhook processing
- Mandate operations
- Enhanced error handling

### Phase 3: Advanced Features
- Dispute management
- Payout operations (if enabled)
- File uploads
- Multi-region support

### Phase 4: Production Readiness
- Distributed tracing (OpenTelemetry)
- Grafana dashboards
- Alerting rules
- Load testing

---

## Related Documentation

- **[NATS_TODO.md](./NATS_TODO.md)** - Implementation checklist with trait mappings
