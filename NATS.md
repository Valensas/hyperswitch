# NATS Bridge Connector for Hyperswitch

## Overview

The NATS Bridge Connector is a universal adapter that enables Hyperswitch to delegate all payment operations to external services via NATS messaging. This provides complete decoupling of payment processing from the Hyperswitch core while maintaining full compatibility with the v1 payment lifecycle.

## Architecture

```
Hyperswitch Core
       ↓
NATS Bridge Connector (implements all Payment traits)
       ↓
NATS/JetStream (Message Broker)
       ↓
Worker Services (any language)
       ↓
Actual Payment Processors (Stripe, Adyen, etc.)
```

## Core Concepts

### 1. Payment Lifecycle Compatibility

The NATS bridge implements all required Hyperswitch payment traits:
- `Payment` - Main payment trait
- `PaymentAuthorize` - Authorization operations
- `PaymentCapture` - Capture operations
- `PaymentSync` - Payment synchronization
- `PaymentVoid` - Cancellation operations
- `PaymentSession` - Session management
- `MandateSetup` - Recurring payment setup
- `PaymentToken` - Tokenization support
- `RefundExecute` - Refund operations
- `RefundSync` - Refund synchronization

### 2. Message Structure

#### Request Message
```json
{
  "operation": "authorize|capture|void|sync|refund",
  "merchant_id": "merchant_123",
  "payment_id": "pay_abc123",
  "router_data": { /* Serialized RouterData */ },
  "metadata": {
    "key": "value"
  },
  "correlation_id": "uuid-v4",
  "timestamp": 1234567890
}
```

#### Response Message
```json
{
  "status": "success|failure|processing",
  "transaction_id": "txn_123",
  "connector_response": { /* Original processor response */ },
  "error": {
    "code": "E001",
    "message": "Error description"
  },
  "correlation_id": "uuid-v4"
}
```

## NATS Subject Hierarchy

```
hyperswitch.payments.{merchant_id}.authorize
hyperswitch.payments.{merchant_id}.capture
hyperswitch.payments.{merchant_id}.void
hyperswitch.payments.{merchant_id}.sync
hyperswitch.payments.{merchant_id}.refund
hyperswitch.payments.{merchant_id}.setup_mandate
hyperswitch.webhooks.{merchant_id}.{connector}
```

## JetStream Configuration

### Stream Setup
```javascript
{
  "name": "PAYMENTS",
  "subjects": ["hyperswitch.payments.>"],
  "retention": "limits",
  "max_messages": 1000000,
  "max_age": "7d",
  "storage": "file",
  "num_replicas": 3,
  "duplicate_window": "2m"  // Deduplication window
}
```

### Consumer Groups
- Each operation type has its own consumer group
- Multiple workers can subscribe for load balancing
- At-least-once delivery guarantee with manual acknowledgment

## Implementation Details

### Connector Structure

The NATS bridge connector follows Hyperswitch's standard connector pattern:

```
crates/hyperswitch_connectors/src/connectors/nats_bridge/
├── mod.rs          # Main connector implementation
├── transformers.rs # Data transformation logic
└── types.rs        # NATS-specific types
```

### Key Components

1. **NatsBridge Struct**
   - Contains NATS client and JetStream context
   - Implements ConnectorCommon trait
   - Manages connection pooling

2. **ConnectorIntegration Implementation**
   - Each payment operation publishes to specific NATS subject
   - Uses request-reply pattern for synchronous operations
   - Configurable timeout (default: 30 seconds)

3. **Error Handling**
   - Network errors trigger retries
   - Timeout errors return appropriate status
   - Worker errors are propagated back to Hyperswitch

## Configuration

```toml
[nats_bridge]
base_url = "nats://localhost:4222"
jetstream_enabled = true
request_timeout_ms = 30000
max_retries = 3
connection_name = "hyperswitch-nats-bridge"

[nats_bridge.subjects]
authorize = "hyperswitch.payments.{merchant_id}.authorize"
capture = "hyperswitch.payments.{merchant_id}.capture"
void = "hyperswitch.payments.{merchant_id}.void"
sync = "hyperswitch.payments.{merchant_id}.sync"
refund = "hyperswitch.payments.{merchant_id}.refund"

[nats_bridge.jetstream]
stream_name = "PAYMENTS"
consumer_name = "hyperswitch-consumer"
ack_wait = 30000
max_deliver = 3
```

