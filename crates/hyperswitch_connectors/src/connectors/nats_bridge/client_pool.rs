// NATS Client helper for connection management
// The NATS Rust client (async-nats) is cheaply cloneable and internally manages connection pooling

use std::time::Duration;

use async_nats::{Client, HeaderMap};
use bytes::Bytes;
use common_utils::errors::CustomResult;
use error_stack::ResultExt;
use futures::StreamExt;
use hyperswitch_interfaces::errors;
use once_cell::sync::Lazy;
use router_env::logger;
use serde::Serialize;
use tokio::sync::RwLock;

/// Global NATS client cache
/// Note: NATS Client is cheaply cloneable - cloning just clones an Arc internally
static NATS_CLIENT_CACHE: Lazy<NatsClientCache> = Lazy::new(NatsClientCache::new);

/// NATS Client Cache - stores one client per URL
/// The client itself can be cloned cheaply and shared across threads
pub struct NatsClientCache {
    clients: RwLock<std::collections::HashMap<String, Client>>,
}

impl NatsClientCache {
    fn new() -> Self {
        Self {
            clients: RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Get global instance
    pub fn global() -> &'static Self {
        &NATS_CLIENT_CACHE
    }

    /// Get or create NATS client for given URL
    /// Returns a clone of the client - cloning is cheap (just an Arc clone internally)
    pub async fn get_client(&self, nats_url: &str) -> CustomResult<Client, errors::ConnectorError> {
        // Check if client exists and is connected
        {
            let clients = self.clients.read().await;
            if let Some(client) = clients.get(nats_url) {
                if client.connection_state() == async_nats::connection::State::Connected {
                    // Clone is cheap - just clones an Arc internally
                    return Ok(client.clone());
                }
                logger::warn!("NATS connection dead, will reconnect: {}", nats_url);
            }
        }

        // Create new client
        let client = async_nats::connect(nats_url)
            .await
            .change_context(errors::ConnectorError::ProcessingStepFailed(Some(
                Bytes::from("Failed to connect to NATS server"),
            )))
            .attach_printable_lazy(|| format!("NATS URL: {}", nats_url))?;

        logger::info!("Connected to NATS: {}", nats_url);

        // Store in cache
        {
            let mut clients = self.clients.write().await;
            clients.insert(nats_url.to_string(), client.clone());
        }

        Ok(client)
    }

    /// Get JetStream context for a client
    /// JetStream provides persistence and exactly-once delivery for financial operations
    pub async fn get_jetstream(
        &self,
        client: &Client,
    ) -> CustomResult<async_nats::jetstream::Context, errors::ConnectorError> {
        Ok(async_nats::jetstream::new(client.clone()))
    }

    /// Create or update JetStream streams for Hyperswitch operations
    /// This should be called during application startup or deployment
    pub async fn setup_jetstream_streams(
        &self,
        jetstream: &async_nats::jetstream::Context,
    ) -> CustomResult<(), errors::ConnectorError> {
        logger::info!("Setting up JetStream streams for NATS bridge");

        // Create HYPERSWITCH_PAYMENTS stream
        let payments_config = async_nats::jetstream::stream::Config {
            name: "HYPERSWITCH_PAYMENTS".to_string(),
            description: Some("Payment operations (authorize, capture, void)".to_string()),
            subjects: vec!["hyperswitch.payments.>".to_string()],
            retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
            max_age: Duration::from_secs(30 * 24 * 60 * 60), // 30 days
            storage: async_nats::jetstream::stream::StorageType::File,
            num_replicas: 1, // TODO: Set to 3 for production HA
            duplicate_window: Duration::from_secs(300), // 5 minutes for idempotency
            ..Default::default()
        };

        match jetstream.get_or_create_stream(payments_config).await {
            Ok(stream) => {
                logger::info!(
                    "JetStream stream created/updated: {} (subjects: {:?})",
                    stream.cached_info().config.name,
                    stream.cached_info().config.subjects
                );
            }
            Err(e) => {
                logger::error!("Failed to create HYPERSWITCH_PAYMENTS stream: {:?}", e);
                return Err(errors::ConnectorError::ProcessingStepFailed(Some(
                    Bytes::from(format!("Failed to create JetStream stream: {}", e)),
                ))
                .into());
            }
        }

        // Create HYPERSWITCH_REFUNDS stream
        let refunds_config = async_nats::jetstream::stream::Config {
            name: "HYPERSWITCH_REFUNDS".to_string(),
            description: Some("Refund operations".to_string()),
            subjects: vec!["hyperswitch.refunds.>".to_string()],
            retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
            max_age: Duration::from_secs(90 * 24 * 60 * 60), // 90 days
            storage: async_nats::jetstream::stream::StorageType::File,
            num_replicas: 1, // TODO: Set to 3 for production HA
            duplicate_window: Duration::from_secs(300), // 5 minutes
            ..Default::default()
        };

        match jetstream.get_or_create_stream(refunds_config).await {
            Ok(stream) => {
                logger::info!(
                    "JetStream stream created/updated: {} (subjects: {:?})",
                    stream.cached_info().config.name,
                    stream.cached_info().config.subjects
                );
            }
            Err(e) => {
                logger::error!("Failed to create HYPERSWITCH_REFUNDS stream: {:?}", e);
                return Err(errors::ConnectorError::ProcessingStepFailed(Some(
                    Bytes::from(format!("Failed to create JetStream stream: {}", e)),
                ))
                .into());
            }
        }

        logger::info!("JetStream streams setup completed successfully");
        Ok(())
    }

    /// Publish message to JetStream with headers (persistent, idempotent)
    /// Uses JetStream publish API instead of core NATS for guaranteed persistence
    pub async fn publish_jetstream<T: Serialize>(
        &self,
        jetstream: &async_nats::jetstream::Context,
        subject: &str,
        headers: HeaderMap,
        payload: &T,
    ) -> CustomResult<(), errors::ConnectorError> {
        let payload_bytes = serde_json::to_vec(payload)
            .change_context(errors::ConnectorError::RequestEncodingFailed)?;

        jetstream
            .publish_with_headers(subject.to_string(), headers, payload_bytes.into())
            .await
            .change_context(errors::ConnectorError::ProcessingStepFailed(Some(
                Bytes::from("Failed to publish to NATS JetStream"),
            )))?
            .await // Wait for acknowledgment from JetStream
            .change_context(errors::ConnectorError::ProcessingStepFailed(Some(
                Bytes::from("JetStream acknowledgment failed"),
            )))?;

        Ok(())
    }

    /// Request-reply pattern (ephemeral) - returns raw Bytes for flexible parsing
    pub async fn request_reply<T: Serialize>(
        &self,
        client: &Client,
        subject: &str,
        headers: HeaderMap,
        payload: &T,
        timeout: Duration,
    ) -> CustomResult<Bytes, errors::ConnectorError> {
        let payload_bytes = serde_json::to_vec(payload)
            .change_context(errors::ConnectorError::RequestEncodingFailed)?;

        let response = tokio::time::timeout(
            timeout,
            client.request_with_headers(subject.to_string(), headers, payload_bytes.into()),
        )
        .await
        .change_context(errors::ConnectorError::ProcessingStepFailed(Some(
            Bytes::from("NATS request timeout"),
        )))?
        .change_context(errors::ConnectorError::ProcessingStepFailed(Some(
            Bytes::from("NATS request failed"),
        )))?;

        // Return raw response payload as Bytes
        Ok(response.payload)
    }

    /// Hybrid JetStream + Request-Reply pattern for financial operations
    /// Publishes to JetStream (persistence + idempotency) and waits for response via inbox
    pub async fn publish_with_response<T: Serialize>(
        &self,
        jetstream: &async_nats::jetstream::Context,
        client: &Client,
        subject: &str,
        mut headers: HeaderMap,
        payload: &T,
        timeout: Duration,
    ) -> CustomResult<Bytes, errors::ConnectorError> {
        let start_time = std::time::Instant::now();

        // 1. Create unique inbox for response
        let inbox = client.new_inbox();
        let mut inbox_sub = client
            .subscribe(inbox.clone())
            .await
            .change_context(errors::ConnectorError::ProcessingStepFailed(Some(
                Bytes::from("Failed to create inbox subscription"),
            )))?;

        // 2. Add reply-to header for worker to respond
        headers.insert("Nats-Reply-To", inbox.as_str());

        // 3. Serialize payload
        let payload_bytes = serde_json::to_vec(payload)
            .change_context(errors::ConnectorError::RequestEncodingFailed)?;

        // 4. Publish to JetStream (with short timeout for JetStream ACK)
        let jetstream_timeout = Duration::from_secs(2);
        let ack_result = tokio::time::timeout(
            jetstream_timeout,
            jetstream.publish_with_headers(
                subject.to_string(),
                headers,
                payload_bytes.into(),
            ),
        )
        .await
        .change_context(errors::ConnectorError::ProcessingStepFailed(Some(
            Bytes::from("JetStream publish timeout"),
        )))?
        .change_context(errors::ConnectorError::ProcessingStepFailed(Some(
            Bytes::from("JetStream publish failed"),
        )))?
        .await;

        let ack = match ack_result {
            Ok(ack) => ack,
            Err(e) => {
                // Metrics: JetStream publish error
                crate::metrics::NATS_JETSTREAM_PUBLISH_ERRORS.add(1, &[]);
                return Err(e).change_context(errors::ConnectorError::ProcessingStepFailed(
                    Some(Bytes::from("JetStream acknowledgment failed")),
                ));
            }
        };

        // Metrics: JetStream ACK success
        crate::metrics::NATS_JETSTREAM_ACK_SUCCESS.add(1, &[]);

        logger::info!(
            "Published to JetStream: stream={} seq={}",
            ack.stream,
            ack.sequence
        );

        // 5. Wait for worker response via inbox (with longer timeout for worker processing)
        let response = tokio::time::timeout(timeout, async {
            inbox_sub
                .next()
                .await
                .ok_or_else(|| {
                    error_stack::report!(errors::ConnectorError::ProcessingStepFailed(Some(
                        Bytes::from("Inbox subscription closed"),
                    )))
                })
        })
        .await;

        // Record metrics based on result
        let duration = start_time.elapsed().as_secs_f64();
        crate::metrics::NATS_REQUEST_DURATION_SECONDS.record(duration, &[]);

        match response {
            Ok(Ok(msg)) => {
                // Metrics: Worker response success
                crate::metrics::NATS_WORKER_RESPONSE_SUCCESS.add(1, &[]);

                logger::info!(
                    "Received response via inbox for JetStream seq={} (took {:.3}s)",
                    ack.sequence,
                    duration
                );

                Ok(msg.payload)
            }
            Err(_) => {
                // Timeout occurred
                crate::metrics::NATS_TIMEOUT_COUNT.add(1, &[]);
                Err(errors::ConnectorError::ProcessingStepFailed(Some(
                    Bytes::from("Worker response timeout"),
                ))
                .into())
            }
            Ok(Err(e)) => {
                // Inbox subscription closed
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Only run with actual NATS server
    async fn test_client_cache() {
        let cache = NatsClientCache::global();
        let client = cache.get_client("nats://localhost:4222").await.unwrap();
        assert_eq!(
            client.connection_state(),
            async_nats::connection::State::Connected
        );

        // Cloning the client is cheap
        let client2 = client.clone();
        assert_eq!(
            client2.connection_state(),
            async_nats::connection::State::Connected
        );
    }
}
