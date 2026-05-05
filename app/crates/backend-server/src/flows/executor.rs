use super::runtime::{merge_json_value, resolve_transition, step_services};
use crate::state::AppState;
use backend_core::Error;
use backend_flow_sdk::{Actor, HumanReadableId, RetryConfig, StepContext, StepOutcome};
use backend_repository::FlowStepPatch;
use chrono::Utc;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, instrument, warn};

pub struct FlowExecutor {
    state: Arc<AppState>,
}

impl FlowExecutor {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    #[instrument(skip(self, step))]
    pub async fn process_flow_step(
        &self,
        step: backend_model::db::FlowStepRow,
    ) -> Result<(), Error> {
        debug!(
            "Processing step: {} for flow: {} (attempt {})",
            step.step_type, step.flow_id, step.attempt_no
        );
        let flow = self
            .state
            .flow
            .get_flow(&step.flow_id)
            .await?
            .ok_or_else(|| Error::internal("FLOW_NOT_FOUND", "Flow not found for step"))?;

        let session = self
            .state
            .flow
            .get_session(&flow.session_id)
            .await?
            .ok_or_else(|| Error::internal("SESSION_NOT_FOUND", "Session not found for flow"))?;

        let flow_def = self
            .state
            .flow_registry
            .get_flow(&flow.flow_type)
            .ok_or_else(|| {
                Error::internal(
                    "UNKNOWN_FLOW_TYPE",
                    format!("Unknown flow type: {}", flow.flow_type),
                )
            })?;

        let flow_definition = self
            .state
            .flow_registry
            .get_flow_definition(&flow.flow_type);
        let retry_config = flow_definition
            .map(|fd| fd.get_step_retry_config(&step.step_type))
            .unwrap_or_default();

        let step_def = flow_def
            .steps()
            .iter()
            .find(|s| s.step_type() == step.step_type)
            .ok_or_else(|| {
                Error::internal(
                    "UNKNOWN_STEP_TYPE",
                    format!("Unknown step type: {}", step.step_type),
                )
            })?;

        let context = StepContext {
            session_id: session.id.clone(),
            session_user_id: session.user_id.clone(),
            flow_id: flow.id.clone(),
            step_id: step.id.clone(),
            input: step.input.clone().unwrap_or_else(|| serde_json::json!({})),
            session_context: session.context.clone(),
            flow_context: flow.context.clone(),
            services: step_services(self.state.user.clone()),
        };

        let outcome = step_def
            .execute(&context)
            .await
            .map_err(|e| Error::internal("STEP_EXECUTION_FAILED", e.to_string()))?;

        match outcome {
            StepOutcome::Done { output, updates } => {
                self.handle_done(&step, &flow, &session, output, updates, flow_def, None)
                    .await?;
            }
            StepOutcome::Branched {
                branch,
                output,
                updates,
            } => {
                self.handle_done(
                    &step,
                    &flow,
                    &session,
                    output,
                    updates,
                    flow_def,
                    Some(branch),
                )
                .await?;
            }
            StepOutcome::Waiting { .. } => {
                debug!("Step execution waiting: {}", step.step_type);
                self.state
                    .flow
                    .patch_step(&step.id, FlowStepPatch::new().status("WAITING"))
                    .await?;
            }
            StepOutcome::Failed { error, retryable } => {
                self.handle_failed(&step, &flow, &session, error, retryable, &retry_config)
                    .await?;
            }
            StepOutcome::Retry { after } => {
                self.handle_retry(&step, &flow, after, &retry_config)
                    .await?;
            }
        }

        Ok(())
    }

    async fn handle_done(
        &self,
        step: &backend_model::db::FlowStepRow,
        flow: &backend_model::db::FlowInstanceRow,
        session: &backend_model::db::FlowSessionRow,
        output: Option<Value>,
        updates: Option<Box<backend_flow_sdk::ContextUpdates>>,
        flow_def: &dyn backend_flow_sdk::Flow,
        branch: Option<String>,
    ) -> Result<(), Error> {
        debug!("Step execution done: {}", step.step_type);
        let actual_output = output.unwrap_or_else(|| serde_json::json!({"result": "done"}));

        self.state
            .flow
            .patch_step(
                &step.id,
                FlowStepPatch::new()
                    .status("COMPLETED")
                    .output(actual_output.clone())
                    .clear_error()
                    .finished_at(Utc::now()),
            )
            .await?;

        let mut next_flow_context = flow.context.clone();
        if let Some(root) = next_flow_context.as_object_mut() {
            let entry = root
                .entry("step_output")
                .or_insert_with(|| Value::Object(Default::default()));
            if let Some(step_map) = entry.as_object_mut() {
                step_map.insert(step.step_type.clone(), actual_output);
            }
        }

        if let Some(updates) = updates {
            self.apply_updates(session, &mut next_flow_context, *updates)
                .await?;
        }

        self.advance_to_next_step(
            flow,
            session,
            step,
            flow_def,
            next_flow_context,
            branch.as_deref(),
            false,
        )
        .await?;
        Ok(())
    }

