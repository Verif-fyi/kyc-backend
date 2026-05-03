use crate::step::ContextUpdates;
use crate::{Actor, FlowError, Step, StepContext, StepOutcome};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Deserialize)]
pub struct UpgradeFullNameConfig {
    #[serde(default = "default_require_decision")]
    pub require_decision: bool,
    #[serde(default)]
    pub source_step_output: Option<String>,
}

impl Default for UpgradeFullNameConfig {
    fn default() -> Self {
        Self {
            require_decision: default_require_decision(),
            source_step_output: None,
        }
    }
}

fn default_require_decision() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
struct UpgradeFullNameInput {
    #[serde(default)]
    decision: Option<String>,
    #[serde(default, alias = "fullName", alias = "fullname")]
    full_name: Option<String>,
    #[serde(default, alias = "validatedDepositViaWhatsapp")]
    validated_deposit_via_whatsapp: Option<bool>,
    #[serde(default, alias = "validatedIdentityViaWhatsapp")]
    validated_identity_via_whatsapp: Option<bool>,
    #[serde(default, alias = "validatedDepositedAmount")]
    validated_deposited_amount: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
enum AdminDecision {
    Approved,
    Rejected,
}

impl AdminDecision {
    fn parse(raw: &str) -> Option<Self> {
        if raw.eq_ignore_ascii_case("APPROVED") {
            Some(Self::Approved)
        } else if raw.eq_ignore_ascii_case("REJECTED") {
            Some(Self::Rejected)
        } else {
            None
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "APPROVED",
            Self::Rejected => "REJECTED",
        }
    }
}

#[derive(Debug, Clone)]
struct NormalizedInput {
    decision: Option<AdminDecision>,
    full_name: Option<String>,
    validated_deposit_via_whatsapp: bool,
    validated_identity_via_whatsapp: bool,
    validated_deposited_amount: Option<f64>,
}

pub struct UpgradeFullNameAction;

#[async_trait]
impl Step for UpgradeFullNameAction {
    fn step_type(&self) -> &'static str {
        "UPGRADE_FULL_NAME"
    }

    fn actor(&self) -> Actor {
        Actor::System
    }

    fn human_id(&self) -> &'static str {
        "upgrade_full_name"
    }

    async fn execute(&self, ctx: &StepContext) -> Result<StepOutcome, FlowError> {
        let config: UpgradeFullNameConfig = super::parse_step_config(ctx)?;
        if let Some(source_step) = config.source_step_output.as_deref() {
            let input = ctx.step_output_pointer(source_step, "").ok_or_else(|| {
                FlowError::InvalidDefinition(format!(
                    "UPGRADE_FULL_NAME requires step_output for source step `{source_step}`"
                ))
            })?;

            let normalized = normalize_input(
                input,
                config.require_decision,
                session_full_name(&ctx.session_context),
            )?;
            persist_full_name(ctx, normalized.full_name.as_deref()).await?;
            return Ok(done_outcome(normalized));
        }

        Ok(StepOutcome::Waiting {
            actor: Actor::Admin,
        })
    }

    async fn validate_input(&self, input: &Value) -> Result<(), FlowError> {
        if !input.is_object() {
            return Err(FlowError::InvalidDefinition(
                "UPGRADE_FULL_NAME expects object input".to_owned(),
            ));
        }

        normalize_input(input, false, None)?;
        Ok(())
    }

    async fn verify_input(
        &self,
        ctx: &StepContext,
        input: &Value,
    ) -> Result<StepOutcome, FlowError> {
        let config: UpgradeFullNameConfig = super::parse_step_config(ctx)?;
        let normalized = normalize_input(
            input,
            config.require_decision,
            session_full_name(&ctx.session_context),
        )?;

        Ok(done_outcome(normalized))
    }
}

fn done_outcome(normalized: NormalizedInput) -> StepOutcome {
    let mut session_patch = Map::new();
    let mut flow_patch = Map::new();

    if let Some(full_name) = normalized.full_name.as_ref() {
        session_patch.insert("full_name".to_owned(), Value::String(full_name.clone()));
        flow_patch.insert("full_name".to_owned(), Value::String(full_name.clone()));
    }

    if let Some(amount) = normalized.validated_deposited_amount {
        // Save to flow context as "amount" so downstream steps/webhooks read the validated amount
        flow_patch.insert("amount".to_owned(), json!(amount));
        // Save to session context as "deposit_amount" for broader availability
        session_patch.insert("deposit_amount".to_owned(), json!(amount));
        session_patch.insert("amount".to_owned(), json!(amount));
    }

    flow_patch.insert(
        "validated_deposit_via_whatsapp".to_owned(),
        Value::Bool(normalized.validated_deposit_via_whatsapp),
    );
    flow_patch.insert(
        "validated_identity_via_whatsapp".to_owned(),
        Value::Bool(normalized.validated_identity_via_whatsapp),
    );
    session_patch.insert(
        "validated_deposit_via_whatsapp".to_owned(),
        Value::Bool(normalized.validated_deposit_via_whatsapp),
    );
    session_patch.insert(
        "validated_identity_via_whatsapp".to_owned(),
        Value::Bool(normalized.validated_identity_via_whatsapp),
    );

    StepOutcome::Done {
        output: Some(json!({
            "decision": normalized.decision.map(|decision| decision.as_str()),
            "full_name": normalized.full_name,
            "validated_deposit_via_whatsapp": normalized.validated_deposit_via_whatsapp,
            "validated_identity_via_whatsapp": normalized.validated_identity_via_whatsapp,
            "validated_deposited_amount": normalized.validated_deposited_amount
        })),
        updates: Some(Box::new(ContextUpdates {
            session_context_patch: Some(Value::Object(session_patch)),
            flow_context_patch: Some(Value::Object(flow_patch)),
            ..Default::default()
        })),
    }
}

