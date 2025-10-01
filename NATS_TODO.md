# NATS Bridge Connector - Production Readiness & Implementation TODO

## 🔴 CRITICAL: Production Readiness Assessment (2025-01-XX)

### **Status: ❌ NOT PRODUCTION READY**

After comprehensive review with HyperArchitect, NatsArchitect, and DomainArchitect, plus deep research into NATS best practices from:
- https://natsbyexample.com
- https://docs.nats.io
- https://github.com/nats-io/nats.rs

**Critical Issue Identified:** Using ephemeral Core NATS request-reply for financial operations.

### **The Problem**

Current implementation uses **Core NATS request-reply** (at-most-once delivery) for ALL operations including:
- ❌ Payment authorization
- ❌ Payment capture
- ❌ Refunds

This violates NATS best practices for financial operations:
- ❌ No persistence (worker crash = lost payment)
- ❌ No idempotency enforcement (Nats-Msg-Id header ignored without JetStream)
- ❌ No audit trail (messages disappear after processing)
- ❌ No message replay (cannot recover from failures)

### **Required Fix: Hybrid JetStream + Request-Reply Pattern**

**Financial Operations** → JetStream publish + inbox for response
**Query Operations** → Core NATS request-reply (current pattern is fine)

---

## 📋 Phase 1: Critical Fixes (REQUIRED FOR PRODUCTION)

**Timeline:** 5-7 days  
**Status:** Starting now

### 1.1 JetStream Infrastructure
- [x] Add JetStream stream creation for HYPERSWITCH_PAYMENTS
- [x] Configure deduplication window (5 minutes)
- [x] Set up persistence (file-based storage)
- [x] Configure replicas (3 for HA)

```rust
// Stream configuration needed
let stream_config = jetstream::stream::Config {
    name: "HYPERSWITCH_PAYMENTS".to_string(),
    subjects: vec!["hyperswitch.payments.>".to_string()],
    duplicate_window: Duration::from_secs(300), // 5 minutes
    retention: RetentionPolicy::Limits,
    max_age: Duration::from_secs(30 * 24 * 60 * 60), // 30 days
    storage: StorageType::File,
    num_replicas: 3,
    ..Default::default()
};
```

### 1.2 Hybrid Request-Reply Implementation
- [x] Create `publish_with_response()` method in client_pool.rs
- [x] Implement JetStream publish + inbox pattern
- [x] Add 2-phase timeout (2s for JetStream ACK, 28s for worker response)
- [x] Update authorize() to use hybrid pattern
- [x] Update capture() to use hybrid pattern
- [x] Update refund execute() to use hybrid pattern

**Key Implementation:**
```rust
pub async fn publish_with_response<T: Serialize>(
    &self,
    jetstream: &jetstream::Context,
    client: &Client,
    subject: &str,
    headers: HeaderMap,
    payload: &T,
    timeout: Duration,
) -> CustomResult<Bytes, errors::ConnectorError> {
    // 1. Create inbox for response
    let inbox = client.new_inbox();
    let mut sub = client.subscribe(inbox.clone()).await?;
    
    // 2. Add reply-to header
    headers.insert("Nats-Reply-To", inbox.as_str());
    
    // 3. Publish to JetStream (persistence + idempotency)
    let ack = tokio::time::timeout(
        Duration::from_secs(2),
        jetstream.publish_with_headers(subject, headers, payload)
    ).await??.await?;
    
    // 4. Wait for worker response
    let response = tokio::time::timeout(timeout, sub.next()).await??;
    Ok(response.payload)
}
```

### 1.3 Worker ACK Pattern Documentation
- [ ] Document explicit ACK requirement for workers
- [ ] Create Python worker example with ACK
- [ ] Create Go worker example with ACK
- [ ] Update NATS.md with ACK patterns