    async fn apply_updates(
        &self,
        session: &backend_model::db::FlowSessionRow,
        next_flow_context: &mut Value,
        updates: backend_flow_sdk::ContextUpdates,
    ) -> Result<(), Error> {
        let backend_flow_sdk::ContextUpdates {
            flow_context_patch,
            session_context_patch,
            user_metadata_patch,
            user_metadata_eager_patch,
            notifications,
        } = updates;

        if let Some(flow_patch) = flow_context_patch {
            merge_json_value(next_flow_context, &flow_patch);
        }

        if let Some(session_patch) = session_context_patch {
            let mut new_session_context = session.context.clone();
            merge_json_value(&mut new_session_context, &session_patch);
            self.state
                .flow
                .update_session_context(&session.id, new_session_context)
                .await?;
        }

        if let Some(metadata_patch) = user_metadata_patch
            && let Some(user_id) = session.user_id.as_deref()
        {
            self.state
                .user
                .update_metadata(user_id, metadata_patch, user_metadata_eager_patch)
                .await?;
        }

        if let Some(notifications) = notifications {
            for notification in notifications {
                match serde_json::from_value::<backend_core::NotificationJob>(notification.clone())
                {
                    Ok(job) => {
                        if let Err(e) = self.state.notification_queue.enqueue(job).await {
                            warn!("Failed to enqueue notification: {}", e);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to deserialize notification job: {}", e);
                    }
                }
            }
        }
        Ok(())
    }

    async fn advance_to_next_step(
        &self,
        flow: &backend_model::db::FlowInstanceRow,
        session: &backend_model::db::FlowSessionRow,
        step: &backend_model::db::FlowStepRow,
        flow_def: &dyn backend_flow_sdk::Flow,
        next_flow_context: Value,
        branch: Option<&str>,
        failed: bool,
    ) -> Result<(), Error> {
        if let Some(next_step_type) = resolve_transition(flow_def, &step.step_type, branch, failed)
        {
            if !Self::is_terminal_step(&next_step_type) {
                let next_step_def = flow_def
                    .steps()
                    .iter()
                    .find(|s| s.step_type() == next_step_type)
                    .ok_or_else(|| {
                        Error::internal(
                            "NEXT_STEP_NOT_FOUND",
                            format!("Next step not found: {}", next_step_type),
                        )
                    })?;

                let existing_steps = self.state.flow.list_steps_for_flow(&flow.id).await?;
                let next_attempt = existing_steps
                    .iter()
                    .filter(|existing| existing.step_type == next_step_type)
                    .count();
                let next_human_suffix = if next_attempt == 0 {
                    next_step_type.to_owned()
                } else {
                    format!("{}-{}", next_step_type, next_attempt)
                };
                let next_step_human_id = HumanReadableId::parse(flow.human_id.clone())
                    .map_err(|error| Error::internal("INVALID_FLOW_HUMAN_ID", error.to_string()))?
                    .with_suffix(&next_human_suffix)
                    .map_err(|error| Error::internal("INVALID_STEP_HUMAN_ID", error.to_string()))?
                    .to_string();

                let created_step = self
                    .state
                    .flow
                    .create_step(backend_repository::FlowStepCreateInput {
                        id: backend_core::id::flow_step_id()?,
                        human_id: next_step_human_id,
                        flow_id: flow.id.clone(),
                        step_type: next_step_type.to_string(),
                        actor: next_step_def.actor().to_string(),
                        status: "WAITING".to_owned(),
                        attempt_no: 1,
                        input: None,
                        output: None,
                        error: None,
                        next_retry_at: if matches!(next_step_def.actor(), Actor::System) {
                            Some(Utc::now())
                        } else {
                            None
                        },
                        finished_at: None,
                    })
                    .await?;

                let mut updated_step_ids = flow.step_ids.clone();
                if let Some(arr) = updated_step_ids.as_array_mut() {
                    arr.push(serde_json::json!(created_step.id));
                }

                self.state
                    .flow
                    .update_flow(
                        &flow.id,
                        Some("RUNNING".to_owned()),
                        Some(Some(next_step_type.to_string())),
                        Some(updated_step_ids),
                        Some(next_flow_context),
                    )
                    .await?;
            } else {
                let final_status = Self::terminal_status_for(&next_step_type);
                self.finalize_flow(&flow.id, Some(&session.id), final_status)
                    .await?;
            }
        } else {
            let final_status = if failed { "FAILED" } else { "COMPLETED" };
            self.finalize_flow(&flow.id, Some(&session.id), final_status)
                .await?;
        }
        Ok(())
    }

    fn is_terminal_step(step_type: &str) -> bool {
        step_type.eq_ignore_ascii_case("FAILED")
            || step_type.eq_ignore_ascii_case("END")
            || step_type.eq_ignore_ascii_case("COMPLETE")
            || step_type.eq_ignore_ascii_case("COMPLETED")
            || step_type.eq_ignore_ascii_case("CLOSED")
    }

    fn terminal_status_for(step_type: &str) -> &'static str {
        if step_type.eq_ignore_ascii_case("FAILED") {
            "FAILED"
        } else if step_type.eq_ignore_ascii_case("CLOSED") {
            "CLOSED"
        } else {
            "COMPLETED"
        }
    }

