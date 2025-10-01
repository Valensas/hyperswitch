pub mod client_pool;
pub mod transformers;

use async_nats::HeaderMap;
use common_enums::enums;
use common_utils::{
    errors::CustomResult,
    ext_traits::BytesExt,
    request::Request,
    types::{AmountConvertor, StringMinorUnit, StringMinorUnitForConnector},
};
use crate::utils::RefundsRequestData;
use error_stack::{report, ResultExt};
use hyperswitch_domain_models::{
    router_data::{AccessToken, ConnectorAuthType, ErrorResponse, RouterData},
    router_flow_types::{
        payments::{Authorize, Capture, CompleteAuthorize, PSync, PaymentMethodToken, Session, SetupMandate, Void},
        refunds::{Execute, RSync},
        AccessTokenAuth,
    },
    router_request_types::{
        AccessTokenRequestData, CompleteAuthorizeData, PaymentMethodTokenizationData, PaymentsAuthorizeData,
        PaymentsCancelData, PaymentsCaptureData, PaymentsSessionData, PaymentsSyncData,
        RefundsData, SetupMandateRequestData,
    },
    router_response_types::{
        ConnectorInfo, PaymentsResponseData, RefundsResponseData, SupportedPaymentMethods,
    },
    types::{
        PaymentsAuthorizeRouterData, PaymentsCaptureRouterData, PaymentsCompleteAuthorizeRouterData,
        PaymentsSyncRouterData, RefundSyncRouterData, RefundsRouterData,
    },
};
use hyperswitch_interfaces::{
    api::{
        self, ConnectorCommon, ConnectorCommonExt, ConnectorIntegration, ConnectorSpecifications,
        ConnectorValidation,
    },
    configs::Connectors,
    errors,
    events::connector_api_logs::ConnectorEvent,
    types::Response,
    webhooks,
};
use transformers as nats_bridge;

use crate::utils;
use client_pool::NatsClientCache;

// Global Tokio runtime for blocking async NATS operations
// This is necessary because Hyperswitch connectors are called from async context
// but the build_request/handle_response methods are synchronous
use once_cell::sync::Lazy;
use tokio::runtime::Runtime;

static NATS_RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .thread_name("nats-bridge")
        .enable_all()
        .build()
        .expect("Failed to create NATS runtime")
});

// NATS Subjects per NATS.md protocol
const SUBJECT_PAYMENTS_AUTHORIZE: &str = "hyperswitch.payments.authorize";
const SUBJECT_PAYMENTS_COMPLETE_AUTHORIZE: &str = "hyperswitch.payments.complete_authorize";
const SUBJECT_PAYMENTS_CAPTURE: &str = "hyperswitch.payments.capture";
#[allow(dead_code)] // Will be used when Void operation is implemented
const SUBJECT_PAYMENTS_VOID: &str = "hyperswitch.payments.void";
const SUBJECT_PAYMENTS_SYNC: &str = "hyperswitch.payments.sync";
const SUBJECT_REFUNDS_EXECUTE: &str = "hyperswitch.refunds.execute";
const SUBJECT_REFUNDS_SYNC: &str = "hyperswitch.refunds.sync";

// Operation timeouts (request-reply pattern)
const TIMEOUT_AUTHORIZE: std::time::Duration = std::time::Duration::from_secs(30);
const TIMEOUT_CAPTURE: std::time::Duration = std::time::Duration::from_secs(30);
const TIMEOUT_REFUND: std::time::Duration = std::time::Duration::from_secs(30);
const TIMEOUT_SYNC: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Clone)]
pub struct NatsBridge {
    amount_converter: &'static (dyn AmountConvertor<Output = StringMinorUnit> + Sync),
}