**Worker Pattern:**
```python
async def handle_authorize(msg):
    # Process payment
    result = await process_stripe_payment(msg.data)
    
    # Send response to reply-to inbox
    reply_to = msg.headers.get('Nats-Reply-To')
    if reply_to:
        await msg._client.publish(reply_to, json.dumps(result))
    
    # CRITICAL: ACK after everything succeeded
    await msg.ack()
```

### 1.4 Timeout Recovery Job

**Purpose:** Automatically mark payments as "Failed" when NATS workers don't respond within the timeout window. This prevents payments from being stuck in limbo indefinitely.

**Implementation Requirements:**

- [ ] Create background scheduler job in `crates/scheduler/src/workflows/`
- [ ] Scan for payments stuck in transitional states
- [ ] Mark stuck payments (>5 minutes for 30s timeout operations) as "Failed"
- [ ] Add metrics for recovery job execution
- [ ] Run job every 1 minute

**Affected Payment States:**
- `Started` - Payment initiated but no response
- `Authorizing` - Authorize sent to worker, no response
- `Pending` - Waiting for worker action

**Timeout Thresholds:**
- **Normal operations** (authorize, capture, refund): 30s timeout → mark failed after **5 minutes**
- **Sync operations**: 5s timeout → mark failed after **2 minutes**

**Implementation Sketch:**

```rust
// File: crates/scheduler/src/workflows/nats_payment_recovery.rs

use common_utils::errors::CustomResult;
use diesel_models::{enums as storage_enums, PaymentAttempt};
use error_stack::ResultExt;
use router_env::logger;
use storage_impl::StorageInterface;

/// Recovery job for NATS bridge payments stuck in transitional states
pub async fn cleanup_stuck_nats_payments(
    db: &dyn StorageInterface,
) -> CustomResult<(), errors::SchedulerError> {
    logger::info!("Starting NATS payment recovery job");

    // Query payments stuck in transitional states for nats_bridge connector
    let stuck_payments = db
        .find_payment_attempts_by_connector_and_statuses(
            "nats_bridge",
            vec![
                storage_enums::AttemptStatus::Started,
                storage_enums::AttemptStatus::Authorizing,
                storage_enums::AttemptStatus::Pending,
            ],
            chrono::Duration::minutes(5), // Older than 5 minutes
        )
        .await
        .change_context(errors::SchedulerError::DatabaseError)?;

    logger::info!(
        "Found {} stuck NATS payments to recover",
        stuck_payments.len()
    );

    let mut recovered_count = 0;
    let mut failed_count = 0;

    for payment in stuck_payments {
        match mark_payment_as_timeout_failed(db, payment).await {
            Ok(_) => {
                recovered_count += 1;
            }
            Err(e) => {
                failed_count += 1;
                logger::error!(
                    payment_id = ?payment.payment_id,
                    attempt_id = ?payment.attempt_id,
                    "Failed to recover stuck payment: {:?}",
                    e
                );
            }
        }
    }

    logger::info!(
        "NATS recovery job completed: recovered={}, failed={}",
        recovered_count,
        failed_count
    );

    // Emit metrics
    metrics::NATS_RECOVERY_JOB_RECOVERED_COUNT.add(recovered_count, &[]);
    metrics::NATS_RECOVERY_JOB_FAILED_COUNT.add(failed_count, &[]);

    Ok(())
}

async fn mark_payment_as_timeout_failed(
    db: &dyn StorageInterface,
    payment: PaymentAttempt,
) -> CustomResult<(), errors::SchedulerError> {
    let update = storage_models::PaymentAttemptUpdate::ErrorUpdate {
        connector: Some(payment.connector.clone()),
        status: storage_enums::AttemptStatus::Failure,
        error_code: Some("CONNECTOR_TIMEOUT".to_string()),
        error_message: Some("NATS worker timeout - no response received within threshold".to_string()),
        error_reason: Some("Worker did not respond within 30 seconds, marked as failed by recovery job".to_string()),
        amount_capturable: None,
        updated_by: "nats_recovery_job".to_string(),
    };

    db.update_payment_attempt(payment, update)
        .await
        .change_context(errors::SchedulerError::DatabaseError)?;

    logger::warn!(
        payment_id = ?payment.payment_id,
        attempt_id = ?payment.attempt_id,
        "Marked stuck NATS payment as failed"
    );

    Ok(())
}
```

