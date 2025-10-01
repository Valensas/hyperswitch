use std::collections::HashMap;

use common_enums::enums;
use common_utils::types::StringMinorUnit;
use error_stack::ResultExt;
use hyperswitch_domain_models::{
    router_data::{ConnectorAuthType, RouterData},
    router_flow_types::refunds::{Execute, RSync},
    router_request_types::ResponseId,
    router_response_types::{PaymentsResponseData, RedirectForm, RefundsResponseData},
    types::{PaymentsAuthorizeRouterData, PaymentsCancelRouterData, PaymentsCaptureRouterData, PaymentsCompleteAuthorizeRouterData, PaymentsSyncRouterData, RefundsRouterData},
};
use hyperswitch_interfaces::errors;
use masking::{ExposeInterface, Secret};
use serde::{Deserialize, Serialize};

use crate::types::{RefundsResponseRouterData, ResponseRouterData};

/// Router Data wrapper with amount
pub struct NatsBridgeRouterData<T> {
    pub amount: StringMinorUnit,
    pub router_data: T,
}

impl<T> From<(StringMinorUnit, T)> for NatsBridgeRouterData<T> {
    fn from((amount, router_data): (StringMinorUnit, T)) -> Self {
        Self {
            amount,
            router_data,
        }
    }
}

/// Authentication Type for NATS Bridge
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NatsBridgeAuthType {
    pub(super) api_key: Secret<String>,
}

impl TryFrom<&ConnectorAuthType> for NatsBridgeAuthType {
    type Error = error_stack::Report<errors::ConnectorError>;

    fn try_from(auth_type: &ConnectorAuthType) -> Result<Self, Self::Error> {
        match auth_type {
            ConnectorAuthType::HeaderKey { api_key } => Ok(Self {
                api_key: api_key.clone(),
            }),
            _ => Err(errors::ConnectorError::FailedToObtainAuthType.into()),
        }
    }
}

/// NATS Message Envelope - wraps all operations
#[derive(Debug, Serialize, Deserialize)]
pub struct NatsMessageEnvelope<T> {
    pub merchant_id: String,
    pub payment_id: Option<String>,
    pub attempt_id: Option<String>,
    pub correlation_id: String,
    pub router_data: T,
}

/// Payment Request for NATS workers - Authorize
#[derive(Debug, Serialize, Deserialize)]
pub struct NatsBridgePaymentsRequest {
    pub amount: String,
    pub currency: enums::Currency,
    pub payment_method: enums::PaymentMethod,
    pub payment_method_type: Option<enums::PaymentMethodType>,
    pub payment_method_data: serde_json::Value,
    pub return_url: Option<String>,
    pub webhook_url: Option<String>,
    pub browser_info: Option<serde_json::Value>,
    pub statement_descriptor: Option<String>,
    pub metadata: Option<HashMap<String, String>>,
}

impl TryFrom<&NatsBridgeRouterData<&PaymentsAuthorizeRouterData>> for NatsBridgePaymentsRequest {
    type Error = error_stack::Report<errors::ConnectorError>;

    fn try_from(
        item: &NatsBridgeRouterData<&PaymentsAuthorizeRouterData>,
    ) -> Result<Self, Self::Error> {
        let router_data = item.router_data;
        let request = &router_data.request;

        Ok(Self {
            amount: item.amount.to_string(),
            currency: request.currency,
            payment_method: request.payment_method_data.get_payment_method().ok_or(
                errors::ConnectorError::MissingRequiredField { field_name: "payment_method" }
            )?,
            payment_method_type: request.payment_method_type,
            payment_method_data: serde_json::to_value(&request.payment_method_data)
                .change_context(errors::ConnectorError::RequestEncodingFailed)?,
            return_url: request.router_return_url.clone(),
            webhook_url: request.webhook_url.clone(),
            browser_info: request.browser_info.as_ref().map(|info| {
                serde_json::to_value(info).unwrap_or(serde_json::Value::Null)
            }),
            statement_descriptor: request.statement_descriptor.clone(),
            metadata: None,
        })
    }
}

