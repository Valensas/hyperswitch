# NATS Bridge Connector - Implementation TODO

## Trait to NATS/JetStream Mapping

### Core Payment Traits → NATS Operations

| Trait/Method | NATS Subject | JetStream Suitable | Description |
|--------------|--------------|-------------------|-------------|
| **PaymentAuthorize** | `hyperswitch.payments.{merchant_id}.authorize` | ✅ Yes - Persistent | Authorizes payment, holds funds. Critical operation requiring durability and replay capability |
| **PaymentCapture** | `hyperswitch.payments.{merchant_id}.capture` | ✅ Yes - Persistent | Captures authorized funds. Needs audit trail and idempotency |
| **PaymentVoid** | `hyperswitch.payments.{merchant_id}.void` | ✅ Yes - Persistent | Cancels authorized payment. Must be durable for reconciliation |
| **PaymentSync** | `hyperswitch.payments.{merchant_id}.sync` | ⚡ No - Request/Reply | Status check operation. Real-time response needed, no persistence required |
| **PaymentSession** | `hyperswitch.payments.{merchant_id}.session` | ⚡ No - Request/Reply | Creates payment session tokens. Ephemeral, real-time operation |
| **PaymentCompleteAuthorize** | `hyperswitch.payments.{merchant_id}.complete_authorize` | ✅ Yes - Persistent | Completes 3DS/2FA authorization. Critical for payment completion |
| **PaymentApprove** | `hyperswitch.payments.{merchant_id}.approve` | ✅ Yes - Persistent | Manual payment approval. Needs audit trail |
| **PaymentReject** | `hyperswitch.payments.{merchant_id}.reject` | ✅ Yes - Persistent | Manual payment rejection. Needs audit trail |
| **PaymentPostSessionTokens** | `hyperswitch.payments.{merchant_id}.post_session_tokens` | ⚡ No - Request/Reply | Updates session tokens. Ephemeral operation |
| **PaymentIncrementalAuthorization** | `hyperswitch.payments.{merchant_id}.incremental_auth` | ✅ Yes - Persistent | Increases authorization amount. Financial operation requiring durability |
| **PaymentUpdateMetadata** | `hyperswitch.payments.{merchant_id}.update_metadata` | ✅ Yes - Persistent | Updates payment metadata. Needs audit trail |

### Refund Traits → NATS Operations

| Trait/Method | NATS Subject | JetStream Suitable | Description |
|--------------|--------------|-------------------|-------------|
| **RefundExecute** | `hyperswitch.refunds.{merchant_id}.execute` | ✅ Yes - Persistent | Initiates refund. Critical financial operation |
| **RefundSync** | `hyperswitch.refunds.{merchant_id}.sync` | ⚡ No - Request/Reply | Checks refund status. Real-time query |
| **RefundUpdate** | `hyperswitch.refunds.{merchant_id}.update` | ✅ Yes - Persistent | Updates refund details. Needs audit trail |

### Mandate/Recurring Traits → NATS Operations

| Trait/Method | NATS Subject | JetStream Suitable | Description |
|--------------|--------------|-------------------|-------------|
| **MandateSetup** | `hyperswitch.mandates.{merchant_id}.setup` | ✅ Yes - Persistent | Sets up recurring payment mandate. Long-term storage needed |
| **MandateRevoke** | `hyperswitch.mandates.{merchant_id}.revoke` | ✅ Yes - Persistent | Cancels recurring mandate. Critical operation |
| **MandateUpdate** | `hyperswitch.mandates.{merchant_id}.update` | ✅ Yes - Persistent | Updates mandate details. Needs versioning |

### Token Management Traits → NATS Operations

| Trait/Method | NATS Subject | JetStream Suitable | Description |
|--------------|--------------|-------------------|-------------|
| **PaymentToken** | `hyperswitch.tokens.{merchant_id}.create` | ✅ Yes - Persistent | Creates payment token. Security-critical, needs audit |
| **TokenValidate** | `hyperswitch.tokens.{merchant_id}.validate` | ⚡ No - Request/Reply | Validates token. Real-time check |
| **TokenDelete** | `hyperswitch.tokens.{merchant_id}.delete` | ✅ Yes - Persistent | Deletes payment token. Compliance requirement |

### Authentication Traits → NATS Operations