async fn persist_full_name(ctx: &StepContext, full_name: Option<&str>) -> Result<(), FlowError> {
    let Some(full_name) = full_name else {
        return Ok(());
    };

    let user_id = ctx.session_user_id.as_deref().ok_or_else(|| {
        FlowError::InvalidDefinition("UPGRADE_FULL_NAME requires session user id".to_owned())
    })?;
    let service = ctx.services.user_contact.as_ref().ok_or_else(|| {
        FlowError::InvalidDefinition("UPGRADE_FULL_NAME requires user contact service".to_owned())
    })?;

    service
        .update_full_name(user_id, full_name)
        .await
        .map_err(FlowError::InvalidDefinition)
}

fn normalize_input(
    input: &Value,
    require_decision: bool,
    fallback_full_name: Option<&str>,
) -> Result<NormalizedInput, FlowError> {
    let parsed: UpgradeFullNameInput = serde_json::from_value(input.clone()).map_err(|error| {
        FlowError::InvalidDefinition(format!("UPGRADE_FULL_NAME invalid input: {error}"))
    })?;

    let decision = match parsed.decision.as_deref().map(str::trim) {
        Some(raw) if !raw.is_empty() => AdminDecision::parse(raw).ok_or_else(|| {
            FlowError::InvalidDefinition("decision must be one of APPROVED or REJECTED".to_owned())
        })?,
        Some(_) | None if require_decision => {
            return Err(FlowError::InvalidDefinition(
                "decision is required".to_owned(),
            ));
        }
        _ => {
            return Ok(NormalizedInput {
                decision: None,
                full_name: parsed
                    .full_name
                    .as_deref()
                    .and_then(trim_non_empty)
                    .or_else(|| fallback_full_name.and_then(trim_non_empty)),
                validated_deposit_via_whatsapp: parsed
                    .validated_deposit_via_whatsapp
                    .unwrap_or(false),
                validated_identity_via_whatsapp: parsed
                    .validated_identity_via_whatsapp
                    .unwrap_or(false),
                validated_deposited_amount: parsed.validated_deposited_amount,
            });
        }
    };

    let full_name = match parsed.full_name {
        Some(full_name) => {
            let Some(value) = trim_non_empty(full_name.as_str()) else {
                return Err(FlowError::InvalidDefinition(
                    "full_name must not be empty when provided".to_owned(),
                ));
            };
            Some(value)
        }
        None => fallback_full_name.and_then(trim_non_empty),
    };

    if matches!(decision, AdminDecision::Approved) && full_name.is_none() {
        return Err(FlowError::InvalidDefinition(
            "full_name is required for APPROVED decision".to_owned(),
        ));
    }

    if matches!(decision, AdminDecision::Approved) && parsed.validated_deposited_amount.is_none() {
        return Err(FlowError::InvalidDefinition(
            "validated_deposited_amount is required for APPROVED decision".to_owned(),
        ));
    }

    Ok(NormalizedInput {
        decision: Some(decision),
        full_name,
        validated_deposit_via_whatsapp: parsed.validated_deposit_via_whatsapp.unwrap_or(false),
        validated_identity_via_whatsapp: parsed.validated_identity_via_whatsapp.unwrap_or(false),
        validated_deposited_amount: parsed.validated_deposited_amount,
    })
}

fn session_full_name(session_context: &Value) -> Option<&str> {
    session_context
        .get("full_name")
        .or_else(|| session_context.get("fullname"))
        .and_then(Value::as_str)
}