impl NatsBridge {
    pub fn new() -> &'static Self {
        &Self {
            amount_converter: &StringMinorUnitForConnector,
        }
    }

    /// Build NATS headers per protocol specification
    fn build_nats_headers<F, Req, Res>(
        &self,
        router_data: &RouterData<F, Req, Res>,
    ) -> HeaderMap {
        let mut headers = HeaderMap::new();

        // Required headers per NATS protocol (NATS.md)
        headers.insert("X-Merchant-Id", router_data.merchant_id.get_string_repr());
        headers.insert("X-Payment-Id", router_data.payment_id.as_str());
        headers.insert("X-Correlation-Id", router_data.connector_request_reference_id.as_str());

        // Idempotency header (Nats-Msg-Id) for JetStream deduplication (5-minute window)
        let msg_id = format!("{}_{}", router_data.payment_id, router_data.attempt_id);
        headers.insert("Nats-Msg-Id", msg_id.as_str());

        headers
    }
}

impl api::Payment for NatsBridge {}
impl api::PaymentSession for NatsBridge {}
impl api::MandateSetup for NatsBridge {}
impl api::PaymentAuthorize for NatsBridge {}
impl api::PaymentSync for NatsBridge {}
impl api::PaymentCapture for NatsBridge {}
impl api::PaymentVoid for NatsBridge {}
impl api::PaymentsCompleteAuthorize for NatsBridge {}
impl api::Refund for NatsBridge {}
impl api::RefundExecute for NatsBridge {}
impl api::RefundSync for NatsBridge {}
impl api::PaymentToken for NatsBridge {}

impl ConnectorIntegration<PaymentMethodToken, PaymentMethodTokenizationData, PaymentsResponseData>
    for NatsBridge
{
    // Not Implemented
}

impl<Flow, Request, Response> ConnectorCommonExt<Flow, Request, Response> for NatsBridge
where
    Self: ConnectorIntegration<Flow, Request, Response>,
{
    fn build_headers(
        &self,
        _req: &RouterData<Flow, Request, Response>,
        _connectors: &Connectors,
    ) -> CustomResult<Vec<(String, masking::Maskable<String>)>, errors::ConnectorError> {
        // NATS uses its own header system, not HTTP headers
        Ok(vec![])
    }
}

impl ConnectorCommon for NatsBridge {
    fn id(&self) -> &'static str {
        "nats_bridge"
    }

    fn get_currency_unit(&self) -> api::CurrencyUnit {
        // Workers handle amount conversion
        api::CurrencyUnit::Minor
    }

    fn common_get_content_type(&self) -> &'static str {
        "application/json"
    }

    fn base_url<'a>(&self, connectors: &'a Connectors) -> &'a str {
        connectors.nats_bridge.base_url.as_ref()
    }

    fn get_auth_header(
        &self,
        _auth_type: &ConnectorAuthType,
    ) -> CustomResult<Vec<(String, masking::Maskable<String>)>, errors::ConnectorError> {
        // NATS authentication is connection-level
        Ok(vec![])
    }

    fn build_error_response(
        &self,
        res: Response,
        event_builder: Option<&mut ConnectorEvent>,
    ) -> CustomResult<ErrorResponse, errors::ConnectorError> {
        let response: nats_bridge::NatsBridgeErrorResponse = res
            .response
            .parse_struct("NatsBridgeErrorResponse")
            .change_context(errors::ConnectorError::ResponseDeserializationFailed)?;

        event_builder.map(|i| i.set_response_body(&response));
        router_env::logger::info!(connector_response=?response);

        Ok(ErrorResponse {
            status_code: res.status_code,
            code: response.code,
            message: response.message,
            reason: response.reason,
            attempt_status: None,
            connector_transaction_id: None,
            connector_metadata: None,
            network_advice_code: None,
            network_decline_code: None,
            network_error_message: None,
        })
    }
}

impl ConnectorValidation for NatsBridge {
    fn validate_psync_reference_id(
        &self,
        _data: &PaymentsSyncData,
        _is_three_ds: bool,
        _status: enums::AttemptStatus,
        _connector_meta_data: Option<common_utils::pii::SecretSerdeValue>,
    ) -> CustomResult<(), errors::ConnectorError> {
        Ok(())
    }
}

impl ConnectorIntegration<Session, PaymentsSessionData, PaymentsResponseData> for NatsBridge {
    // TODO: implement sessions flow via hyperswitch.payments.session
}