| Trait/Method | NATS Subject | JetStream Suitable | Description |
|--------------|--------------|-------------------|-------------|
| **ExternalAuthentication** | `hyperswitch.auth.{merchant_id}.external` | ⚡ No - Request/Reply | 3DS/2FA authentication. Real-time operation |
| **ConnectorAccessToken** | `hyperswitch.auth.{merchant_id}.access_token` | ⚡ No - Request/Reply | Gets connector access token. Short-lived tokens |
| **ConnectorAuthenticationToken** | `hyperswitch.auth.{merchant_id}.auth_token` | ⚡ No - Request/Reply | Gets authentication token. Ephemeral |

### Dispute Management Traits → NATS Operations

| Trait/Method | NATS Subject | JetStream Suitable | Description |
|--------------|--------------|-------------------|-------------|
| **AcceptDispute** | `hyperswitch.disputes.{merchant_id}.accept` | ✅ Yes - Persistent | Accepts dispute. Legal/compliance requirement |
| **DefendDispute** | `hyperswitch.disputes.{merchant_id}.defend` | ✅ Yes - Persistent | Submits dispute evidence. Needs document trail |
| **SubmitEvidence** | `hyperswitch.disputes.{merchant_id}.evidence` | ✅ Yes - Persistent | Uploads dispute evidence. Document storage |

### Webhook Traits → NATS Operations

| Trait/Method | NATS Subject | JetStream Suitable | Description |
|--------------|--------------|-------------------|-------------|
| **IncomingWebhook** | `hyperswitch.webhooks.{merchant_id}.{connector}.incoming` | ✅ Yes - Persistent | Processes incoming webhooks. Must not lose events |
| **VerifyWebhookSource** | `hyperswitch.webhooks.{merchant_id}.verify` | ⚡ No - Request/Reply | Verifies webhook signature. Real-time validation |

### Payout Traits → NATS Operations (if enabled)

| Trait/Method | NATS Subject | JetStream Suitable | Description |
|--------------|--------------|-------------------|-------------|
| **PayoutCreate** | `hyperswitch.payouts.{merchant_id}.create` | ✅ Yes - Persistent | Creates payout. Financial operation |
| **PayoutFulfill** | `hyperswitch.payouts.{merchant_id}.fulfill` | ✅ Yes - Persistent | Executes payout. Critical financial operation |
| **PayoutCancel** | `hyperswitch.payouts.{merchant_id}.cancel` | ✅ Yes - Persistent | Cancels payout. Needs audit trail |
| **PayoutSync** | `hyperswitch.payouts.{merchant_id}.sync` | ⚡ No - Request/Reply | Checks payout status. Real-time query |

### Special Operations → NATS Operations

| Trait/Method | NATS Subject | JetStream Suitable | Description |
|--------------|--------------|-------------------|-------------|
| **TaxCalculation** | `hyperswitch.tax.{merchant_id}.calculate` | ⚡ No - Request/Reply | Calculates tax. Real-time calculation |
| **FraudCheck** | `hyperswitch.fraud.{merchant_id}.check` | ✅ Yes - Analytics | Fraud detection. Needs analysis/ML pipeline |
| **GiftCardBalanceCheck** | `hyperswitch.giftcard.{merchant_id}.balance` | ⚡ No - Request/Reply | Checks gift card balance. Real-time query |

### JetStream Decision Criteria

**✅ Use JetStream for:**
- Financial operations (authorize, capture, refund)
- Operations requiring audit trail
- Operations needing idempotency
- Webhook processing
- Compliance-related operations
- Operations that might need replay

**⚡ Use Request/Reply for:**
- Status checks and queries
- Real-time validations
- Session/token generation
- Balance checks
- Tax calculations
- Operations that don't modify state

### Stream Configuration Recommendations

```yaml
PAYMENT_STREAM:
  subjects:
    - "hyperswitch.payments.>"
  retention: 30 days
  replicas: 3
  storage: file

REFUND_STREAM:
  subjects:
    - "hyperswitch.refunds.>"
  retention: 90 days
  replicas: 3
  storage: file

WEBHOOK_STREAM:
  subjects:
    - "hyperswitch.webhooks.>"
  retention: 7 days
  replicas: 2
  storage: file

DISPUTE_STREAM:
  subjects:
    - "hyperswitch.disputes.>"
  retention: 365 days  # Legal requirement
  replicas: 3
  storage: file
```

## Phase 0: Pre-Implementation Requirements

### Environment Setup
- [ ] Install NATS server locally (v2.10+)
- [ ] Enable JetStream: `nats-server -js`
- [ ] Install NATS CLI tools: `brew install nats-io/nats-tools/nats`
- [ ] Verify JetStream: `nats stream ls`

