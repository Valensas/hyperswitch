// NATS Client helper for connection management
// The NATS Rust client (async-nats) is cheaply cloneable and internally manages connection pooling

use std::time::Duration;

use async_nats::{Client, HeaderMap};
use bytes::Bytes;
use common_utils::errors::CustomResult;
use error_stack::ResultExt;
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