impl ConnectorIntegration<SetupMandate, SetupMandateRequestData, PaymentsResponseData>
    for NatsBridge
{
    // TODO: implement mandate setup via hyperswitch.mandates.setup
}

// ============================================================================
// AUTHORIZE - Request-Reply Pattern (Synchronous with Worker)
// ============================================================================
impl ConnectorIntegration<Authorize, PaymentsAuthorizeData, PaymentsResponseData> for NatsBridge {
    fn build_request(
        &self,
        req: &PaymentsAuthorizeRouterData,
        connectors: &Connectors,
    ) -> CustomResult<Option<Request>, errors::ConnectorError> {
        let nats_url = self.base_url(connectors);
        let headers = self.build_nats_headers(req);

        let amount = utils::convert_amount(
            self.amount_converter,
            req.request.minor_amount,
            req.request.currency,
        )?;

        let connector_router_data = nats_bridge::NatsBridgeRouterData::from((amount, req));
        let connector_req =
            nats_bridge::NatsBridgePaymentsRequest::try_from(&connector_router_data)?;

        // Use hybrid JetStream + request-reply pattern for financial operations
        // JetStream provides persistence and idempotency, inbox provides synchronous response for 3DS
        let response_payload = tokio::task::block_in_place(|| {
            NATS_RUNTIME.block_on(async {
                let cache = NatsClientCache::global();
                let client = cache.get_client(nats_url).await?;
                let jetstream = cache.get_jetstream(&client).await?;

                cache
                    .publish_with_response(
                        &jetstream,
                        &client,
                        SUBJECT_PAYMENTS_AUTHORIZE,
                        headers,
                        &connector_req,
                        TIMEOUT_AUTHORIZE,
                    )
                    .await
            })
        })?;

        router_env::logger::info!(
            payment_id = ?req.payment_id,
            attempt_id = ?req.attempt_id,
            "Received authorize response from worker (via JetStream + inbox)"
        );

        // Return response as HTTP request body for handle_response to parse
        Ok(Some(
            common_utils::request::RequestBuilder::new()
                .method(common_utils::request::Method::Post)
                .url(&format!("nats://{}", SUBJECT_PAYMENTS_AUTHORIZE))
                .set_body(common_utils::request::RequestContent::RawBytes(response_payload.to_vec()))
                .build(),
        ))
    }

    fn handle_response(
        &self,
        data: &PaymentsAuthorizeRouterData,
        event_builder: Option<&mut ConnectorEvent>,
        res: Response,
    ) -> CustomResult<PaymentsAuthorizeRouterData, errors::ConnectorError> {
        let response: nats_bridge::NatsBridgePaymentsResponse = res
            .response
            .parse_struct("NatsBridge PaymentsAuthorizeResponse")
            .change_context(errors::ConnectorError::ResponseDeserializationFailed)?;
        event_builder.map(|i| i.set_response_body(&response));
        router_env::logger::info!(connector_response=?response);

        RouterData::try_from(crate::types::ResponseRouterData {
            response,
            data: data.clone(),
            http_code: res.status_code,
        })
    }

    fn get_error_response(
        &self,
        res: Response,
        event_builder: Option<&mut ConnectorEvent>,
    ) -> CustomResult<ErrorResponse, errors::ConnectorError> {
        self.build_error_response(res, event_builder)
    }
}