## Worker Service Interface

Workers subscribe to NATS subjects and process payment operations. They can be implemented in any language:

### Python Example
```python
import nats
import json

async def handle_payment_authorize(msg):
    data = json.loads(msg.data.decode())
    router_data = data['router_data']

    # Process with actual payment processor
    result = await process_with_stripe(router_data)

    response = {
        'status': 'success',
        'transaction_id': result['id'],
        'connector_response': result,
        'correlation_id': data['correlation_id']
    }

    await msg.respond(json.dumps(response).encode())

async def main():
    nc = await nats.connect("nats://localhost:4222")
    js = nc.jetstream()

    # Subscribe to authorization requests
    await js.subscribe(
        "hyperswitch.payments.*.authorize",
        cb=handle_payment_authorize,
        durable="payment-worker"
    )
```

### Go Example
```go
func handleAuthorize(msg *nats.Msg) {
    var request NatsPaymentRequest
    json.Unmarshal(msg.Data, &request)

    // Process payment
    response := processPayment(request.RouterData)

    responseData, _ := json.Marshal(response)
    msg.Respond(responseData)
}

func main() {
    nc, _ := nats.Connect("nats://localhost:4222")
    js, _ := nc.JetStream()

    js.Subscribe("hyperswitch.payments.*.authorize", handleAuthorize)
}
```

## Benefits

### 1. **Decoupling**
- Hyperswitch core is decoupled from payment processing logic
- Workers can be developed and deployed independently
- Easy to switch or add payment processors

### 2. **Scalability**
- Horizontal scaling by adding more workers
- Load balanced across worker instances
- No direct connection between Hyperswitch and processors

### 3. **Reliability**
- JetStream provides message persistence
- Automatic retries with exponential backoff
- Dead letter queue for failed messages
- Message replay capability

### 4. **Flexibility**
- Workers can be in any language (Python, Go, Node.js, Java)
- Different workers for different merchants/regions
- Dynamic routing based on message metadata

### 5. **Observability**
- Centralized logging through NATS
- Message tracing with correlation IDs
- Metrics on message processing times
- Stream monitoring and alerting

### 6. **Security**
- TLS encryption for NATS connections
- Authentication with NATS credentials
- Subject-based authorization
- Message encryption at application layer

## Message Flow

1. **Payment Initiation**
   - Hyperswitch receives payment request
   - Routes to NATS bridge connector

2. **Message Publishing**
   - Connector serializes RouterData to JSON
   - Publishes to appropriate NATS subject
   - Waits for response (request-reply pattern)

3. **Worker Processing**
   - Worker receives message from NATS
   - Deserializes and processes with actual gateway
   - Sends response back via NATS

4. **Response Handling**
   - NATS bridge receives response
   - Deserializes to PaymentsResponseData
   - Returns to Hyperswitch core

5. **Persistence**
   - All messages stored in JetStream
   - Configurable retention period
   - Available for replay/audit

## Advanced Features

### Load Balancing
- Queue groups for worker load balancing
- Weighted distribution based on worker capacity
- Health checks for worker availability

### Fault Tolerance
- Automatic failover to backup workers
- Circuit breaking at NATS level
- Graceful degradation on partial failures

### Multi-Region Support
- NATS clusters across regions
- Geo-routing based on merchant location
- Regional failover capabilities

### Rate Limiting
- Rate limiting at NATS level
- Per-merchant/per-operation limits
- Backpressure handling

### Message Replay
- Replay failed payments from JetStream
- Time-based replay for reconciliation
- Selective replay with filters

## Security Considerations

1. **Transport Security**
   - TLS 1.3 for NATS connections
   - Certificate-based authentication
   - Mutual TLS support

2. **Message Security**
   - Encrypt sensitive data in messages
   - Sign messages for integrity
   - Audit trail for all operations

3. **Access Control**
   - Subject-based permissions
   - Role-based access for workers
   - Merchant isolation via subjects

## Monitoring & Alerting

### Key Metrics
- Message processing latency
- Success/failure rates per operation
- Worker availability and load
- Stream lag and consumer progress
- Connection health

