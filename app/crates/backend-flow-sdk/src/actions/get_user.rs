use crate::step::ContextUpdates;
use crate::{Actor, FlowError, Step, StepContext, StepOutcome};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GetUserTarget {
    Flow,
    #[default]
    Session,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GetUserConfig {
    #[serde(default)]
    pub save_to: GetUserTarget,
}

impl Default for GetUserConfig {
    fn default() -> Self {
        Self {
            save_to: GetUserTarget::Session,
        }
    }
}

pub struct GetUserAction;

#[async_trait]
impl Step for GetUserAction {
    fn step_type(&self) -> &'static str {
        "GET_USER"
    }

    fn actor(&self) -> Actor {
        Actor::System
    }

    fn human_id(&self) -> &'static str {
        "get_user"
    }

    async fn execute(&self, ctx: &StepContext) -> Result<StepOutcome, FlowError> {
        tracing::debug!(step = self.step_type(), "Executing step");
        let config: GetUserConfig = super::parse_step_config(ctx)?;
        let Some(user_id) = ctx.session_user_id.as_deref() else {
            return Ok(StepOutcome::Done {
                output: Some(json!({ "userExists": false })),
                updates: None,
            });
        };

        let service = ctx.services.user_lookup.as_ref().ok_or_else(|| {
            FlowError::InvalidDefinition("GET_USER requires user lookup service".to_owned())
        })?;

        let user = service
            .get_user(user_id)
            .await
            .map_err(FlowError::InvalidDefinition)?;

        let Some(user) = user else {
            return Ok(StepOutcome::Done {
                output: Some(json!({ "userExists": false, "userId": user_id })),
                updates: None,
            });
        };

        let patch = json!({
            "user_exists": true,
            "user_id": user.user_id,
            "username": user.username,
            "full_name": user.full_name,
            "email": user.email,
            "phone_number": user.phone_number,
            "user": {
                "user_id": user.user_id,
                "username": user.username,
                "full_name": user.full_name,
                "email": user.email,
                "phone_number": user.phone_number,
                "metadata": user.metadata,
            }
        });

        let updates = match config.save_to {
            GetUserTarget::Flow => ContextUpdates {
                flow_context_patch: Some(patch.clone()),
                ..Default::default()
            },
            GetUserTarget::Session => ContextUpdates {
                session_context_patch: Some(patch.clone()),
                ..Default::default()
            },
        };

        Ok(StepOutcome::Done {
            output: Some(json!({
                "userExists": true,
                "userId": patch["user_id"],
                "phoneNumber": patch["phone_number"],
                "fullName": patch["full_name"],
            })),
            updates: Some(Box::new(updates)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StepServices, UserLookupService, UserRecord};
    use serde_json::json;
    use std::sync::Arc;

    #[derive(Debug)]
    struct TestUserLookup;

    #[async_trait]
    impl UserLookupService for TestUserLookup {
        async fn get_user(&self, user_id: &str) -> Result<Option<UserRecord>, String> {
            Ok(Some(UserRecord {
                user_id: user_id.to_owned(),
                username: "test_user".to_owned(),
                full_name: Some("Test User".to_owned()),
                email: Some("test@example.com".to_owned()),
                phone_number: Some("+1234567890".to_owned()),
                metadata: json!({ "tier": "gold" }),
            }))
        }
    }

    #[tokio::test]
    async fn get_user_updates_session_context() {
        let action = GetUserAction;
        let ctx = StepContext {
            session_id: "sess-1".to_owned(),
            session_user_id: Some("usr-1".to_owned()),
            flow_id: "flow-1".to_owned(),
            step_id: "step-1".to_owned(),
            input: json!({}),
            session_context: json!({}),
            flow_context: json!({}),
            services: StepServices {
                user_lookup: Some(Arc::new(TestUserLookup)),
                ..Default::default()
            },
        };

        let outcome = action.execute(&ctx).await.unwrap();
        match outcome {
            StepOutcome::Done { output, updates } => {
                assert_eq!(output.unwrap()["userExists"], true);
                let patch = updates.unwrap().session_context_patch.unwrap();
                assert_eq!(patch["user_exists"], true);
                assert_eq!(patch["phone_number"], "+237690000001");
            }
            _ => panic!("expected done outcome"),
        }
    }
}