// ============================================================================
// COMPLETE AUTHORIZE - JetStream + Inbox Pattern (3DS/2FA Completion)
// ============================================================================
impl ConnectorIntegration<CompleteAuthorize, CompleteAuthorizeData, PaymentsResponseData>
    for NatsBridge
{
    fn build_request(
        &self,
        req: &PaymentsCompleteAuthorizeRouterData,
        connectors: &Connectors,
    ) -> CustomResult<Option<Request>, errors::ConnectorError> {
        let nats_url = self.base_url(connectors);
        let headers = self.build_nats_headers(req);

        // CompleteAuthorizeData has i64 amount, convert to MinorUnit first
        let minor_unit_amount = common_utils::types::MinorUnit::new(req.request.amount);
        let amount = utils::convert_amount(
            self.amount_converter,
            minor_unit_amount,
            req.request.currency,
        )?;

        let connector_router_data = nats_bridge::NatsBridgeRouterData::from((amount, req));
        let connector_req =
            nats_bridge::NatsBridgeCompleteAuthorizeRequest::try_from(&connector_router_data)?;

        // Use hybrid JetStream + request-reply pattern (3DS needs synchronous response)
        let response_payload = tokio::task::block_in_place(|| {
            NATS_RUNTIME.block_on(async {
                let cache = NatsClientCache::global();
                let client = cache.get_client(nats_url).await?;
                let jetstream = cache.get_jetstream(&client).await?;

                cache
                    .publish_with_response(
                        &jetstream,
                        &client,
                        SUBJECT_PAYMENTS_COMPLETE_AUTHORIZE,
                        headers,
                        &connector_req,
                        TIMEOUT_AUTHORIZE,
                    )
                    .await
            })
        })?;

        router_env::logger::info!(
            payment_id = ?req.payment_id,
            attempt_id = ?req.attempt_id,
            "Received complete_authorize response from worker (via JetStream + inbox)"
        );

        Ok(Some(
            common_utils::request::RequestBuilder::new()
                .method(common_utils::request::Method::Post)
                .url(&format!("nats://{}", SUBJECT_PAYMENTS_COMPLETE_AUTHORIZE))
                .set_body(common_utils::request::RequestContent::RawBytes(
                    response_payload.to_vec(),
                ))
                .build(),
        ))
    }

    fn handle_response(
        &self,
        data: &PaymentsCompleteAuthorizeRouterData,
        event_builder: Option<&mut ConnectorEvent>,
        res: Response,
    ) -> CustomResult<PaymentsCompleteAuthorizeRouterData, errors::ConnectorError> {
        let response: nats_bridge::NatsBridgePaymentsResponse = res
            .response
            .parse_struct("NatsBridge CompleteAuthorizeResponse")
            .change_context(errors::ConnectorError::ResponseDeserializationFailed)?;
        event_builder.map(|i| i.set_response_body(&response));
        router_env::logger::info!(connector_response=?response);

        RouterData::try_from(crate::types::ResponseRouterData {
            response,
            data: data.clone(),
            http_code: res.status_code,
        })
    }

    fn get_error_response(
        &self,
        res: Response,
        event_builder: Option<&mut ConnectorEvent>,
    ) -> CustomResult<ErrorResponse, errors::ConnectorError> {
        self.build_error_response(res, event_builder)
    }
}

// ============================================================================
// SYNC - Request-Reply Pattern (Query Worker for Status)
// ============================================================================
impl ConnectorIntegration<PSync, PaymentsSyncData, PaymentsResponseData> for NatsBridge {
    fn build_request(
        &self,
        req: &PaymentsSyncRouterData,
        connectors: &Connectors,
    ) -> CustomResult<Option<Request>, errors::ConnectorError> {
        let nats_url = self.base_url(connectors);
        let headers = self.build_nats_headers(req);
        let connector_req = nats_bridge::NatsBridgeSyncRequest::try_from(req)?;

        // Use request-reply to query worker for payment status
        let response_payload = tokio::task::block_in_place(|| {
            NATS_RUNTIME.block_on(async {
                let cache = NatsClientCache::global();
                let client = cache.get_client(nats_url).await?;

                cache
                    .request_reply(
                        &client,
                        SUBJECT_PAYMENTS_SYNC,
                        headers,
                        &connector_req,
                        TIMEOUT_SYNC,
                    )
                    .await
            })
        })?;

        router_env::logger::info!(
            payment_id = ?req.payment_id,
            "Received sync response from worker"
        );

        Ok(Some(
            common_utils::request::RequestBuilder::new()
                .method(common_utils::request::Method::Get)
                .url(&format!("nats://{}", SUBJECT_PAYMENTS_SYNC))
                .set_body(common_utils::request::RequestContent::RawBytes(response_payload.to_vec()))
                .build(),
        ))
    }