/// CompleteAuthorize Request (3DS/2FA completion)
#[derive(Debug, Serialize, Deserialize)]
pub struct NatsBridgeCompleteAuthorizeRequest {
    pub amount: String,
    pub currency: enums::Currency,
    pub payment_method_data: Option<serde_json::Value>,
    pub connector_transaction_id: Option<String>,
    pub redirect_params: Option<String>,
    pub redirect_payload: Option<serde_json::Value>,
}

impl TryFrom<&NatsBridgeRouterData<&PaymentsCompleteAuthorizeRouterData>>
    for NatsBridgeCompleteAuthorizeRequest
{
    type Error = error_stack::Report<errors::ConnectorError>;

    fn try_from(
        item: &NatsBridgeRouterData<&PaymentsCompleteAuthorizeRouterData>,
    ) -> Result<Self, Self::Error> {
        let router_data = item.router_data;

        Ok(Self {
            amount: item.amount.to_string(),
            currency: router_data.request.currency,
            payment_method_data: router_data.request.payment_method_data.as_ref().map(|pmd| {
                serde_json::to_value(pmd).unwrap_or(serde_json::Value::Null)
            }),
            connector_transaction_id: router_data.request.connector_transaction_id.clone(),
            redirect_params: router_data.request.redirect_response.as_ref()
                .and_then(|rr| rr.params.as_ref().map(|p| p.clone().expose())),
            redirect_payload: router_data.request.redirect_response.as_ref()
                .and_then(|rr| rr.payload.as_ref().map(|p| p.clone().expose())),
        })
    }
}

/// Capture Request
#[derive(Debug, Serialize, Deserialize)]
pub struct NatsBridgeCaptureRequest {
    pub amount_to_capture: String,
    pub currency: enums::Currency,
    pub connector_transaction_id: String,
}

impl TryFrom<&NatsBridgeRouterData<&PaymentsCaptureRouterData>> for NatsBridgeCaptureRequest {
    type Error = error_stack::Report<errors::ConnectorError>;

    fn try_from(
        item: &NatsBridgeRouterData<&PaymentsCaptureRouterData>,
    ) -> Result<Self, Self::Error> {
        let router_data = item.router_data;

        Ok(Self {
            amount_to_capture: item.amount.to_string(),
            currency: router_data.request.currency,
            connector_transaction_id: router_data
                .request
                .connector_transaction_id
                .clone(),
        })
    }
}

/// Void Request (Payment Cancellation)
#[derive(Debug, Serialize, Deserialize)]
pub struct NatsBridgeVoidRequest {
    pub connector_transaction_id: String,
    pub cancellation_reason: Option<String>,
    pub amount: Option<String>,
    pub currency: Option<enums::Currency>,
}

impl TryFrom<&NatsBridgeRouterData<&PaymentsCancelRouterData>> for NatsBridgeVoidRequest {
    type Error = error_stack::Report<errors::ConnectorError>;

    fn try_from(
        item: &NatsBridgeRouterData<&PaymentsCancelRouterData>,
    ) -> Result<Self, Self::Error> {
        let router_data = item.router_data;

        Ok(Self {
            connector_transaction_id: router_data.request.connector_transaction_id.clone(),
            cancellation_reason: router_data.request.cancellation_reason.clone(),
            amount: Some(item.amount.to_string()),
            currency: router_data.request.currency,
        })
    }
}

/// Sync Request
#[derive(Debug, Serialize, Deserialize)]
pub struct NatsBridgeSyncRequest {
    pub connector_transaction_id: Option<String>,
    pub encoded_data: Option<String>,
    pub connector_meta: Option<serde_json::Value>,
}

impl TryFrom<&PaymentsSyncRouterData> for NatsBridgeSyncRequest {
    type Error = error_stack::Report<errors::ConnectorError>;

    fn try_from(router_data: &PaymentsSyncRouterData) -> Result<Self, Self::Error> {
        Ok(Self {
            connector_transaction_id: router_data
                .request
                .connector_transaction_id
                .get_connector_transaction_id()
                .ok()
                .map(|id| id.to_string()),
            encoded_data: router_data.request.encoded_data.clone(),
            connector_meta: router_data.request.connector_meta.clone(),
        })
    }
}