fn trim_non_empty(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StepServices, UserContactService};
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default)]
    struct TestContactService {
        full_name_calls: Arc<Mutex<Vec<(String, String)>>>,
    }

    #[async_trait]
    impl UserContactService for TestContactService {
        async fn update_phone_number(
            &self,
            _user_id: &str,
            _phone_number: &str,
        ) -> Result<(), String> {
            Ok(())
        }

        async fn update_full_name(&self, user_id: &str, full_name: &str) -> Result<(), String> {
            let mut calls = self
                .full_name_calls
                .lock()
                .map_err(|_| "lock poisoned".to_owned())?;
            calls.push((user_id.to_owned(), full_name.to_owned()));
            Ok(())
        }
    }

    fn make_ctx(
        config: HashMap<String, Value>,
        session_context: Value,
        user_contact: Option<Arc<dyn UserContactService>>,
    ) -> StepContext {
        StepContext {
            session_id: "sess-1".to_owned(),
            session_user_id: Some("usr-1".to_owned()),
            flow_id: "flow-1".to_owned(),
            step_id: "step-1".to_owned(),
            input: json!({}),
            session_context,
            flow_context: json!({}),
            services: StepServices {
                config: Some(config),
                user_contact,
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn verifies_and_updates_full_name_and_whatsapp_flags() {
        let action = UpgradeFullNameAction;
        let ctx = make_ctx(HashMap::new(), json!({ "full_name": "Old Name" }), None);
        let input = json!({
            "decision": "APPROVED",
            "full_name": "  New Name  ",
            "validated_deposit_via_whatsapp": true,
            "validated_identity_via_whatsapp": false,
            "validated_deposited_amount": 500.0
        });

        let outcome = action.verify_input(&ctx, &input).await.unwrap();
        match outcome {
            StepOutcome::Done { output, updates } => {
                let output = output.expect("output");
                assert_eq!(output["decision"], "APPROVED");
                assert_eq!(output["full_name"], "New Name");
                assert_eq!(output["validated_deposit_via_whatsapp"], true);
                assert_eq!(output["validated_identity_via_whatsapp"], false);
                assert_eq!(output["validated_deposited_amount"], 500.0);

                let updates = updates.expect("updates");
                assert_eq!(
                    updates.session_context_patch.expect("session patch")["full_name"],
                    "New Name"
                );
                assert_eq!(
                    updates.flow_context_patch.expect("flow patch")["full_name"],
                    "New Name"
                );
            }
            _ => panic!("expected done outcome"),
        }
    }

    #[tokio::test]
    async fn falls_back_to_existing_session_full_name() {
        let action = UpgradeFullNameAction;
        let ctx = make_ctx(
            HashMap::new(),
            json!({ "full_name": "Existing Name" }),
            None,
        );
        let input = json!({
            "decision": "APPROVED",
            "validated_deposited_amount": 500.0
        });

        let outcome = action.verify_input(&ctx, &input).await.unwrap();
        match outcome {
            StepOutcome::Done { output, updates } => {
                let output = output.expect("output");
                assert_eq!(output["full_name"], "Existing Name");
                assert_eq!(output["validated_deposit_via_whatsapp"], false);
                assert_eq!(output["validated_identity_via_whatsapp"], false);
                assert_eq!(output["validated_deposited_amount"], 500.0);

                let updates = updates.expect("updates");
                assert_eq!(
                    updates.flow_context_patch.expect("flow patch")["validated_deposit_via_whatsapp"],
                    false
                );
            }
            _ => panic!("expected done outcome"),
        }
    }

    #[tokio::test]
    async fn rejects_invalid_decision() {
        let action = UpgradeFullNameAction;
        let input = json!({
            "decision": "MAYBE"
        });
        let err = action.validate_input(&input).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("decision must be one of APPROVED or REJECTED")
        );
    }

    #[tokio::test]
    async fn rejects_approved_decision_without_amount() {
        let action = UpgradeFullNameAction;
        let input = json!({
            "decision": "APPROVED",
            "full_name": "New Name"
        });
        let err = action.validate_input(&input).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("validated_deposited_amount is required for APPROVED decision")
        );
    }

    #[tokio::test]
    async fn execute_can_read_input_from_previous_step_output() {
        let action = UpgradeFullNameAction;
        let tracker = Arc::new(TestContactService::default());
        let mut config = HashMap::new();
        config.insert(
            "source_step_output".to_owned(),
            json!("await_admin_decision"),
        );

        let ctx = StepContext {
            session_id: "sess-1".to_owned(),
            session_user_id: Some("usr-1".to_owned()),
            flow_id: "flow-1".to_owned(),
            step_id: "step-1".to_owned(),
            input: json!({}),
            session_context: json!({ "full_name": "Old Name" }),
            flow_context: json!({
                "step_output": {
                    "await_admin_decision": {
                        "decision": "APPROVED",
                        "full_name": "Updated Name",
                        "validated_deposit_via_whatsapp": true,
                        "validated_identity_via_whatsapp": true,
                        "validated_deposited_amount": 500.0
                    }
                }
            }),
            services: StepServices {
                config: Some(config),
                user_contact: Some(tracker.clone()),
                ..Default::default()
            },
        };

        let outcome = action.execute(&ctx).await.unwrap();
        match outcome {
            StepOutcome::Done { output, .. } => {
                let output = output.expect("output");
                assert_eq!(output["full_name"], "Updated Name");
                assert_eq!(output["validated_deposit_via_whatsapp"], true);
                assert_eq!(output["validated_identity_via_whatsapp"], true);
            }
            _ => panic!("expected done outcome"),
        }

        let calls = tracker.full_name_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "usr-1");
        assert_eq!(calls[0].1, "Updated Name");
    }
}