### Alerting Rules
- High failure rate threshold
- Processing latency SLA breach
- Worker disconnection
- Stream capacity warnings
- Dead letter queue size

## Disaster Recovery

1. **Backup Strategy**
   - Regular JetStream snapshots
   - Cross-region replication
   - Point-in-time recovery

2. **Failover Process**
   - Automatic worker failover
   - Regional failover for NATS cluster
   - Connection retry with exponential backoff

3. **Data Recovery**
   - Message replay from JetStream
   - Reconciliation with payment processors
   - Audit log reconstruction

## RouterData Serialization Strategy

### What to Serialize
The RouterData contains all context needed for payment processing:
- `merchant_id` - For routing and isolation
- `payment_id` - Unique payment identifier
- `connector_auth_type` - Authentication credentials
- `request` - The actual payment/refund data
- `metadata` - Additional context
- `test_mode` - Environment flag

### Serialization Approach
1. Use `serde_json::to_value()` for the entire RouterData
2. Extract critical fields for NATS headers:
   - `X-Merchant-Id`: For subject routing
   - `X-Payment-Id`: For correlation
   - `X-Idempotency-Key`: For deduplication
   - `X-Test-Mode`: For environment routing

### Example Serialization
```rust
let nats_message = NatsPaymentMessage {
    operation: "authorize",
    merchant_id: router_data.merchant_id.clone(),
    payment_id: router_data.payment_id.clone(),
    router_data: serde_json::to_value(&router_data)?,
    headers: HashMap::from([
        ("X-Idempotency-Key", router_data.payment_id.clone()),
        ("X-Test-Mode", router_data.test_mode.to_string()),
    ]),
    correlation_id: Uuid::new_v4().to_string(),
    timestamp: Utc::now().timestamp(),
};
```

## NATS Connection Management

### Connection Pool Strategy
- Maintain single shared NATS client per Hyperswitch instance
- Use `Arc<Client>` for thread-safe sharing
- Implement connection monitoring with health checks
- Auto-reconnect with exponential backoff

### Connection Lifecycle
1. Initialize on first connector use (lazy loading)
2. Share across all payment operations
3. Monitor with heartbeat every 30s
4. Graceful shutdown on application termination

### Connection Configuration
```rust
let client = async_nats::connect_with_options(
    "nats://localhost:4222",
    async_nats::ConnectOptions::new()
        .connection_timeout(Duration::from_secs(10))
        .reconnect_buffer_size(256)
        .max_reconnects(10)
        .ping_interval(Duration::from_secs(30))
        .name("hyperswitch-nats-bridge")
).await?;
```

## Error Recovery Patterns

### Network Failures
- Retry with exponential backoff (1s, 2s, 4s, 8s, max 30s)
- Circuit breaker after 5 consecutive failures
- Fallback to error response after timeout

### Worker Failures
- Dead letter queue for failed messages
- Manual intervention queue for critical failures
- Automatic retry with different worker (round-robin)

### Message Failures
- Validation errors return immediately (no retry)
- Timeout errors trigger single retry
- Duplicate detection via JetStream dedup window

### Error Response Mapping
```rust
match error {
    NatsError::Timeout => ErrorResponse {
        code: "NATS_TIMEOUT",
        message: "Payment processor timeout",
        status_code: 504,
        attempt_status: Some(AttemptStatus::Pending),
    },
    NatsError::NoWorkers => ErrorResponse {
        code: "NO_WORKERS_AVAILABLE",
        message: "Payment service temporarily unavailable",
        status_code: 503,
        attempt_status: Some(AttemptStatus::Failure),
    },
    NatsError::WorkerError(e) => ErrorResponse {
        code: e.code,
        message: e.message,
        status_code: 400,
        attempt_status: Some(AttemptStatus::Failure),
    },
}
```

## Performance Considerations

### Latency Optimization
- Use connection pooling to avoid handshake overhead
- Implement local caching for frequently accessed data
- Batch operations where possible
- Use binary serialization (MessagePack) for large payloads

### Throughput Optimization
- Configure appropriate JetStream limits
- Use consumer groups for parallel processing
- Implement backpressure handling
- Monitor queue depths and adjust workers

### Resource Optimization
- Lazy load NATS connections
- Share clients across operations
- Use streaming for large responses
- Implement connection lifecycle management