    fn handle_response(
        &self,
        data: &PaymentsSyncRouterData,
        event_builder: Option<&mut ConnectorEvent>,
        res: Response,
    ) -> CustomResult<PaymentsSyncRouterData, errors::ConnectorError> {
        let response: nats_bridge::NatsBridgePaymentsResponse = res
            .response
            .parse_struct("nats_bridge PaymentsSyncResponse")
            .change_context(errors::ConnectorError::ResponseDeserializationFailed)?;
        event_builder.map(|i| i.set_response_body(&response));
        router_env::logger::info!(connector_response=?response);

        RouterData::try_from(crate::types::ResponseRouterData {
            response,
            data: data.clone(),
            http_code: res.status_code,
        })
    }

    fn get_error_response(
        &self,
        res: Response,
        event_builder: Option<&mut ConnectorEvent>,
    ) -> CustomResult<ErrorResponse, errors::ConnectorError> {
        self.build_error_response(res, event_builder)
    }
}

// ============================================================================
// CAPTURE - Request-Reply Pattern
// ============================================================================
impl ConnectorIntegration<Capture, PaymentsCaptureData, PaymentsResponseData> for NatsBridge {
    fn build_request(
        &self,
        req: &PaymentsCaptureRouterData,
        connectors: &Connectors,
    ) -> CustomResult<Option<Request>, errors::ConnectorError> {
        let nats_url = self.base_url(connectors);
        let headers = self.build_nats_headers(req);

        let amount = utils::convert_amount(
            self.amount_converter,
            req.request.minor_amount_to_capture,
            req.request.currency,
        )?;

        let connector_router_data = nats_bridge::NatsBridgeRouterData::from((amount, req));
        let connector_req =
            nats_bridge::NatsBridgeCaptureRequest::try_from(&connector_router_data)?;

        // Use hybrid JetStream + request-reply pattern for financial operations
        let response_payload = tokio::task::block_in_place(|| {
            NATS_RUNTIME.block_on(async {
                let cache = NatsClientCache::global();
                let client = cache.get_client(nats_url).await?;
                let jetstream = cache.get_jetstream(&client).await?;

                cache
                    .publish_with_response(
                        &jetstream,
                        &client,
                        SUBJECT_PAYMENTS_CAPTURE,
                        headers,
                        &connector_req,
                        TIMEOUT_CAPTURE,
                    )
                    .await
            })
        })?;

        router_env::logger::info!(
            payment_id = ?req.payment_id,
            attempt_id = ?req.attempt_id,
            "Received capture response from worker (via JetStream + inbox)"
        );

        Ok(Some(
            common_utils::request::RequestBuilder::new()
                .method(common_utils::request::Method::Post)
                .url(&format!("nats://{}", SUBJECT_PAYMENTS_CAPTURE))
                .set_body(common_utils::request::RequestContent::RawBytes(response_payload.to_vec()))
                .build(),
        ))
    }

    fn handle_response(
        &self,
        data: &PaymentsCaptureRouterData,
        event_builder: Option<&mut ConnectorEvent>,
        res: Response,
    ) -> CustomResult<PaymentsCaptureRouterData, errors::ConnectorError> {
        let response: nats_bridge::NatsBridgePaymentsResponse = res
            .response
            .parse_struct("NatsBridge PaymentsCaptureResponse")
            .change_context(errors::ConnectorError::ResponseDeserializationFailed)?;
        event_builder.map(|i| i.set_response_body(&response));
        router_env::logger::info!(connector_response=?response);

        RouterData::try_from(crate::types::ResponseRouterData {
            response,
            data: data.clone(),
            http_code: res.status_code,
        })
    }

    fn get_error_response(
        &self,
        res: Response,
        event_builder: Option<&mut ConnectorEvent>,
    ) -> CustomResult<ErrorResponse, errors::ConnectorError> {
        self.build_error_response(res, event_builder)
    }
}

