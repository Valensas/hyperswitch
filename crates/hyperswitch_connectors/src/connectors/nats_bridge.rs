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
use crate::{types::ResponseRouterData, utils::RefundsRequestData};
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
        ConnectorInfo, PaymentMethodDetails, PaymentsResponseData, RefundsResponseData,
        SupportedPaymentMethods, SupportedPaymentMethodsExt,
    },
    types::{
        PaymentsAuthorizeRouterData, PaymentsCancelRouterData, PaymentsCaptureRouterData, PaymentsCompleteAuthorizeRouterData,
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

// Global Tokio runtime for NATS operations
// Separate runtime needed because build_request is sync but NATS client is async
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
const SUBJECT_PAYMENTS_VOID: &str = "hyperswitch.payments.void";
const SUBJECT_PAYMENTS_SYNC: &str = "hyperswitch.payments.sync";
const SUBJECT_REFUNDS_EXECUTE: &str = "hyperswitch.refunds.execute";
const SUBJECT_REFUNDS_SYNC: &str = "hyperswitch.refunds.sync";

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

        // Wrap in envelope with merchant_id per NATS protocol
        let envelope = nats_bridge::NatsMessageEnvelope {
            merchant_id: req.merchant_id.get_string_repr().to_string(),
            payment_id: Some(req.payment_id.clone()),
            attempt_id: Some(req.attempt_id.clone()),
            correlation_id: req.connector_request_reference_id.clone(),
            router_data: connector_req.clone(),
        };

        // Use standard NATS request-reply pattern
        // Spawn blocking thread to avoid nested runtime issue
        let nats_url_clone = nats_url.to_string();
        let headers_clone = headers.clone();

        let response_payload: bytes::Bytes = std::thread::spawn(move || -> Result<bytes::Bytes, error_stack::Report<errors::ConnectorError>> {
            NATS_RUNTIME.block_on(async {
                let cache = NatsClientCache::global();
                let client = cache.get_client(&nats_url_clone).await?;

                // Serialize payload
                let payload_bytes = serde_json::to_vec(&envelope)
                    .change_context(errors::ConnectorError::RequestEncodingFailed)?;

                // Use NATS request-reply (automatically creates inbox and sets msg.reply)
                let response = client
                    .request_with_headers(
                        SUBJECT_PAYMENTS_AUTHORIZE.to_string(),
                        headers_clone,
                        payload_bytes.into(),
                    )
                    .await
                    .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

                Ok(response.payload)
            })
        })
        .join()
        .map_err(|_| errors::ConnectorError::ProcessingStepFailed(None))?
        .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

        router_env::logger::info!(
            payment_id = ?req.payment_id,
            attempt_id = ?req.attempt_id,
            "Received authorize response from worker (via JetStream + inbox)"
        );

        // We've already executed the NATS request and have the response
        // Return None to indicate no HTTP call should be made
        // Instead, we'll need to handle this in a custom way
        Ok(None)
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

        RouterData::try_from(ResponseRouterData {
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

        // Use standard NATS request-reply pattern
        let nats_url_clone = nats_url.to_string();
        let headers_clone = headers.clone();

        let response_payload: bytes::Bytes = std::thread::spawn(move || -> Result<bytes::Bytes, error_stack::Report<errors::ConnectorError>> {
            NATS_RUNTIME.block_on(async {
                let cache = NatsClientCache::global();
                let client = cache.get_client(&nats_url_clone).await?;

                let payload_bytes = serde_json::to_vec(&connector_req)
                    .change_context(errors::ConnectorError::RequestEncodingFailed)?;

                let response = client
                    .request_with_headers(
                        SUBJECT_PAYMENTS_COMPLETE_AUTHORIZE.to_string(),
                        headers_clone,
                        payload_bytes.into(),
                    )
                    .await
                    .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

                Ok(response.payload)
            })
        })
        .join()
        .map_err(|_| errors::ConnectorError::ProcessingStepFailed(None))?
        .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

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

        RouterData::try_from(ResponseRouterData {
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

        // Use standard NATS request-reply pattern
        let nats_url_clone = nats_url.to_string();
        let headers_clone = headers.clone();

        let response_payload: bytes::Bytes = std::thread::spawn(move || -> Result<bytes::Bytes, error_stack::Report<errors::ConnectorError>> {
            NATS_RUNTIME.block_on(async {
                let cache = NatsClientCache::global();
                let client = cache.get_client(&nats_url_clone).await?;

                let payload_bytes = serde_json::to_vec(&connector_req)
                    .change_context(errors::ConnectorError::RequestEncodingFailed)?;

                let response = client
                    .request_with_headers(
                        SUBJECT_PAYMENTS_SYNC.to_string(),
                        headers_clone,
                        payload_bytes.into(),
                    )
                    .await
                    .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

                Ok(response.payload)
            })
        })
        .join()
        .map_err(|_| errors::ConnectorError::ProcessingStepFailed(None))?
        .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

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

        RouterData::try_from(ResponseRouterData {
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

        // Use standard NATS request-reply pattern
        let nats_url_clone = nats_url.to_string();
        let headers_clone = headers.clone();

        let response_payload: bytes::Bytes = std::thread::spawn(move || -> Result<bytes::Bytes, error_stack::Report<errors::ConnectorError>> {
            NATS_RUNTIME.block_on(async {
                let cache = NatsClientCache::global();
                let client = cache.get_client(&nats_url_clone).await?;

                let payload_bytes = serde_json::to_vec(&connector_req)
                    .change_context(errors::ConnectorError::RequestEncodingFailed)?;

                let response = client
                    .request_with_headers(
                        SUBJECT_PAYMENTS_CAPTURE.to_string(),
                        headers_clone,
                        payload_bytes.into(),
                    )
                    .await
                    .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

                Ok(response.payload)
            })
        })
        .join()
        .map_err(|_| errors::ConnectorError::ProcessingStepFailed(None))?
        .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

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

        RouterData::try_from(ResponseRouterData {
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
// VOID - JetStream + Request-Reply Hybrid (Financial Operation)
// ============================================================================
impl ConnectorIntegration<Void, PaymentsCancelData, PaymentsResponseData> for NatsBridge {
    fn build_request(
        &self,
        req: &PaymentsCancelRouterData,
        connectors: &Connectors,
    ) -> CustomResult<Option<Request>, errors::ConnectorError> {
        let nats_url = self.base_url(connectors);
        let headers = self.build_nats_headers(req);

        // For void operations, amount is optional (typically cancels the entire authorization)
        // Use minor_amount if provided, otherwise use zero
        let amount = if let Some(minor_amount) = req.request.minor_amount {
            let currency = req.request.currency.ok_or(
                errors::ConnectorError::MissingRequiredField { field_name: "currency" }
            )?;
            utils::convert_amount(self.amount_converter, minor_amount, currency)?
        } else {
            // No amount specified - void the entire payment (use zero as placeholder)
            // Use any currency since amount is zero - currency doesn't matter
            let zero_amount = common_utils::types::MinorUnit::new(0);
            self.amount_converter
                .convert(zero_amount, enums::Currency::USD)
                .change_context(errors::ConnectorError::RequestEncodingFailed)?
        };

        let connector_router_data = nats_bridge::NatsBridgeRouterData::from((amount, req));
        let connector_req = nats_bridge::NatsBridgeVoidRequest::try_from(&connector_router_data)?;

        // Use standard NATS request-reply pattern
        let nats_url_clone = nats_url.to_string();
        let headers_clone = headers.clone();

        let response_payload: bytes::Bytes = std::thread::spawn(move || -> Result<bytes::Bytes, error_stack::Report<errors::ConnectorError>> {
            NATS_RUNTIME.block_on(async {
                let cache = NatsClientCache::global();
                let client = cache.get_client(&nats_url_clone).await?;

                let payload_bytes = serde_json::to_vec(&connector_req)
                    .change_context(errors::ConnectorError::RequestEncodingFailed)?;

                let response = client
                    .request_with_headers(
                        SUBJECT_PAYMENTS_VOID.to_string(),
                        headers_clone,
                        payload_bytes.into(),
                    )
                    .await
                    .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

                Ok(response.payload)
            })
        })
        .join()
        .map_err(|_| errors::ConnectorError::ProcessingStepFailed(None))?
        .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

        router_env::logger::info!(
            payment_id = ?req.payment_id,
            attempt_id = ?req.attempt_id,
            "Received void response from worker (via JetStream + inbox)"
        );

        Ok(Some(
            common_utils::request::RequestBuilder::new()
                .method(common_utils::request::Method::Post)
                .url(&format!("nats://{}", SUBJECT_PAYMENTS_VOID))
                .set_body(common_utils::request::RequestContent::RawBytes(response_payload.to_vec()))
                .build(),
        ))
    }

    fn handle_response(
        &self,
        data: &PaymentsCancelRouterData,
        event_builder: Option<&mut ConnectorEvent>,
        res: Response,
    ) -> CustomResult<PaymentsCancelRouterData, errors::ConnectorError> {
        let response: nats_bridge::NatsBridgePaymentsResponse = res
            .response
            .parse_struct("NatsBridgePaymentsResponse")
            .change_context(errors::ConnectorError::ResponseDeserializationFailed)?;

        router_env::logger::info!(connector_response=?response);

        event_builder.map(|event| event.set_response_body(&response));

        RouterData::try_from(ResponseRouterData {
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

        // Use standard NATS request-reply pattern
        let nats_url_clone = nats_url.to_string();
        let headers_clone = headers.clone();

        let response_payload: bytes::Bytes = std::thread::spawn(move || -> Result<bytes::Bytes, error_stack::Report<errors::ConnectorError>> {
            NATS_RUNTIME.block_on(async {
                let cache = NatsClientCache::global();
                let client = cache.get_client(&nats_url_clone).await?;

                let payload_bytes = serde_json::to_vec(&connector_req)
                    .change_context(errors::ConnectorError::RequestEncodingFailed)?;

                let response = client
                    .request_with_headers(
                        SUBJECT_REFUNDS_EXECUTE.to_string(),
                        headers_clone,
                        payload_bytes.into(),
                    )
                    .await
                    .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

                Ok(response.payload)
            })
        })
        .join()
        .map_err(|_| errors::ConnectorError::ProcessingStepFailed(None))?
        .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

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

        RouterData::try_from(ResponseRouterData {
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

        // Use standard NATS request-reply pattern
        let nats_url_clone = nats_url.to_string();
        let headers_clone = headers.clone();

        let response_payload: bytes::Bytes = std::thread::spawn(move || -> Result<bytes::Bytes, error_stack::Report<errors::ConnectorError>> {
            NATS_RUNTIME.block_on(async {
                let cache = NatsClientCache::global();
                let client = cache.get_client(&nats_url_clone).await?;

                let payload_bytes = serde_json::to_vec(&sync_request)
                    .change_context(errors::ConnectorError::RequestEncodingFailed)?;

                let response = client
                    .request_with_headers(
                        SUBJECT_REFUNDS_SYNC.to_string(),
                        headers_clone,
                        payload_bytes.into(),
                    )
                    .await
                    .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

                Ok(response.payload)
            })
        })
        .join()
        .map_err(|_| errors::ConnectorError::ProcessingStepFailed(None))?
        .change_context(errors::ConnectorError::ProcessingStepFailed(None))?;

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

        RouterData::try_from(ResponseRouterData {
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
    LazyLock::new(|| {
        use common_enums::{CaptureMethod, FeatureStatus, PaymentMethod};
        use api_models::feature_matrix::{CardSpecificFeatures, PaymentMethodSpecificFeatures};

        let default_capture_methods = vec![
            CaptureMethod::Automatic,
            CaptureMethod::Manual,
        ];

        let supported_card_networks = vec![
            common_enums::CardNetwork::Visa,
            common_enums::CardNetwork::Mastercard,
            common_enums::CardNetwork::AmericanExpress,
            common_enums::CardNetwork::Discover,
        ];

        let mut supported = SupportedPaymentMethods::new();

        // Credit cards
        supported.add(
            PaymentMethod::Card,
            enums::PaymentMethodType::Credit,
            PaymentMethodDetails {
                mandates: FeatureStatus::NotSupported,
                refunds: FeatureStatus::Supported,
                supported_capture_methods: default_capture_methods.clone(),
                specific_features: Some(PaymentMethodSpecificFeatures::Card(
                    CardSpecificFeatures {
                        three_ds: FeatureStatus::Supported,
                        no_three_ds: FeatureStatus::Supported,
                        supported_card_networks: supported_card_networks.clone(),
                    },
                )),
            },
        );

        // Debit cards
        supported.add(
            PaymentMethod::Card,
            enums::PaymentMethodType::Debit,
            PaymentMethodDetails {
                mandates: FeatureStatus::NotSupported,
                refunds: FeatureStatus::Supported,
                supported_capture_methods: default_capture_methods,
                specific_features: Some(PaymentMethodSpecificFeatures::Card(
                    CardSpecificFeatures {
                        three_ds: FeatureStatus::Supported,
                        no_three_ds: FeatureStatus::Supported,
                        supported_card_networks,
                    },
                )),
            },
        );

        supported
    });

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