    async fn handle_failed(
        &self,
        step: &backend_model::db::FlowStepRow,
        flow: &backend_model::db::FlowInstanceRow,
        session: &backend_model::db::FlowSessionRow,
        error: String,
        retryable: bool,
        retry_config: &RetryConfig,
    ) -> Result<(), Error> {
        info!(
            "Step execution failed: {} (error={}, retryable={}, attempt={}/{})",
            step.step_type, error, retryable, step.attempt_no, retry_config.max
        );

        if retryable && step.attempt_no < retry_config.max {
            let delay = Duration::from_millis(retry_config.delay_ms);
            self.schedule_retry(&step.id, step.attempt_no + 1, delay)
                .await?;
            debug!(
                "Scheduled retry {} for step {} after {}ms",
                step.attempt_no + 1,
                step.step_type,
                retry_config.delay_ms
            );
        } else {
            self.state
                .flow
                .patch_step(
                    &step.id,
                    FlowStepPatch::new()
                        .status("FAILED")
                        .error(serde_json::json!({"error": error, "retryable": retryable, "attempts": step.attempt_no}))
                        .finished_at(Utc::now()),
                )
                .await?;
            let flow_def = self
                .state
                .flow_registry
                .get_flow(&flow.flow_type)
                .ok_or_else(|| Error::internal("UNKNOWN_FLOW_TYPE", "Flow not found"))?;
            self.advance_to_next_step(
                flow,
                session,
                step,
                flow_def,
                flow.context.clone(),
                None,
                true,
            )
            .await?;
        }
        Ok(())
    }

    async fn handle_retry(
        &self,
        step: &backend_model::db::FlowStepRow,
        flow: &backend_model::db::FlowInstanceRow,
        after: Duration,
        retry_config: &RetryConfig,
    ) -> Result<(), Error> {
        debug!(
            "Step execution retry requested: {} (after={:?}, attempt={}/{})",
            step.step_type, after, step.attempt_no, retry_config.max
        );

        if step.attempt_no < retry_config.max {
            let delay = after.max(Duration::from_millis(retry_config.delay_ms));
            self.schedule_retry(&step.id, step.attempt_no + 1, delay)
                .await?;
        } else {
            warn!(
                "Max retries exceeded for step {} (attempts={})",
                step.step_type, step.attempt_no
            );
            self.state
                .flow
                .patch_step(
                    &step.id,
                    FlowStepPatch::new()
                        .status("FAILED")
                        .error(serde_json::json!({"error": "max_retries_exceeded", "attempts": step.attempt_no}))
                        .finished_at(Utc::now()),
                )
                .await?;
            self.finalize_flow(&flow.id, None, "FAILED").await?;
        }
        Ok(())
    }

    async fn schedule_retry(
        &self,
        step_id: &str,
        next_attempt: i32,
        delay: Duration,
    ) -> Result<(), Error> {
        use chrono::Duration as ChronoDuration;
        let next_retry_at = Utc::now()
            + ChronoDuration::from_std(delay).unwrap_or_else(|_| ChronoDuration::seconds(1));

        self.state
            .flow
            .patch_step(
                step_id,
                FlowStepPatch::new()
                    .status("WAITING")
                    .attempt_no(next_attempt)
                    .next_retry_at(next_retry_at),
            )
            .await?;
        Ok(())
    }

    async fn finalize_flow(
        &self,
        flow_id: &str,
        session_id: Option<&str>,
        status: &str,
    ) -> Result<(), Error> {
        let flow = self
            .state
            .flow
            .get_flow(flow_id)
            .await?
            .ok_or_else(|| Error::internal("FLOW_NOT_FOUND", "Flow not found"))?;

        self.state
            .flow
            .update_flow(flow_id, Some(status.to_owned()), Some(None), None, None)
            .await?;

        if status.eq_ignore_ascii_case("COMPLETED") {
            self.write_completed_kyc_metadata(&flow).await?;
        }

        if let Some(sid) = session_id {
            self.state
                .flow
                .update_session_status(sid, status, Some(Utc::now()))
                .await?;
        }
        Ok(())
    }

    async fn write_completed_kyc_metadata(
        &self,
        flow: &backend_model::db::FlowInstanceRow,
    ) -> Result<(), Error> {
        let session = self.state.flow.get_session(&flow.session_id).await?;
        let Some(session) = session else {
            return Ok(());
        };
        let Some(user_id) = session.user_id.as_deref() else {
            return Ok(());
        };
        let session_type = session.session_type.clone();
        let flow_type = flow.flow_type.clone();
        let session_id = session.id.clone();
        let flow_id = flow.id.clone();

        self.state
            .user
            .update_metadata(
                user_id,
                serde_json::json!({
                    "kyc": {
                        session_type: {
                            flow_type: {
                                "completed": true,
                                "completed_at": Utc::now().to_rfc3339(),
                                "flow_id": flow_id,
                                "session_id": session_id
                            }
                        }
                    }
                }),
                Some(serde_json::json!({ "kyc": false })),
            )
            .await
    }
}