impl ConnectorIntegration<Void, PaymentsCancelData, PaymentsResponseData> for NatsBridge {
    // TODO: Implement void via hyperswitch.payments.void
}

// ============================================================================
// REFUND EXECUTE - Request-Reply Pattern
// ============================================================================
impl ConnectorIntegration<Execute, RefundsData, RefundsResponseData> for NatsBridge {
    fn build_request(
        &self,
        req: &RefundsRouterData<Execute>,
        connectors: &Connectors,
    ) -> CustomResult<Option<Request>, errors::ConnectorError> {
        let nats_url = self.base_url(connectors);
        let headers = self.build_nats_headers(req);

        let refund_amount = utils::convert_amount(
            self.amount_converter,
            req.request.minor_refund_amount,
            req.request.currency,
        )?;

        let connector_router_data = nats_bridge::NatsBridgeRouterData::from((refund_amount, req));
        let connector_req = nats_bridge::NatsBridgeRefundRequest::try_from(&connector_router_data)?;

        // Use hybrid JetStream + request-reply pattern for financial operations
        let response_payload = tokio::task::block_in_place(|| {
            NATS_RUNTIME.block_on(async {
                let cache = NatsClientCache::global();
                let client = cache.get_client(nats_url).await?;
                let jetstream = cache.get_jetstream(&client).await?;

                cache
                    .publish_with_response(
                        &jetstream,
                        &client,
                        SUBJECT_REFUNDS_EXECUTE,
                        headers,
                        &connector_req,
                        TIMEOUT_REFUND,
                    )
                    .await
            })
        })?;

        router_env::logger::info!(
            refund_id = ?req.request.refund_id,
            payment_id = ?req.payment_id,
            "Received refund execute response from worker (via JetStream + inbox)"
        );

        Ok(Some(
            common_utils::request::RequestBuilder::new()
                .method(common_utils::request::Method::Post)
                .url(&format!("nats://{}", SUBJECT_REFUNDS_EXECUTE))
                .set_body(common_utils::request::RequestContent::RawBytes(response_payload.to_vec()))
                .build(),
        ))
    }

    fn handle_response(
        &self,
        data: &RefundsRouterData<Execute>,
        event_builder: Option<&mut ConnectorEvent>,
        res: Response,
    ) -> CustomResult<RefundsRouterData<Execute>, errors::ConnectorError> {
        let response: nats_bridge::RefundResponse = res
            .response
            .parse_struct("nats_bridge RefundResponse")
            .change_context(errors::ConnectorError::ResponseDeserializationFailed)?;
        event_builder.map(|i| i.set_response_body(&response));
        router_env::logger::info!(connector_response=?response);

        RouterData::try_from(crate::types::ResponseRouterData {
            response,
            data: data.clone(),
            http_code: res.status_code,
        })
    }

    fn get_error_response(
        &self,
        res: Response,
        event_builder: Option<&mut ConnectorEvent>,
    ) -> CustomResult<ErrorResponse, errors::ConnectorError> {
        self.build_error_response(res, event_builder)
    }
}

// ============================================================================
// REFUND SYNC - Request-Reply Pattern
// ============================================================================
impl ConnectorIntegration<RSync, RefundsData, RefundsResponseData> for NatsBridge {
    fn build_request(
        &self,
        req: &RefundSyncRouterData,
        connectors: &Connectors,
    ) -> CustomResult<Option<Request>, errors::ConnectorError> {
        let nats_url = self.base_url(connectors);
        let headers = self.build_nats_headers(req);

        // Build sync request with connector_refund_id
        let sync_request = serde_json::json!({
            "connector_refund_id": req.request.get_connector_refund_id()?,
        });

        // Use request-reply to query worker for refund status
        let response_payload = tokio::task::block_in_place(|| {
            NATS_RUNTIME.block_on(async {
                let cache = NatsClientCache::global();
                let client = cache.get_client(nats_url).await?;

                cache
                    .request_reply(
                        &client,
                        SUBJECT_REFUNDS_SYNC,
                        headers,
                        &sync_request,
                        TIMEOUT_SYNC,
                    )
                    .await
            })
        })?;

        router_env::logger::info!(
            refund_id = ?req.request.refund_id,
            "Received refund sync response from worker"
        );

        Ok(Some(
            common_utils::request::RequestBuilder::new()
                .method(common_utils::request::Method::Get)
                .url(&format!("nats://{}", SUBJECT_REFUNDS_SYNC))
                .set_body(common_utils::request::RequestContent::RawBytes(response_payload.to_vec()))
                .build(),
        ))
    }