**Scheduler Configuration:**

Add to `crates/scheduler/src/configs.rs`:
```rust
pub const NATS_RECOVERY_JOB_INTERVAL_SECS: u64 = 60; // Run every 1 minute
```

**Additional Metrics Needed:**

Add to `crates/hyperswitch_connectors/src/metrics.rs`:
```rust
counter_metric!(NATS_RECOVERY_JOB_RECOVERED_COUNT, GLOBAL_METER);
counter_metric!(NATS_RECOVERY_JOB_FAILED_COUNT, GLOBAL_METER);
```

**Testing Strategy:**
1. Create test payment with nats_bridge connector
2. Force it into "Authorizing" state
3. Wait 5+ minutes
4. Verify recovery job marks it as "Failed"
5. Check error message includes "CONNECTOR_TIMEOUT"

**Monitoring & Alerts:**
- Alert if `NATS_RECOVERY_JOB_RECOVERED_COUNT` > 10/hour (indicates worker issues)
- Alert if `NATS_RECOVERY_JOB_FAILED_COUNT` > 0 (indicates database issues)
- Dashboard: Track recovery job execution time and recovered payment count over time

### 1.5 CompleteAuthorize (3DS) Implementation
- [x] Implement ConnectorIntegration for CompleteAuthorize
- [x] Use hybrid JetStream pattern (needs synchronous redirect URL)
- [x] Subject: hyperswitch.payments.complete_authorize
- [x] Timeout: 30 seconds
- [ ] Test with 3DS redirect flow

### 1.6 Basic Observability
- [x] Add metrics for NATS operations
  - `nats_request_duration_seconds` (histogram with p50, p95, p99)
  - `nats_timeout_count` (counter)
  - `nats_jetstream_publish_errors` (counter)
  - `nats_worker_response_success` (counter)
  - `nats_jetstream_ack_success` (counter)
- [x] Add structured logging with correlation IDs
- [x] Log JetStream ACK sequence numbers
- [ ] Alert on high timeout rates (Grafana/PagerDuty configuration)

---

## 📋 Phase 2: Production Hardening (RECOMMENDED FOR PRODUCTION)

**Timeline:** 3-5 days after Phase 1  
**Status:** Pending Phase 1 completion

### 2.1 Simplified Connection Management
- [x] Remove HashMap-based client cache
- [x] Implement single client with Lazy initialization
- [x] Remove manual connection state checks
- [x] Trust async-nats built-in reconnection
- [ ] Add connection state monitoring (metrics only) - OPTIONAL

**Simplified Pattern:**
```rust
static NATS_CLIENT: Lazy<RwLock<Option<Client>>> = Lazy::new(|| RwLock::new(None));

pub async fn get_client(nats_url: &str) -> Result<Client> {
    // Fast path: return existing client
    if let Some(client) = NATS_CLIENT.read().await.as_ref() {
        return Ok(client.clone()); // Cheap Arc clone
    }
    
    // Slow path: initialize once
    let mut guard = NATS_CLIENT.write().await;
    if guard.is_none() {
        let client = async_nats::connect_with_options(nats_url, options).await?;
        *guard = Some(client.clone());
    }
    Ok(guard.as_ref().unwrap().clone())
}
```

### 2.2 Circuit Breaker
- [ ] Implement circuit breaker for NATS operations
- [ ] Track consecutive failures
- [ ] Open circuit after 5 failures
- [ ] Half-open state with probe requests
- [ ] Add circuit breaker metrics

### 2.3 Void Operation
- [ ] Implement ConnectorIntegration for Void
- [ ] Subject: hyperswitch.payments.void
- [ ] Use hybrid JetStream pattern
- [ ] Timeout: 30 seconds

