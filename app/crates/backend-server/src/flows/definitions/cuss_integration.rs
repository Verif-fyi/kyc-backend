use async_trait::async_trait;
use backend_flow_sdk::step::ContextUpdates;
use backend_flow_sdk::{Actor, FlowError, Step, StepContext, StepOutcome};
use gen_oas_client_cuss::apis::configuration::Configuration;
use gen_oas_client_cuss::apis::registration_api::{approve_and_deposit, register_customer};
use gen_oas_client_cuss::models::{ApproveAndDepositRequest, RegistrationRequest};
use serde_json::json;
use std::sync::Arc;

pub struct CussRegisterStep {
    cuss_config: Arc<Configuration>,
}

impl CussRegisterStep {
    pub fn new(cuss_url: String) -> Self {
        let config = Configuration {
            base_path: cuss_url,
            ..Default::default()
        };
        Self {
            cuss_config: Arc::new(config),
        }
    }
}

#[async_trait]
impl Step for CussRegisterStep {
    fn step_type(&self) -> &'static str {
        "CUSS_REGISTER_CUSTOMER"
    }

    fn actor(&self) -> Actor {
        Actor::System
    }

    fn human_id(&self) -> &'static str {
        "cuss_register"
    }

    fn feature(&self) -> Option<&'static str> {
        Some("flow-cuss-integration")
    }

    async fn execute(&self, ctx: &StepContext) -> Result<StepOutcome, FlowError> {
        tracing::debug!(step = self.step_type(), "Executing step");
        let phone = ctx
            .session_context
            .get("phone_number")
            .and_then(|v| v.as_str())
            .ok_or_else(|| FlowError::InvalidDefinition("Missing phone_number".to_string()))?;

        let full_name = ctx
            .session_context
            .get("recipient_full_name")
            .or_else(|| ctx.session_context.get("fullname"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| FlowError::InvalidDefinition("Missing full_name".to_string()))?;

        let external_id = ctx.session_id.clone();

        let request = RegistrationRequest::new(
            full_name.to_string(),
            phone.to_string(),
            external_id.clone(),
        );

        tracing::info!(
            "[CUSS_REGISTER] Registering customer: phone={}, name={}, external_id={}",
            phone,
            full_name,
            external_id
        );
        tracing::info!(
            "[CUSS_REGISTER] cuss base_path: {}, request: {:?}",
            self.cuss_config.base_path,
            request
        );

        match register_customer(&self.cuss_config, request).await {
            Ok(response) => {
                let fineract_client_id = response.fineract_client_id.unwrap_or(0);
                let savings_account_id = response.savings_account_id.unwrap_or(0);

                Ok(StepOutcome::Done {
                    output: Some(json!({
                        "fineractClientId": fineract_client_id,
                        "savingsAccountId": savings_account_id,
                        "success": true
                    })),
                    updates: Some(Box::new(ContextUpdates {
                        user_metadata_patch: Some(json!({
                            "fineractClientId": fineract_client_id,
                            "savingsAccountId": savings_account_id,
                            "cuss_registration_status": "COMPLETED",
                            "cuss_registration_at": chrono::Utc::now().to_rfc3339()
                        })),
                        user_metadata_eager_patch: None,
                        session_context_patch: Some(json!({
                            "fineractClientId": fineract_client_id,
                            "savingsAccountId": savings_account_id
                        })),
                        flow_context_patch: None,
                        notifications: None,
                    })),
                })
            }
            Err(err) => {
                let is_retryable = matches!(err, gen_oas_client_cuss::apis::Error::ResponseError(ref resp)
                    if resp.status.is_server_error());
                Ok(StepOutcome::Failed {
                    error: format!("CUSS register failed: {}", err),
                    retryable: is_retryable,
                })
            }
        }
    }
}

pub struct CussApproveStep {
    cuss_config: Arc<Configuration>,
}

impl CussApproveStep {
    pub fn new(cuss_url: String) -> Self {
        let config = Configuration {
            base_path: cuss_url,
            ..Default::default()
        };
        Self {
            cuss_config: Arc::new(config),
        }
    }
}

#[async_trait]
impl Step for CussApproveStep {
    fn step_type(&self) -> &'static str {
        "CUSS_APPROVE_AND_DEPOSIT"
    }

    fn actor(&self) -> Actor {
        Actor::System
    }

    fn human_id(&self) -> &'static str {
        "cuss_approve"
    }

    fn feature(&self) -> Option<&'static str> {
        Some("flow-cuss-integration")
    }

    async fn execute(&self, ctx: &StepContext) -> Result<StepOutcome, FlowError> {
        tracing::debug!(step = self.step_type(), "Executing step");
        let savings_account_id = ctx
            .flow_context
            .get("step_output")
            .and_then(|v| v.get("CUSS_REGISTER_CUSTOMER"))
            .and_then(|v| v.get("savingsAccountId"))
            .and_then(|v| v.as_i64())
            .ok_or_else(|| {
                FlowError::InvalidDefinition(
                    "Missing savingsAccountId from registration".to_string(),
                )
            })?;

        let deposit_amount = ctx
            .flow_context
            .get("amount")
            .or_else(|| ctx.session_context.get("deposit_amount"))
            .or_else(|| ctx.session_context.get("amount"))
            .and_then(|v| {
                if let Some(f) = v.as_f64() {
                    Some(f)
                } else if let Some(s) = v.as_str() {
                    s.parse::<f64>().ok()
                } else {
                    None
                }
            });

        let mut request = ApproveAndDepositRequest::new(savings_account_id);
        request.deposit_amount = deposit_amount;

        tracing::info!(
            "[CUSS_APPROVE] Approving account: savingsAccountId={}, depositAmount={:?}",
            savings_account_id,
            deposit_amount
        );
        tracing::info!(
            "[CUSS_APPROVE] cuss base_path: {}, request: {:?}",
            self.cuss_config.base_path,
            request
        );

        match approve_and_deposit(&self.cuss_config, request).await {
            Ok(response) => {
                let transaction_id = response.transaction_id.unwrap_or(0);

                Ok(StepOutcome::Done {
                    output: Some(json!({
                        "transactionId": transaction_id,
                        "savingsAccountId": savings_account_id,
                        "success": true
                    })),
                    updates: Some(Box::new(ContextUpdates {
                        user_metadata_patch: Some(json!({
                            "deposit_transaction_id": transaction_id,
                            "cuss_approval_status": "COMPLETED",
                            "cuss_approval_at": chrono::Utc::now().to_rfc3339()
                        })),
                        user_metadata_eager_patch: None,
                        session_context_patch: None,
                        flow_context_patch: None,
                        notifications: None,
                    })),
                })
            }
            Err(err) => {
                let is_retryable = matches!(err, gen_oas_client_cuss::apis::Error::ResponseError(ref resp)
                    if resp.status.is_server_error());
                Ok(StepOutcome::Failed {
                    error: format!("CUSS approve failed: {}", err),
                    retryable: is_retryable,
                })
            }
        }
    }
}

pub fn steps(cuss_url: String) -> Vec<Arc<dyn Step>> {
    vec![
        Arc::new(CussRegisterStep::new(cuss_url.clone())),
        Arc::new(CussApproveStep::new(cuss_url)),
    ]
}