### Development Tools
- [ ] Install cargo-watch for auto-reload: `cargo install cargo-watch`
- [ ] Setup NATS monitoring dashboard
- [ ] Configure VS Code with Rust analyzer
- [ ] Install NATS protocol analyzer

### Local NATS Configuration
- [ ] Create local NATS config file with JetStream enabled
- [ ] Configure authentication (user/password or JWT)
- [ ] Set up TLS certificates for development
- [ ] Configure stream and consumer defaults

## Phase 1: Initial Setup and Scaffold Generation

### 1.1 Generate Connector Base
- [ ] Run `sh scripts/add_connector.sh nats_bridge nats://localhost:4222`
- [ ] Verify all files are generated correctly
- [ ] Check that enums are added in all required places
- [ ] Ensure routing configurations are updated

### 1.2 Add Dependencies
- [ ] Add `async-nats = "0.33"` to `crates/hyperswitch_connectors/Cargo.toml`
- [ ] Add `serde_json = "1.0"` for message serialization
- [ ] Add `uuid` for correlation ID generation
- [ ] Add `chrono` for timestamps

### 1.3 Configuration Setup
- [ ] Add NATS configuration to `config/development.toml`
- [ ] Add NATS configuration to `config/docker_compose.toml`
- [ ] Add NATS configuration to `config/config.example.toml`
- [ ] Add JetStream configuration parameters

## Phase 2: Core Implementation

### 2.1 NATS Client Integration
- [ ] Create NATS client initialization in `mod.rs`
- [ ] Implement connection pooling
- [ ] Add connection health checks
- [ ] Implement graceful shutdown

### 2.2 Type Definitions
- [ ] Define `NatsPaymentRequest` structure in `types.rs`
- [ ] Define `NatsPaymentResponse` structure
- [ ] Define `PaymentOperation` enum
- [ ] Create error types for NATS operations

### 2.3 Transform RouterData to NATS Messages
- [ ] Implement serialization in `transformers.rs`
- [ ] Handle all payment method types
- [ ] Preserve all necessary metadata
- [ ] Add correlation ID generation

## Phase 3: Implement Payment Operations

### 3.1 Payment Authorization
- [ ] Implement `PaymentAuthorize` trait for NATS
- [ ] Replace HTTP logic with NATS publish
- [ ] Add request-reply pattern
- [ ] Handle timeout scenarios

### 3.2 Payment Capture
- [ ] Implement `PaymentCapture` trait
- [ ] Create capture message format
- [ ] Handle partial captures
- [ ] Implement idempotency

### 3.3 Payment Void/Cancel
- [ ] Implement `PaymentVoid` trait
- [ ] Create cancellation message format
- [ ] Handle post-capture cancellations
- [ ] Implement status tracking

### 3.4 Payment Sync
- [ ] Implement `PaymentSync` trait
- [ ] Create sync message format
- [ ] Handle status queries
- [ ] Implement caching strategy

### 3.5 Refund Operations
- [ ] Implement `RefundExecute` trait
- [ ] Implement `RefundSync` trait
- [ ] Create refund message format
- [ ] Handle partial refunds

### 3.6 Mandate Setup
- [ ] Implement `MandateSetup` trait
- [ ] Create mandate message format
- [ ] Handle recurring payment setup
- [ ] Implement mandate storage

### 3.7 Session Management
- [ ] Implement `PaymentSession` trait
- [ ] Handle session token generation
- [ ] Implement session validation
- [ ] Add session expiry handling

### 3.8 Tokenization
- [ ] Implement `PaymentToken` trait
- [ ] Create tokenization message format
- [ ] Handle token storage
- [ ] Implement token validation

## Phase 4: JetStream Integration

### 4.1 Stream Configuration
- [ ] Create PAYMENTS stream
- [ ] Configure retention policies
- [ ] Set up deduplication window
- [ ] Configure replication

### 4.2 Consumer Setup
- [ ] Create durable consumers for each operation
- [ ] Configure acknowledgment policies
- [ ] Set up delivery policies
- [ ] Implement consumer groups

### 4.3 Message Persistence
- [ ] Implement message storage
- [ ] Add message replay capability
- [ ] Create audit trail
- [ ] Implement data retention

## Phase 5: Error Handling & Reliability

### 5.1 Error Handling
- [ ] Implement comprehensive error mapping
- [ ] Add retry logic with exponential backoff
- [ ] Create dead letter queue handling
- [ ] Implement circuit breaker pattern

### 5.2 Timeout Management
- [ ] Configure operation-specific timeouts
- [ ] Implement timeout error responses
- [ ] Add timeout metrics
- [ ] Create timeout recovery

