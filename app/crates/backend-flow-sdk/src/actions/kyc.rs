use crate::step::ContextUpdates;
use crate::{Actor, FlowError, Step, StepContext, StepOutcome};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentType {
    #[default]
    Id,
    Address,
    Selfie,
}

impl From<&str> for DocumentType {
    fn from(s: &str) -> Self {
        match s {
            "id" => DocumentType::Id,
            "address" => DocumentType::Address,
            "selfie" => DocumentType::Selfie,
            _ => DocumentType::Id,
        }
    }
}

impl std::str::FromStr for DocumentType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(s))
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[allow(dead_code)]
pub struct UploadDocumentConfig {
    #[serde(default)]
    pub document_type: DocumentType,

    #[serde(default = "default_bucket")]
    pub bucket: String,

    #[serde(default = "default_url_expiry")]
    pub url_expiry_seconds: u64,
}

fn default_bucket() -> String {
    "kyc-documents".to_string()
}

fn default_url_expiry() -> u64 {
    3600
}

pub struct UploadDocumentAction;

#[async_trait]
impl Step for UploadDocumentAction {
    fn step_type(&self) -> &'static str {
        "UPLOAD_DOCUMENT"
    }

    fn actor(&self) -> Actor {
        Actor::EndUser
    }

    fn human_id(&self) -> &'static str {
        "upload_document"
    }

    async fn execute(&self, ctx: &StepContext) -> Result<StepOutcome, FlowError> {
        tracing::debug!(step = self.step_type(), "Executing step");
        let config: UploadDocumentConfig = super::parse_step_config(ctx)?;

        let doc_type_str = match config.document_type {
            DocumentType::Id => "id",
            DocumentType::Address => "address",
            DocumentType::Selfie => "selfie",
        };

        tracing::info!(
            "[UPLOAD_DOCUMENT] Preparing upload URL for type={}, session={}",
            doc_type_str,
            ctx.session_id
        );

        if let Some(storage) = ctx.services.storage.as_ref() {
            let _upload_result = storage
                .generate_upload_url(doc_type_str, &ctx.session_id)
                .await
                .map_err(FlowError::InvalidDefinition)?;

            return Ok(StepOutcome::Waiting {
                actor: Actor::EndUser,
            });
        }

        Ok(StepOutcome::Waiting {
            actor: Actor::EndUser,
        })
    }

    async fn validate_input(&self, input: &serde_json::Value) -> Result<(), FlowError> {
        if input.get("upload_key").is_none() && input.get("uploaded").is_none() {
            return Err(FlowError::InvalidDefinition(
                "Missing upload_key or uploaded confirmation".to_string(),
            ));
        }
        Ok(())
    }

    async fn verify_input(
        &self,
        ctx: &StepContext,
        input: &serde_json::Value,
    ) -> Result<StepOutcome, FlowError> {
        let config: UploadDocumentConfig = super::parse_step_config(ctx)?;

        let doc_type_str = match config.document_type {
            DocumentType::Id => "id",
            DocumentType::Address => "address",
            DocumentType::Selfie => "selfie",
        };

        let upload_key = input.get("upload_key").and_then(|v| v.as_str());

        if let Some(key) = upload_key {
            tracing::info!(
                "[UPLOAD_DOCUMENT] Document uploaded: type={}, key={}, session={}",
                doc_type_str,
                key,
                ctx.session_id
            );

            return Ok(StepOutcome::Done {
                output: Some(json!({
                    "uploaded": true,
                    "document_type": doc_type_str,
                    "upload_key": key
                })),
                updates: Some(Box::new(ContextUpdates {
                    flow_context_patch: Some(json!({
                        "document_uploaded": true,
                        "document_type": doc_type_str,
                        "upload_key": key
                    })),
                    ..Default::default()
                })),
            });
        }

        Ok(StepOutcome::Failed {
            error: "UPLOAD_INCOMPLETE".to_string(),
            retryable: true,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ReviewDocumentConfig {
    #[serde(default)]
    pub document_type: String,

    #[serde(default)]
    pub on_approve: Option<String>,

    #[serde(default)]
    pub on_reject: Option<String>,
}

impl Default for ReviewDocumentConfig {
    fn default() -> Self {
        Self {
            document_type: "id".to_string(),
            on_approve: None,
            on_reject: None,
        }
    }
}

pub struct ReviewDocumentAction;

#[async_trait]
impl Step for ReviewDocumentAction {
    fn step_type(&self) -> &'static str {
        "REVIEW_DOCUMENT"
    }

    fn actor(&self) -> Actor {
        Actor::Admin
    }

    fn human_id(&self) -> &'static str {
        "review_document"
    }

    async fn execute(&self, ctx: &StepContext) -> Result<StepOutcome, FlowError> {
        tracing::debug!(step = self.step_type(), "Executing step");
        let config: ReviewDocumentConfig = super::parse_step_config(ctx)?;

        tracing::info!(
            "[REVIEW_DOCUMENT] Waiting for admin review: type={}, session={}",
            config.document_type,
            ctx.session_id
        );

        Ok(StepOutcome::Waiting {
            actor: Actor::Admin,
        })
    }

    async fn validate_input(&self, input: &serde_json::Value) -> Result<(), FlowError> {
        if input.get("approved").is_none() {
            return Err(FlowError::InvalidDefinition(
                "Missing approved field in input".to_string(),
            ));
        }
        Ok(())
    }

    async fn verify_input(
        &self,
        ctx: &StepContext,
        input: &serde_json::Value,
    ) -> Result<StepOutcome, FlowError> {
        let config: ReviewDocumentConfig = super::parse_step_config(ctx)?;

        let approved = input
            .get("approved")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| {
                FlowError::InvalidDefinition("approved must be a boolean".to_string())
            })?;

        let notes = input.get("notes").and_then(|v| v.as_str()).unwrap_or("");

        tracing::info!(
            "[REVIEW_DOCUMENT] Review completed: type={}, approved={}, notes={}, session={}",
            config.document_type,
            approved,
            notes,
            ctx.session_id
        );

        let mut updates = ContextUpdates {
            flow_context_patch: Some(json!({
                "review_completed": true,
                "review_approved": approved,
                "review_notes": notes,
                "review_document_type": config.document_type
            })),
            ..Default::default()
        };

        if approved {
            updates.user_metadata_patch = Some(json!({
                "kyc": {
                    "kyc_status": "VERIFIED",
                    &config.document_type: {
                        "status": "APPROVED",
                        "verified_at": chrono::Utc::now().to_rfc3339()
                    }
                }
            }));
        } else {
            updates.user_metadata_patch = Some(json!({
                "kyc": {
                    "kyc_status": "REJECTED",
                    &config.document_type: {
                        "status": "REJECTED",
                        "rejection_reason": notes
                    }
                }
            }));
        }

        Ok(StepOutcome::Done {
            output: Some(json!({
                "reviewed": true,
                "approved": approved,
                "document_type": config.document_type
            })),
            updates: Some(Box::new(updates)),
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ValidateDepositConfig {
    #[serde(default = "default_min_amount")]
    pub min_amount: f64,

    #[serde(default = "default_max_amount")]
    pub max_amount: f64,

    #[serde(default = "default_currencies")]
    pub currencies: Vec<String>,
}

fn default_min_amount() -> f64 {
    1.0
}

fn default_max_amount() -> f64 {
    100000.0
}

fn default_currencies() -> Vec<String> {
    vec!["USD".to_string(), "EUR".to_string(), "XAF".to_string()]
}

impl Default for ValidateDepositConfig {
    fn default() -> Self {
        Self {
            min_amount: default_min_amount(),
            max_amount: default_max_amount(),
            currencies: default_currencies(),
        }
    }
}

pub struct ValidateDepositAction;

#[async_trait]
impl Step for ValidateDepositAction {
    fn step_type(&self) -> &'static str {
        "VALIDATE_DEPOSIT"
    }

    fn actor(&self) -> Actor {
        Actor::System
    }

    fn human_id(&self) -> &'static str {
        "validate_deposit"
    }

    async fn execute(&self, ctx: &StepContext) -> Result<StepOutcome, FlowError> {
        tracing::debug!(step = self.step_type(), "Executing step");
        let config: ValidateDepositConfig = super::parse_step_config(ctx)?;

        let amount = ctx
            .input
            .get("amount")
            .or_else(|| ctx.session_config("deposit_amount"))
            .and_then(|v| v.as_f64())
            .or_else(|| {
                ctx.input
                    .get("amount")
                    .or_else(|| ctx.session_config("deposit_amount"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
            });

        let currency = ctx
            .input
            .get("currency")
            .or_else(|| ctx.session_config("deposit_currency"))
            .and_then(|v| v.as_str());

        match (amount, currency) {
            (Some(amt), Some(curr)) => {
                let is_valid = amt >= config.min_amount
                    && amt <= config.max_amount
                    && config
                        .currencies
                        .iter()
                        .any(|c| c.eq_ignore_ascii_case(curr));

                tracing::info!(
                    "[VALIDATE_DEPOSIT] amount={}, currency={}, valid={}",
                    amt,
                    curr,
                    is_valid
                );

                if is_valid {
                    Ok(StepOutcome::Done {
                        output: Some(json!({
                            "valid": true,
                            "amount": amt,
                            "currency": curr
                        })),
                        updates: None,
                    })
                } else {
                    let reason = if amt < config.min_amount {
                        format!("Amount below minimum ({})", config.min_amount)
                    } else if amt > config.max_amount {
                        format!("Amount above maximum ({})", config.max_amount)
                    } else {
                        format!("Currency {} not supported", curr)
                    };

                    Ok(StepOutcome::Failed {
                        error: reason,
                        retryable: true,
                    })
                }
            }
            (None, _) => Err(FlowError::InvalidDefinition("Missing amount".to_string())),
            (_, None) => Err(FlowError::InvalidDefinition("Missing currency".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StepServices;
    use serde_json::json;
    use std::collections::HashMap;

    fn make_ctx(config: HashMap<String, serde_json::Value>) -> StepContext {
        StepContext {
            session_id: "test".to_string(),
            session_user_id: None,
            flow_id: "test-flow".to_string(),
            step_id: "kyc-step".to_string(),
            input: json!({}),
            session_context: json!({}),
            flow_context: json!({}),
            services: StepServices {
                config: Some(config),
                ..Default::default()
            },
        }
    }

    fn make_ctx_with_input(
        config: HashMap<String, serde_json::Value>,
        input: serde_json::Value,
    ) -> StepContext {
        StepContext {
            session_id: "test".to_string(),
            session_user_id: None,
            flow_id: "test-flow".to_string(),
            step_id: "kyc-step".to_string(),
            input,
            session_context: json!({}),
            flow_context: json!({}),
            services: StepServices {
                config: Some(config),
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn upload_document_waits_for_user() {
        let action = UploadDocumentAction;
        let mut config = HashMap::new();
        config.insert("document_type".to_string(), json!("id"));
        let ctx = make_ctx(config);

        let result = action.execute(&ctx).await.unwrap();

        match result {
            StepOutcome::Waiting { actor } => {
                assert_eq!(actor, Actor::EndUser);
            }
            _ => panic!("Expected Waiting outcome"),
        }
    }

    #[tokio::test]
    async fn review_document_waits_for_admin() {
        let action = ReviewDocumentAction;
        let mut config = HashMap::new();
        config.insert("document_type".to_string(), json!("id"));
        config.insert("on_approve".to_string(), json!("next_step"));
        config.insert("on_reject".to_string(), json!("failed"));
        let ctx = make_ctx(config);

        let result = action.execute(&ctx).await.unwrap();

        match result {
            StepOutcome::Waiting { actor } => {
                assert_eq!(actor, Actor::Admin);
            }
            _ => panic!("Expected Waiting outcome"),
        }
    }

    #[tokio::test]
    async fn review_document_approves() {
        let action = ReviewDocumentAction;
        let mut config = HashMap::new();
        config.insert("document_type".to_string(), json!("id"));
        let ctx = make_ctx(config);

        let input = json!({
            "approved": true,
            "notes": "Document verified"
        });

        let result = action.verify_input(&ctx, &input).await.unwrap();

        match result {
            StepOutcome::Done { output, updates } => {
                let output = output.unwrap();
                assert_eq!(output["approved"], true);

                let updates = updates.unwrap();
                let user_patch = updates.user_metadata_patch.unwrap();
                // Check the nested structure: kyc.kyc_status
                assert_eq!(user_patch["kyc"]["kyc_status"], "VERIFIED");
            }
            _ => panic!("Expected Done outcome"),
        }
    }

    #[tokio::test]
    async fn review_document_rejects() {
        let action = ReviewDocumentAction;
        let mut config = HashMap::new();
        config.insert("document_type".to_string(), json!("address"));
        let ctx = make_ctx(config);

        let input = json!({
            "approved": false,
            "notes": "Document expired"
        });

        let result = action.verify_input(&ctx, &input).await.unwrap();

        match result {
            StepOutcome::Done { output, updates } => {
                let output = output.unwrap();
                assert_eq!(output["approved"], false);

                let updates = updates.unwrap();
                let user_patch = updates.user_metadata_patch.unwrap();
                assert_eq!(user_patch["kyc"]["kyc_status"], "REJECTED");
            }
            _ => panic!("Expected Done outcome"),
        }
    }

    #[tokio::test]
    async fn validate_deposit_accepts_valid() {
        let action = ValidateDepositAction;
        let mut config = HashMap::new();
        config.insert("min_amount".to_string(), json!(1.0));
        config.insert("max_amount".to_string(), json!(100000.0));
        config.insert("currencies".to_string(), json!(["USD", "EUR", "XAF"]));

        let ctx = make_ctx_with_input(
            config,
            json!({
                "amount": 100.0,
                "currency": "USD"
            }),
        );

        let result = action.execute(&ctx).await.unwrap();

        match result {
            StepOutcome::Done { output, .. } => {
                let output = output.unwrap();
                assert_eq!(output["valid"], true);
                assert_eq!(output["amount"], 100.0);
                assert_eq!(output["currency"], "USD");
            }
            _ => panic!("Expected Done outcome"),
        }
    }

    #[tokio::test]
    async fn validate_deposit_rejects_below_min() {
        let action = ValidateDepositAction;
        let ctx = make_ctx_with_input(
            HashMap::new(),
            json!({
                "amount": 0.50,
                "currency": "USD"
            }),
        );

        let result = action.execute(&ctx).await.unwrap();

        match result {
            StepOutcome::Failed { error, retryable } => {
                assert!(error.contains("below minimum"));
                assert!(retryable);
            }
            _ => panic!("Expected Failed outcome"),
        }
    }

    #[tokio::test]
    async fn validate_deposit_rejects_invalid_currency() {
        let action = ValidateDepositAction;
        let ctx = make_ctx_with_input(
            HashMap::new(),
            json!({
                "amount": 100.0,
                "currency": "GBP"
            }),
        );

        let result = action.execute(&ctx).await.unwrap();

        match result {
            StepOutcome::Failed { error, .. } => {
                assert!(error.contains("not supported"));
            }
            _ => panic!("Expected Failed outcome"),
        }
    }

    #[tokio::test]
    async fn validate_deposit_uses_defaults() {
        let action = ValidateDepositAction;
        let ctx = make_ctx_with_input(
            HashMap::new(),
            json!({
                "amount": 50.0,
                "currency": "EUR"
            }),
        );

        let result = action.execute(&ctx).await.unwrap();

        match result {
            StepOutcome::Done { output, .. } => {
                let output = output.unwrap();
                assert_eq!(output["valid"], true);
            }
            _ => panic!("Expected Done outcome"),
        }
    }
}