### 2.4 Connection Health Monitoring
- [ ] Spawn background task to monitor connection state
- [ ] Emit metrics on state transitions
- [ ] Log warnings on reconnection attempts
- [ ] Alert on prolonged disconnections

### 2.5 Load Testing
- [ ] Create load test scenarios (JMeter or k6)
- [ ] Test 1000 req/s throughput
- [ ] Measure p95/p99 latencies
- [ ] Test worker failure scenarios
- [ ] Test NATS server restart recovery

---

## 📋 Phase 3: Advanced Features (OPTIONAL)

**Timeline:** 5-7 days  
**Status:** Post-production

### 3.1 Webhook Receiver
- [ ] Implement IncomingWebhook trait
- [ ] Subscribe to hyperswitch.webhooks.incoming
- [ ] Parse worker async updates
- [ ] Update payment status from webhooks

### 3.2 Dead Letter Queue (DLQ)
- [ ] Create HYPERSWITCH_PAYMENTS_DLQ stream
- [ ] Configure max delivery attempts (5)
- [ ] Route failed messages to DLQ
- [ ] Add DLQ monitoring dashboard
- [ ] Create DLQ replay tool

### 3.3 Message Replay UI
- [ ] Build admin UI for JetStream message replay
- [ ] Filter messages by merchant_id, payment_id
- [ ] Replay failed operations
- [ ] Track replay history

### 3.4 Advanced Monitoring
- [ ] Create Grafana dashboards
- [ ] Add alerting rules (PagerDuty/Slack)
- [ ] Implement distributed tracing (OpenTelemetry)
- [ ] Add log aggregation (ELK stack)

---

## Protocol Summary

**Architecture**: merchant_id in BOTH headers AND body

```
Subject:  hyperswitch.payments.authorize  (NO merchant_id)
Headers:  X-Merchant-Id: merchant_123      (for NATS observability)
          Nats-Msg-Id: pay_123_attempt_456 (for JetStream deduplication)
Body:     { "merchant_id": "merchant_123", ... }  (authoritative)
```

---

## Trait to NATS/JetStream Mapping

### Core Payment Traits → NATS Operations

| Trait/Method | NATS Subject | Pattern | Timeout | Priority |
|--------------|--------------|---------|---------|----------|
| **PaymentAuthorize** | `hyperswitch.payments.authorize` | JetStream + inbox | 30s | Phase 1 ✅ |
| **PaymentCapture** | `hyperswitch.payments.capture` | JetStream + inbox | 30s | Phase 1 ✅ |
| **PaymentVoid** | `hyperswitch.payments.void` | JetStream + inbox | 30s | Phase 2 |
| **PaymentSync** | `hyperswitch.payments.sync` | Request/Reply | 5s | ✅ Done |
| **PaymentSession** | `hyperswitch.payments.session` | Request/Reply | 5s | Future |
| **PaymentCompleteAuthorize** | `hyperswitch.payments.complete_authorize` | JetStream + inbox | 30s | Phase 1 ✅ |
| **RefundExecute** | `hyperswitch.refunds.execute` | JetStream + inbox | 30s | Phase 1 ✅ |
| **RefundSync** | `hyperswitch.refunds.sync` | Request/Reply | 5s | ✅ Done |

### JetStream Decision Criteria

**✅ Use JetStream + Inbox for:**
- Financial operations (authorize, capture, refund)
- Operations requiring audit trail
- Operations needing idempotency
- Compliance-related operations

**⚡ Use Core NATS Request/Reply for:**
- Status checks and queries (sync, rsync)
- Real-time validations
- Session/token generation
- Operations that don't modify state

---

## Stream Configuration