### 5.3 Idempotency
- [ ] Implement idempotency keys
- [ ] Add duplicate detection
- [ ] Create idempotency storage
- [ ] Handle replay scenarios

## Phase 6: Worker Service Development

### 6.1 Sample Worker Implementation
- [ ] Create Python sample worker
- [ ] Create Go sample worker
- [ ] Create Node.js sample worker
- [ ] Document worker interface

### 6.2 Worker Templates
- [ ] Create worker template for Stripe
- [ ] Create worker template for Adyen
- [ ] Create worker template for PayPal
- [ ] Create generic worker template

### 6.3 Worker Management
- [ ] Implement worker health checks
- [ ] Add worker registration
- [ ] Create worker discovery
- [ ] Implement load balancing

## Phase 7: Testing

### 7.1 Unit Tests
- [ ] Write tests for message serialization
- [ ] Write tests for error handling
- [ ] Write tests for timeout scenarios
- [ ] Write tests for retry logic

### 7.2 Integration Tests
- [ ] Create NATS test container setup
- [ ] Write end-to-end payment flow tests
- [ ] Test failover scenarios
- [ ] Test message replay

### 7.3 Load Testing
- [ ] Create load test scenarios
- [ ] Test concurrent operations
- [ ] Measure throughput limits
- [ ] Test backpressure handling

### 7.4 Connector Tests
- [ ] Update `crates/router/tests/connectors/nats_bridge.rs`
- [ ] Add test credentials to `sample_auth.toml`
- [ ] Create mock NATS server for tests
- [ ] Implement test worker

### 7.5 Mock NATS Server for Tests
- [ ] Use `testcontainers` with NATS image
- [ ] Create deterministic test subjects
- [ ] Implement mock worker responses
- [ ] Add latency injection for timeout tests

### 7.6 Test Scenarios
- [ ] Happy path: Successful payment authorization
- [ ] Timeout: Worker doesn't respond in time
- [ ] Network failure: NATS connection drops
- [ ] Worker error: Worker returns error response
- [ ] Duplicate: Same payment sent twice
- [ ] Replay: Message replay from JetStream
- [ ] Idempotency: Multiple identical requests
- [ ] Circuit breaker: Multiple consecutive failures

## Phase 8: Monitoring & Observability

### 8.1 Metrics
- [ ] Add message processing metrics
- [ ] Implement latency tracking
- [ ] Add success/failure rates
- [ ] Create dashboard templates

### 8.2 Logging
- [ ] Implement structured logging
- [ ] Add correlation ID tracking
- [ ] Create log aggregation
- [ ] Implement log analysis

### 8.3 Tracing
- [ ] Implement distributed tracing
- [ ] Add trace propagation
- [ ] Create trace visualization
- [ ] Implement trace analysis

## Phase 9: Documentation

### 9.1 API Documentation
- [ ] Document NATS message formats
- [ ] Create worker API specification
- [ ] Document configuration options
- [ ] Create troubleshooting guide

### 9.2 Implementation Guide
- [ ] Write worker development guide
- [ ] Create deployment guide
- [ ] Document best practices
- [ ] Create migration guide

### 9.3 Examples
- [ ] Create example configurations
- [ ] Provide worker examples
- [ ] Create integration examples
- [ ] Document common patterns

## Phase 10: Security

### 10.1 Transport Security
- [ ] Implement TLS for NATS connections
- [ ] Add certificate management
- [ ] Implement mutual TLS
- [ ] Create security policies

### 10.2 Message Security
- [ ] Implement message encryption
- [ ] Add message signing
- [ ] Create key management
- [ ] Implement access control

### 10.3 Audit & Compliance
- [ ] Implement audit logging
- [ ] Add compliance checks
- [ ] Create audit reports
- [ ] Implement data retention

## Phase 11: Production Readiness

### 11.1 Performance Optimization
- [ ] Optimize message serialization
- [ ] Implement connection pooling
- [ ] Add caching layer
- [ ] Optimize memory usage

### 11.2 High Availability
- [ ] Implement failover mechanisms
- [ ] Add health check endpoints
- [ ] Create disaster recovery plan
- [ ] Implement backup strategies

### 11.3 Deployment
- [ ] Create Docker image
- [ ] Write Kubernetes manifests
- [ ] Create Helm charts
- [ ] Document deployment process

## Phase 12: Advanced Features

### 12.1 Dynamic Routing
- [ ] Implement routing rules engine
- [ ] Add A/B testing capability
- [ ] Create fallback mechanisms
- [ ] Implement load distribution