    fn handle_response(
        &self,
        data: &RefundSyncRouterData,
        event_builder: Option<&mut ConnectorEvent>,
        res: Response,
    ) -> CustomResult<RefundSyncRouterData, errors::ConnectorError> {
        let response: nats_bridge::RefundResponse = res
            .response
            .parse_struct("nats_bridge RefundSyncResponse")
            .change_context(errors::ConnectorError::ResponseDeserializationFailed)?;
        event_builder.map(|i| i.set_response_body(&response));
        router_env::logger::info!(connector_response=?response);

        RouterData::try_from(crate::types::ResponseRouterData {
            response,
            data: data.clone(),
            http_code: res.status_code,
        })
    }

    fn get_error_response(
        &self,
        res: Response,
        event_builder: Option<&mut ConnectorEvent>,
    ) -> CustomResult<ErrorResponse, errors::ConnectorError> {
        self.build_error_response(res, event_builder)
    }
}

#[async_trait::async_trait]
impl webhooks::IncomingWebhook for NatsBridge {
    fn get_webhook_object_reference_id(
        &self,
        _request: &webhooks::IncomingWebhookRequestDetails<'_>,
    ) -> CustomResult<api_models::webhooks::ObjectReferenceId, errors::ConnectorError> {
        Err(report!(errors::ConnectorError::WebhooksNotImplemented))
    }

    fn get_webhook_event_type(
        &self,
        _request: &webhooks::IncomingWebhookRequestDetails<'_>,
    ) -> CustomResult<api_models::webhooks::IncomingWebhookEvent, errors::ConnectorError> {
        Err(report!(errors::ConnectorError::WebhooksNotImplemented))
    }

    fn get_webhook_resource_object(
        &self,
        _request: &webhooks::IncomingWebhookRequestDetails<'_>,
    ) -> CustomResult<Box<dyn masking::ErasedMaskSerialize>, errors::ConnectorError> {
        Err(report!(errors::ConnectorError::WebhooksNotImplemented))
    }
}

impl api::ConnectorAccessToken for NatsBridge {}

impl ConnectorIntegration<AccessTokenAuth, AccessTokenRequestData, AccessToken> for NatsBridge {}

use std::sync::LazyLock;

static NATS_BRIDGE_SUPPORTED_PAYMENT_METHODS: LazyLock<SupportedPaymentMethods> =
    LazyLock::new(SupportedPaymentMethods::new);

static NATS_BRIDGE_CONNECTOR_INFO: ConnectorInfo = ConnectorInfo {
    display_name: "NATS Bridge",
    description: "NATS message bus bridge for delegating payment operations to external workers",
    connector_type: enums::HyperswitchConnectorCategory::PaymentGateway,
    integration_status: enums::ConnectorIntegrationStatus::Alpha,
};

static NATS_BRIDGE_SUPPORTED_WEBHOOK_FLOWS: [enums::EventClass; 0] = [];

impl ConnectorSpecifications for NatsBridge {
    fn get_connector_about(&self) -> Option<&'static ConnectorInfo> {
        Some(&NATS_BRIDGE_CONNECTOR_INFO)
    }

    fn get_supported_payment_methods(&self) -> Option<&'static SupportedPaymentMethods> {
        Some(&*NATS_BRIDGE_SUPPORTED_PAYMENT_METHODS)
    }

    fn get_supported_webhook_flows(&self) -> Option<&'static [enums::EventClass]> {
        Some(&NATS_BRIDGE_SUPPORTED_WEBHOOK_FLOWS)
    }
}