/// Payment Response from NATS workers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NatsBridgePaymentsResponse {
    pub status: NatsPaymentStatus,
    pub connector_transaction_id: Option<String>,
    pub redirect_form: Option<NatsRedirectForm>,
    pub connector_metadata: Option<serde_json::Value>,
    pub network_txn_id: Option<String>,
    pub connector_response_reference_id: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub error_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NatsPaymentStatus {
    Succeeded,
    Failed,
    Processing,
    RequiresCustomerAction,
    RequiresMerchantAction,
    RequiresPaymentMethod,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NatsRedirectForm {
    Html {
        html_data: String,
    },
    Form {
        endpoint: String,
        method: String,
        form_fields: HashMap<String, String>,
    },
}

impl<F, T> TryFrom<ResponseRouterData<F, NatsBridgePaymentsResponse, T, PaymentsResponseData>>
    for RouterData<F, T, PaymentsResponseData>
{
    type Error = error_stack::Report<errors::ConnectorError>;

    fn try_from(
        item: ResponseRouterData<F, NatsBridgePaymentsResponse, T, PaymentsResponseData>,
    ) -> Result<Self, Self::Error> {
        let response = item.response;

        let status = match response.status {
            NatsPaymentStatus::Succeeded => enums::AttemptStatus::Charged,
            NatsPaymentStatus::Failed => enums::AttemptStatus::Failure,
            NatsPaymentStatus::Processing => enums::AttemptStatus::Authorizing,
            NatsPaymentStatus::RequiresCustomerAction => enums::AttemptStatus::AuthenticationPending,
            NatsPaymentStatus::RequiresMerchantAction => enums::AttemptStatus::Pending,
            NatsPaymentStatus::RequiresPaymentMethod => enums::AttemptStatus::PaymentMethodAwaited,
            NatsPaymentStatus::Cancelled => enums::AttemptStatus::Voided,
        };

        let router_response = if matches!(response.status, NatsPaymentStatus::Failed) {
            Err(hyperswitch_domain_models::router_data::ErrorResponse {
                code: response.error_code.unwrap_or_else(|| "UNKNOWN".to_string()),
                message: response.error_message.unwrap_or_else(|| "Payment failed".to_string()),
                reason: response.error_reason,
                status_code: item.http_code,
                attempt_status: Some(enums::AttemptStatus::Failure),
                connector_transaction_id: response.connector_transaction_id.clone(),
                connector_metadata: None,
                network_advice_code: None,
                network_decline_code: None,
                network_error_message: None,
            })
        } else {
            Ok(PaymentsResponseData::TransactionResponse {
                resource_id: ResponseId::ConnectorTransactionId(
                    response.connector_transaction_id.clone().unwrap_or_default(),
                ),
                redirection_data: Box::new(response.redirect_form.as_ref().and_then(|form| {
                    match form {
                        NatsRedirectForm::Html { html_data } => Some(RedirectForm::Html {
                            html_data: html_data.clone(),
                        }),
                        NatsRedirectForm::Form { endpoint, method, form_fields } => {
                            Some(RedirectForm::Form {
                                endpoint: endpoint.clone(),
                                method: method.parse().ok()?,
                                form_fields: form_fields.clone(),
                            })
                        }
                    }
                })),
                mandate_reference: Box::new(None),
                connector_metadata: response.connector_metadata,
                network_txn_id: response.network_txn_id,
                connector_response_reference_id: response.connector_response_reference_id,
                incremental_authorization_allowed: None,
                charges: None,
            })
        };

        Ok(Self {
            status,
            response: router_response,
            ..item.data
        })
    }
}

/// Refund Request
#[derive(Debug, Serialize, Deserialize)]
pub struct NatsBridgeRefundRequest {
    pub refund_amount: String,
    pub currency: enums::Currency,
    pub connector_transaction_id: String,
    pub refund_id: String,
    pub reason: Option<String>,
    pub webhook_url: Option<String>,
    pub metadata: Option<HashMap<String, String>>,
}

impl<F> TryFrom<&NatsBridgeRouterData<&RefundsRouterData<F>>> for NatsBridgeRefundRequest {
    type Error = error_stack::Report<errors::ConnectorError>;

    fn try_from(item: &NatsBridgeRouterData<&RefundsRouterData<F>>) -> Result<Self, Self::Error> {
        let router_data = item.router_data;

        Ok(Self {
            refund_amount: item.amount.to_string(),
            currency: router_data.request.currency,
            connector_transaction_id: router_data.request.connector_transaction_id.clone(),
            refund_id: router_data.request.refund_id.clone(),
            reason: router_data.request.reason.clone(),
            webhook_url: router_data.request.webhook_url.clone(),
            metadata: None,
        })
    }
}

/// Refund Response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefundResponse {
    pub refund_status: NatsRefundStatus,
    pub connector_refund_id: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NatsRefundStatus {
    Succeeded,
    Failed,
    Pending,
}

impl TryFrom<RefundsResponseRouterData<Execute, RefundResponse>> for RefundsRouterData<Execute> {
    type Error = error_stack::Report<errors::ConnectorError>;

    fn try_from(
        item: RefundsResponseRouterData<Execute, RefundResponse>,
    ) -> Result<Self, Self::Error> {
        let response = item.response;

        let router_response = if matches!(response.refund_status, NatsRefundStatus::Failed) {
            Err(hyperswitch_domain_models::router_data::ErrorResponse {
                code: response.error_code.unwrap_or_else(|| "UNKNOWN".to_string()),
                message: response.error_message.unwrap_or_else(|| "Refund failed".to_string()),
                reason: None,
                status_code: item.http_code,
                attempt_status: None,
                connector_transaction_id: None,
                connector_metadata: None,
                network_advice_code: None,
                network_decline_code: None,
                network_error_message: None,
            })
        } else {
            Ok(RefundsResponseData {
                connector_refund_id: response.connector_refund_id.unwrap_or_default(),
                refund_status: match response.refund_status {
                    NatsRefundStatus::Succeeded => enums::RefundStatus::Success,
                    NatsRefundStatus::Failed => enums::RefundStatus::Failure,
                    NatsRefundStatus::Pending => enums::RefundStatus::Pending,
                },
            })
        };

        Ok(Self {
            response: router_response,
            ..item.data
        })
    }
}

impl TryFrom<RefundsResponseRouterData<RSync, RefundResponse>> for RefundsRouterData<RSync> {
    type Error = error_stack::Report<errors::ConnectorError>;

    fn try_from(
        item: RefundsResponseRouterData<RSync, RefundResponse>,
    ) -> Result<Self, Self::Error> {
        let response = item.response;

        let router_response = if matches!(response.refund_status, NatsRefundStatus::Failed) {
            Err(hyperswitch_domain_models::router_data::ErrorResponse {
                code: response.error_code.unwrap_or_else(|| "UNKNOWN".to_string()),
                message: response.error_message.unwrap_or_else(|| "Refund sync failed".to_string()),
                reason: None,
                status_code: item.http_code,
                attempt_status: None,
                connector_transaction_id: None,
                connector_metadata: None,
                network_advice_code: None,
                network_decline_code: None,
                network_error_message: None,
            })
        } else {
            Ok(RefundsResponseData {
                connector_refund_id: response.connector_refund_id.unwrap_or_default(),
                refund_status: match response.refund_status {
                    NatsRefundStatus::Succeeded => enums::RefundStatus::Success,
                    NatsRefundStatus::Failed => enums::RefundStatus::Failure,
                    NatsRefundStatus::Pending => enums::RefundStatus::Pending,
                },
            })
        };

        Ok(Self {
            response: router_response,
            ..item.data
        })
    }
}

/// Error Response
#[derive(Debug, Serialize, Deserialize)]
pub struct NatsBridgeErrorResponse {
    pub code: String,
    pub message: String,
    pub reason: Option<String>,
}