### 12.2 Multi-Region Support
- [ ] Implement geo-routing
- [ ] Add regional failover
- [ ] Create data replication
- [ ] Implement latency optimization

### 12.3 Rate Limiting
- [ ] Implement rate limiting
- [ ] Add quota management
- [ ] Create throttling mechanisms
- [ ] Implement backpressure

## Performance Targets

### Latency Requirements
- Authorization: < 2 seconds p99
- Capture: < 1 second p99
- Sync: < 500ms p99
- Refund: < 3 seconds p99
- Session creation: < 1 second p99

### Throughput Targets
- 1000 requests/second per instance
- 10,000 concurrent operations
- 100,000 messages in JetStream
- 50 workers per operation type

### Resource Usage
- Memory: < 500MB per Hyperswitch instance
- CPU: < 2 cores at peak load
- Network: < 100Mbps bandwidth
- Connections: < 100 NATS connections

## Migration from Direct Connectors

### Phase 1: Shadow Mode
- [ ] Run NATS bridge in parallel with direct connectors
- [ ] Compare responses for accuracy
- [ ] Measure performance differences
- [ ] Log discrepancies for analysis

### Phase 2: Canary Deployment
- [ ] Route 1% traffic to NATS bridge
- [ ] Monitor error rates and latency
- [ ] Gradually increase to 10%, 50%, 100%
- [ ] Maintain rollback capability

### Phase 3: Full Migration
- [ ] Switch all traffic to NATS bridge
- [ ] Deprecate direct connector code
- [ ] Document lessons learned
- [ ] Update monitoring dashboards

## Debugging Guide

### Common Issues and Solutions

1. **Connection refused**
   - Check: NATS server is running (`ps aux | grep nats-server`)
   - Solution: Start NATS with `nats-server -js`

2. **Timeout errors**
   - Check: Worker health (`nats consumer info PAYMENTS`)
   - Solution: Increase timeout or scale workers

3. **Subject not found**
   - Check: Worker subscriptions (`nats sub "hyperswitch.>" --count 1`)
   - Solution: Verify worker is subscribed to correct subject

4. **Duplicate messages**
   - Check: JetStream dedup window (`nats stream info PAYMENTS`)
   - Solution: Increase dedup window or add idempotency key

5. **Authentication failures**
   - Check: NATS credentials in config
   - Solution: Update credentials or disable auth for dev

### Debug Commands
```bash
# Monitor all payment messages
nats sub "hyperswitch.payments.>"

# Check stream status
nats stream info PAYMENTS

# View consumer status
nats consumer info PAYMENTS hyperswitch-consumer

# Publish test message
echo '{"test": "data"}' | nats pub hyperswitch.payments.test.authorize

# Check message in stream
nats stream get PAYMENTS 1

# Monitor specific merchant
nats sub "hyperswitch.payments.merchant_123.>"

# Check worker subscriptions
nats server report connections

# View stream statistics
nats stream report
```

### Troubleshooting Checklist
- [ ] NATS server running and accessible
- [ ] JetStream enabled and configured
- [ ] Workers subscribed to correct subjects
- [ ] Network connectivity between services
- [ ] Proper authentication credentials
- [ ] Sufficient resources (memory/CPU)
- [ ] Correct message format
- [ ] Timeout values appropriate
- [ ] Error handling implemented
- [ ] Monitoring dashboards working

## Validation Checklist

Before marking the connector as production-ready:

- [ ] All payment operations work end-to-end
- [ ] Error handling is comprehensive
- [ ] Timeouts are properly configured
- [ ] Retries work as expected
- [ ] JetStream persistence is verified
- [ ] Workers can process all message types
- [ ] Security measures are in place
- [ ] Monitoring is operational
- [ ] Documentation is complete
- [ ] Load testing passes requirements
- [ ] Integration tests are green
- [ ] Code review is complete
- [ ] Performance benchmarks are met
- [ ] Deployment automation works
- [ ] Rollback procedures are tested

## Notes

- Start with Phase 1-3 for MVP
- Phases 4-6 for production readiness
- Phases 7-9 for quality assurance
- Phases 10-12 for enterprise features

## Dependencies

- Requires NATS server 2.10+ with JetStream enabled
- Workers can be developed in parallel
- Configuration changes need coordination with DevOps
- Security review needed before production deployment

## Timeline Estimate

- Phase 1-3: 1 week
- Phase 4-6: 2 weeks
- Phase 7-9: 1 week
- Phase 10-12: 2 weeks

Total estimated time: 6 weeks for full implementation