```yaml
HYPERSWITCH_PAYMENTS:
  subjects:
    - "hyperswitch.payments.>"
  retention: limits
  max_age: 30d
  storage: file
  replicas: 3
  duplicate_window: 5m  # Nats-Msg-Id deduplication

HYPERSWITCH_REFUNDS:
  subjects:
    - "hyperswitch.refunds.>"
  retention: limits
  max_age: 90d
  storage: file
  replicas: 3
  duplicate_window: 5m

HYPERSWITCH_PAYMENTS_DLQ:
  subjects:
    - "hyperswitch.dlq.payments.>"
  retention: limits
  max_age: 7d
  storage: file
  replicas: 2
```

---

## Implementation Notes from NatsArchitect Review

### Key Findings

1. **✅ Correct Patterns:**
   - Client cloning (Arc-wrapped, cheap)
   - Timeout handling with tokio::time::timeout
   - block_in_place usage for sync-to-async bridge
   - Protocol compliance (headers + body)

2. **❌ Critical Issues:**
   - Using Core NATS for financial ops (should use JetStream)
   - Nats-Msg-Id header set but not enforced
   - No message persistence or replay
   - No worker acknowledgments

3. **⚠️ Needs Improvement:**
   - Connection cache has memory leak risk
   - Manual reconnection logic (trust auto-reconnect)
   - Missing observability (metrics, tracing)
   - No circuit breaker

### Recommended Pattern (from NatsArchitect)

```rust
// Hybrid JetStream + Request-Reply
async fn authorize_hybrid(req: &PaymentRequest) -> Result<PaymentResponse> {
    // 1. Publish to JetStream (persistence + idempotency)
    let ack = jetstream.publish_with_headers(subject, headers, payload).await?.await?;
    
    // 2. Wait for response via inbox (for 3DS synchronous response)
    let inbox = client.new_inbox();
    let response = timeout(30s, inbox_sub.next()).await?;
    
    // Worker:
    // - Pulls from JetStream stream
    // - Processes payment
    // - Sends response to reply-to inbox
    // - ACKs message after success
    
    Ok(response)
}
```

---

## Validation Checklist

### Phase 1 Completion Criteria
- [ ] JetStream stream HYPERSWITCH_PAYMENTS created
- [ ] authorize() using hybrid pattern
- [ ] capture() using hybrid pattern
- [ ] refund execute() using hybrid pattern
- [ ] Timeout recovery job running
- [ ] CompleteAuthorize (3DS) implemented
- [ ] Basic metrics emitting
- [ ] Worker examples documented
- [ ] Integration tests passing

### Phase 2 Completion Criteria  
- [ ] Connection management simplified
- [ ] Circuit breaker implemented
- [ ] Void operation working
- [ ] Load test passing (1000 req/s)
- [ ] p99 latency < 2s for authorize

### Production Readiness Criteria
- [ ] All Phase 1 items complete
- [ ] All Phase 2 items complete
- [ ] Zero stuck payments in 24h test
- [ ] Zero duplicate charges in load test
- [ ] Monitoring dashboards operational
- [ ] Runbook documented
- [ ] Security review passed

---

## Timeline

- **Phase 1 (Critical):** 5-7 days ← **START HERE**
- **Phase 2 (Hardening):** 3-5 days
- **Phase 3 (Advanced):** 5-7 days (optional)

**Total to production:** 2-3 weeks

---

## Related Documentation

- **[NATS.md](./NATS.md)** - Complete protocol guide with worker examples
- **[NATS Architecture Review](./docs/nats-architecture-review.md)** - Full architect assessment
- **[natsbyexample.com](https://natsbyexample.com)** - NATS patterns and examples
- **[docs.nats.io](https://docs.nats.io)** - Official NATS documentation

---

## Notes

- **Current implementation compiles and runs** but uses wrong pattern for financial ops
- **Not a rewrite** - infrastructure is solid, just needs pattern upgrade
- **Architects confirmed** core patterns (async safety, client cloning, timeouts) are correct
- **Main blocker** - Must use JetStream for persistence and idempotency
- **Estimated effort** - 2-3 weeks to full production readiness
