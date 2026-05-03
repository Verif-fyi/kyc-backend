use super::models::{
    AddFlowRequest, CompletedKycResponse, CreateSessionRequest, FlowDetailResponse, FlowResponse,
    SessionDetailResponse, SessionResponse, StepResponse, SubmitStepRequest, UserResponse,
};
use crate::api::BackendApi;
use crate::flows::registry::{actor_label, waiting_status};
use crate::flows::runtime::{merge_json_value, merged_json, resolve_transition, step_services};
use axum::http::HeaderMap;
use backend_core::Error;
use backend_flow_sdk::{Actor, Flow, FlowError, HumanReadableId, StepContext, StepOutcome};
use backend_model::db::{FlowInstanceRow, FlowSessionRow};
use backend_repository::{
    FlowInstanceCreateInput, FlowSessionCreateInput, FlowSessionFilter, FlowStepCreateInput,
    FlowStepPatch,
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use tracing::{debug, info, instrument};

const FLOW_STATUS_RUNNING: &str = "RUNNING";
const FLOW_STATUS_COMPLETED: &str = "COMPLETED";
const FLOW_STATUS_FAILED: &str = "FAILED";
const FLOW_STATUS_CLOSED: &str = "CLOSED";

pub async fn require_user_id(_api: &BackendApi, headers: &HeaderMap) -> Result<String, Error> {
    let user_id = headers
        .get(crate::api::BFF_AUTH_USER_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            Error::unauthorized(
                "Invalid signature-authenticated user id",
            )
        })?;
    Ok(user_id)
}

#[instrument(skip(api))]
pub async fn create_session(
    api: &BackendApi,
    user_id: String,
    body: CreateSessionRequest,
) -> Result<SessionResponse, Error> {
    debug!("Creating session of type: {}", body.session_type);
    let session_definition = api
        .state
        .flow_registry
        .get_session(&body.session_type)
        .ok_or_else(|| {
            Error::bad_request(
                "UNKNOWN_SESSION_TYPE",
                format!("Unknown session type: {}", body.session_type),
            )
        })?;

    let session_id = backend_core::id::flow_session_id()?;
    let human_id = normalize_or_default_human_id(
        body.human_id,
        format!(
            "{}.{}.{}",
            session_definition.human_id_prefix,
            Utc::now().format("%Y-%m-%d"),
            session_id
        ),
    )?;

    let row = api
        .state
        .flow
        .create_session(FlowSessionCreateInput {
            id: session_id,
            human_id,
            user_id: Some(user_id.clone()),
            session_type: body.session_type,
            status: "OPEN".to_owned(),
            context: session_context_with_user_id(body.context, &user_id),
        })
        .await?;

    Ok(row.into())
}

#[instrument(skip(api))]
pub async fn list_sessions(
    api: &BackendApi,
    user_id: String,
) -> Result<Vec<SessionResponse>, Error> {
    debug!("Listing sessions for user: {}", user_id);
    let (rows, _) = api
        .state
        .flow
        .list_sessions(FlowSessionFilter {
            user_id: Some(user_id),
            user_ids: None,
            session_type: None,
            status: None,
            page: 1,
            limit: 100,
        })
        .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

#[instrument(skip(api))]
pub async fn get_session(
    api: &BackendApi,
    session_id: String,
    user_id: String,
) -> Result<SessionDetailResponse, Error> {
    debug!("Getting session: {}", session_id);
    let session = ensure_session_owner(api, &session_id, &user_id).await?;
    let flows = api.state.flow.list_flows_for_session(&session.id).await?;

    Ok(SessionDetailResponse {
        session: session.into(),
        flows: flows.into_iter().map(Into::into).collect(),
    })
}

pub async fn list_session_flows(
    api: &BackendApi,
    session_id: String,
    user_id: String,
) -> Result<Vec<FlowResponse>, Error> {
    ensure_session_owner(api, &session_id, &user_id).await?;
    let flows = api.state.flow.list_flows_for_session(&session_id).await?;
    Ok(flows.into_iter().map(Into::into).collect())
}

#[instrument(skip(api))]
pub async fn add_flow_to_session(
    api: &BackendApi,
    session_id: String,
    user_id: String,
    body: AddFlowRequest,
) -> Result<FlowResponse, Error> {
    debug!(
        "Adding flow `{}` to session: {}",
        body.flow_type, session_id
    );
    let session = ensure_session_owner(api, &session_id, &user_id).await?;

    let flow_definition = get_flow_definition(api, &body.flow_type)?;
    validate_session_flow_compatibility(api, &session, flow_definition.flow_type())?;

    let initial_step = body
        .initial_step
        .clone()
        .unwrap_or_else(|| flow_definition.initial_step().to_owned());

    ensure_flow_step_exists(flow_definition, &initial_step)?;

    let flow_id = backend_core::id::flow_instance_id()?;
    let flow_human_id = normalize_or_default_human_id(
        body.human_id,
        format!("{}.{}", session.human_id, flow_definition.human_id()),
    )?;

    let created = api
        .state
        .flow
        .create_flow(FlowInstanceCreateInput {
            id: flow_id,
            human_id: flow_human_id,
            session_id: session_id.clone(),
            flow_type: body.flow_type,
            status: FLOW_STATUS_RUNNING.to_owned(),
            current_step: None,
            step_ids: json!([]),
            context: object_context(body.context),
        })
        .await?;

    api.state
        .flow
        .update_session_status(&session_id, FLOW_STATUS_RUNNING, None)
        .await?;

    let advanced = create_step_chain(api, &session, created, initial_step, None).await?;
    refresh_session_status(api, &session_id).await?;

    Ok(advanced.into())
}

#[instrument(skip(api))]
pub async fn get_flow(
    api: &BackendApi,
    flow_id: String,
    user_id: String,
) -> Result<FlowDetailResponse, Error> {
    debug!("Getting flow: {}", flow_id);
    let flow = api
        .state
        .flow
        .get_flow(&flow_id)
        .await?
        .ok_or_else(|| Error::not_found("FLOW_NOT_FOUND", "Flow not found"))?;

    ensure_flow_owner(api, &flow, &user_id).await?;
    let steps = api.state.flow.list_steps_for_flow(&flow_id).await?;

    Ok(FlowDetailResponse {
        flow: flow.into(),
        steps: steps.into_iter().map(Into::into).collect(),
    })
}

#[instrument(skip(api))]
pub async fn list_flow_steps(
    api: &BackendApi,
    flow_id: String,
    user_id: String,
) -> Result<Vec<StepResponse>, Error> {
    debug!("Listing steps for flow: {}", flow_id);
    let flow = api
        .state
        .flow
        .get_flow(&flow_id)
        .await?
        .ok_or_else(|| Error::not_found("FLOW_NOT_FOUND", "Flow not found"))?;

    ensure_flow_owner(api, &flow, &user_id).await?;
    let steps = api.state.flow.list_steps_for_flow(&flow_id).await?;

    Ok(steps.into_iter().map(Into::into).collect())
}

#[instrument(skip(api))]
pub async fn get_step(
    api: &BackendApi,
    step_id: String,
    user_id: String,
) -> Result<StepResponse, Error> {
    debug!("Getting step: {}", step_id);
    let step = api
        .state
        .flow
        .get_step(&step_id)
        .await?
        .ok_or_else(|| Error::not_found("STEP_NOT_FOUND", "Step not found"))?;

    let flow = api
        .state
        .flow
        .get_flow(&step.flow_id)
        .await?
        .ok_or_else(|| Error::not_found("FLOW_NOT_FOUND", "Flow not found"))?;

    ensure_flow_owner(api, &flow, &user_id).await?;
    Ok(step.into())
}

#[instrument(skip(api))]
pub async fn submit_step(
    api: &BackendApi,
    step_id: String,
    user_id: String,
    body: SubmitStepRequest,
) -> Result<StepResponse, Error> {
    debug!("Submitting step: {}", step_id);
    let step = api
        .state
        .flow
        .get_step(&step_id)
        .await?
        .ok_or_else(|| Error::not_found("STEP_NOT_FOUND", "Step not found"))?;

    let flow = api
        .state
        .flow
        .get_flow(&step.flow_id)
        .await?
        .ok_or_else(|| Error::not_found("FLOW_NOT_FOUND", "Flow not found"))?;

    ensure_flow_owner(api, &flow, &user_id).await?;

    let flow_definition = get_flow_definition(api, &flow.flow_type)?;
    let step_definition = get_step_definition(flow_definition, &step.step_type)?;

    if matches!(step_definition.actor(), Actor::System) {
        return Err(Error::conflict(
            "SYSTEM_STEP_NOT_SUBMITTABLE",
            "System steps are executed automatically",
        ));
    }

    step_definition
        .validate_input(&body.input)
        .await
        .map_err(flow_error_to_http)?;

    let session = api
        .state
        .flow
        .get_session(&flow.session_id)
        .await?
        .ok_or_else(|| Error::not_found("SESSION_NOT_FOUND", "Session not found"))?;

    let verify_context = StepContext {
        session_id: session.id.clone(),
        session_user_id: session.user_id.clone(),
        flow_id: flow.id.clone(),
        step_id: step.id.clone(),
        input: body.input.clone(),
        session_context: session.context.clone(),
        flow_context: flow.context.clone(),
        services: step_services(api.state.user.clone()),
    };

    let verify_outcome = step_definition
        .verify_input(&verify_context, &body.input)
        .await
        .map_err(flow_error_to_http)?;

    let (output_value, context_updates, branch) = match verify_outcome {
        StepOutcome::Done { output, updates } => (
            output.unwrap_or_else(|| json!({"verified": true})),
            updates,
            None,
        ),
        StepOutcome::Branched {
            branch,
            output,
            updates,
        } => (
            output.unwrap_or_else(|| json!({"verified": true})),
            updates,
            Some(branch),
        ),
        StepOutcome::Failed {
            error,
            retryable: _,
        } => {
            return Err(Error::bad_request("VERIFICATION_FAILED", error));
        }
        _ => (json!({"verified": true}), None, None),
    };

    let mut updated_flow_context = flow.context.clone();
    if let Some(updates) = context_updates {
        if let Some(patch) = updates.flow_context_patch.as_ref() {
            updated_flow_context = merged_json(updated_flow_context, patch);
        }
        apply_context_updates(api, &session, updates).await?;
    }

    let updated_step = api
        .state
        .flow
        .patch_step(
            &step_id,
            FlowStepPatch::new()
                .status(FLOW_STATUS_COMPLETED)
                .input(body.input.clone())
                .output(output_value.clone())
                .clear_error()
                .finished_at(Utc::now()),
        )
        .await?;

    let context = store_step_output(updated_flow_context, &updated_step.step_type, &body.input);
    let mut current_flow = api
        .state
        .flow
        .update_flow(&flow.id, None, None, None, Some(context))
        .await?;

    if let Some(next_step) = resolve_transition(
        flow_definition,
        &updated_step.step_type,
        branch.as_deref(),
        false,
    ) {
        if has_flow_step(flow_definition, &next_step) {
            current_flow = create_step_chain(api, &session, current_flow, next_step, None).await?;
        } else {
            current_flow = finalize_flow(api, &current_flow, terminal_status(&next_step)).await?;
        }
    } else {
        current_flow = finalize_flow(api, &current_flow, FLOW_STATUS_COMPLETED).await?;
    }

    refresh_session_status(api, &current_flow.session_id).await?;

    Ok(updated_step.into())
}

pub(crate) fn get_flow_definition<'a>(
    api: &'a BackendApi,
    flow_type: &str,
) -> Result<&'a dyn Flow, Error> {
    api.state.flow_registry.get_flow(flow_type).ok_or_else(|| {
        Error::bad_request(
            "UNKNOWN_FLOW_TYPE",
            format!("Unknown flow type: {flow_type}"),
        )
    })
}

pub(crate) fn get_step_definition<'a>(
    flow: &'a dyn Flow,
    step_type: &str,
) -> Result<&'a dyn backend_flow_sdk::Step, Error> {
    flow.steps()
        .iter()
        .find(|step| step.step_type() == step_type)
        .map(|step| step.as_ref())
        .ok_or_else(|| {
            Error::bad_request(
                "UNKNOWN_STEP_TYPE",
                format!(
                    "Unknown step type `{step_type}` for flow `{}`",
                    flow.flow_type()
                ),
            )
        })
}

pub(crate) fn ensure_flow_step_exists(flow: &dyn Flow, step_type: &str) -> Result<(), Error> {
    if has_flow_step(flow, step_type) {
        return Ok(());
    }

    Err(Error::bad_request(
        "UNKNOWN_STEP_TYPE",
        format!(
            "Unknown step type `{step_type}` for flow `{}`",
            flow.flow_type()
        ),
    ))
}

pub(crate) fn has_flow_step(flow: &dyn Flow, step_type: &str) -> bool {
    flow.steps()
        .iter()
        .any(|step| step.step_type() == step_type)
}

pub(crate) fn terminal_status(step_type: &str) -> &'static str {
    if step_type.eq_ignore_ascii_case("FAILED") {
        FLOW_STATUS_FAILED
    } else if step_type.eq_ignore_ascii_case("CLOSED") {
        FLOW_STATUS_CLOSED
    } else {
        FLOW_STATUS_COMPLETED
    }
}

fn validate_session_flow_compatibility(
    api: &BackendApi,
    session: &FlowSessionRow,
    flow_type: &str,
) -> Result<(), Error> {
    let Some(definition) = api.state.flow_registry.get_session(&session.session_type) else {
        return Ok(());
    };

    if definition.allowed_flows.is_empty()
        || definition
            .allowed_flows
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(flow_type))
    {
        return Ok(());
    }

    Err(Error::bad_request(
        "FLOW_NOT_ALLOWED",
        format!(
            "Flow `{flow_type}` is not allowed for session type `{}`",
            session.session_type
        ),
    ))
}

#[instrument(skip(api, session, flow))]
pub(crate) async fn create_step_chain(
    api: &BackendApi,
    session: &FlowSessionRow,
    mut flow: FlowInstanceRow,
    mut step_type: String,
    initial_input: Option<Value>,
) -> Result<FlowInstanceRow, Error> {
    debug!("Entering step chain at: {}", step_type);
    let mut pending_input = initial_input;

    loop {
        let flow_definition = get_flow_definition(api, &flow.flow_type)?;
        let flow_config = api.state.flow_registry.get_flow_definition(&flow.flow_type);
        let step_definition = get_step_definition(flow_definition, &step_type)?;
        let step_runs_async = matches!(step_definition.actor(), Actor::System)
            && is_async_step(flow_config, &step_type);
        debug!(
            "Processing step: {} (actor={:?})",
            step_type,
            step_definition.actor()
        );

        let existing_steps = api.state.flow.list_steps_for_flow(&flow.id).await?;
        let attempt_no = existing_steps
            .iter()
            .filter(|existing| existing.step_type == step_type)
            .count() as i32;

        let step_id = backend_core::id::flow_step_id()?;
        let human_suffix = if attempt_no == 0 {
            step_definition.human_id().to_owned()
        } else {
            format!("{}-{}", step_definition.human_id(), attempt_no)
        };

        let step_human_id = HumanReadableId::parse(flow.human_id.clone())
            .map_err(flow_error_to_http)?
            .with_suffix(&human_suffix)
            .map_err(flow_error_to_http)?
            .to_string();

        let created_step = api
            .state
            .flow
            .create_step(FlowStepCreateInput {
                id: step_id.clone(),
                human_id: step_human_id,
                flow_id: flow.id.clone(),
                step_type: step_type.clone(),
                actor: actor_label(step_definition.actor()).to_owned(),
                status: if step_runs_async {
                    "WAITING".to_owned()
                } else {
                    waiting_status(step_definition.actor()).to_owned()
                },
                attempt_no,
                input: pending_input.clone(),
                output: None,
                error: None,
                next_retry_at: if step_runs_async {
                    Some(Utc::now())
                } else {
                    None
                },
                finished_at: None,
            })
            .await?;

        let updated_step_ids = append_step_id(&flow.step_ids, &created_step.id);
        flow = api
            .state
            .flow
            .update_flow(
                &flow.id,
                Some(FLOW_STATUS_RUNNING.to_owned()),
                Some(Some(step_type.clone())),
                Some(updated_step_ids),
                None,
            )
            .await?;

        if step_runs_async {
            debug!("Queued async system step: {}", step_type);
            return Ok(flow);
        }

        if !matches!(step_definition.actor(), Actor::System) {
            return Ok(flow);
        }

        let input_value = pending_input.clone().unwrap_or_else(|| json!({}));
        let context = StepContext {
            session_id: flow.session_id.clone(),
            session_user_id: session.user_id.clone(),
            flow_id: flow.id.clone(),
            step_id,
            input: input_value.clone(),
            session_context: session.context.clone(),
            flow_context: flow.context.clone(),
            services: step_services(api.state.user.clone()),
        };

        match step_definition
            .execute(&context)
            .await
            .map_err(flow_error_to_http)?
        {
            StepOutcome::Done { output, updates } => {
                let actual_output = output.unwrap_or_else(|| json!({"result": "done"}));
                flow = handle_completed_step(
                    api,
                    session,
                    flow,
                    &created_step,
                    &step_type,
                    input_value.clone(),
                    actual_output,
                    updates,
                    None,
                )
                .await?;
                let next = resolve_transition(flow_definition, &step_type, None, false);
                if let Some(next_step) = next {
                    if !has_flow_step(flow_definition, &next_step) {
                        return finalize_flow(api, &flow, terminal_status(&next_step)).await;
                    }
                    step_type = next_step;
                    pending_input = None;
                } else {
                    return finalize_flow(api, &flow, FLOW_STATUS_COMPLETED).await;
                }
            }
            StepOutcome::Branched {
                branch,
                output,
                updates,
            } => {
                let actual_output = output.unwrap_or_else(|| json!({"result": "done"}));
                flow = handle_completed_step(
                    api,
                    session,
                    flow,
                    &created_step,
                    &step_type,
                    input_value.clone(),
                    actual_output,
                    updates,
                    Some(branch.clone()),
                )
                .await?;
                let next = resolve_transition(flow_definition, &step_type, Some(&branch), false);
                if let Some(next_step) = next {
                    if !has_flow_step(flow_definition, &next_step) {
                        return finalize_flow(api, &flow, terminal_status(&next_step)).await;
                    }
                    step_type = next_step;
                    pending_input = None;
                } else {
                    return finalize_flow(api, &flow, FLOW_STATUS_COMPLETED).await;
                }
            }
            StepOutcome::Waiting { .. } => {
                debug!("Step waiting: {}", step_type);
                api.state
                    .flow
                    .patch_step(&created_step.id, FlowStepPatch::new().status("WAITING"))
                    .await?;
                return Ok(flow);
            }
            StepOutcome::Failed { error, retryable } => {
                info!(
                    "Step failed: {} (error={}, retryable={})",
                    step_type, error, retryable
                );
                api.state
                    .flow
                    .patch_step(
                        &created_step.id,
                        FlowStepPatch::new()
                            .status(FLOW_STATUS_FAILED)
                            .error(json!({"error": error, "retryable": retryable}))
                            .finished_at(Utc::now()),
                    )
                    .await?;

                if let Some(next_step) = resolve_transition(flow_definition, &step_type, None, true)
                {
                    if has_flow_step(flow_definition, &next_step) {
                        step_type = next_step;
                        pending_input = None;
                        continue;
                    }

                    return finalize_flow(api, &flow, terminal_status(&next_step)).await;
                }

                return finalize_flow(api, &flow, FLOW_STATUS_FAILED).await;
            }
            StepOutcome::Retry { after } => {
                debug!("Step retry: {} (after={:?})", step_type, after);
                api.state
                    .flow
                    .patch_step(
                        &created_step.id,
                        FlowStepPatch::new().status("WAITING").next_retry_at(
                            Utc::now()
                                + Duration::from_std(after)
                                    .unwrap_or_else(|_| Duration::seconds(0)),
                        ),
                    )
                    .await?;

                return Ok(flow);
            }
        }
    }
}

fn is_async_step(
    flow_definition: Option<&backend_flow_sdk::flow::FlowDefinition>,
    step_type: &str,
) -> bool {
    flow_definition
        .and_then(|definition| definition.steps.get(step_type))
        .and_then(|step| step.config.as_ref())
        .and_then(|config| config.get("async"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub(crate) async fn finalize_flow(
    api: &BackendApi,
    flow: &FlowInstanceRow,
    status: &str,
) -> Result<FlowInstanceRow, Error> {
    let finalized = api
        .state
        .flow
        .update_flow(&flow.id, Some(status.to_owned()), Some(None), None, None)
        .await?;

    if status.eq_ignore_ascii_case(FLOW_STATUS_COMPLETED) {
        write_completed_kyc_metadata(api, &finalized).await?;
    }

    refresh_session_status(api, &finalized.session_id).await?;
    Ok(finalized)
}

async fn write_completed_kyc_metadata(
    api: &BackendApi,
    flow: &FlowInstanceRow,
) -> Result<(), Error> {
    let session = api.state.flow.get_session(&flow.session_id).await?;
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

    api.state
        .user
        .update_metadata(
            user_id,
            json!({
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
            Some(json!({ "kyc": false })),
        )
        .await
}

pub(crate) async fn refresh_session_status(
    api: &BackendApi,
    session_id: &str,
) -> Result<(), Error> {
    let flows = api.state.flow.list_flows_for_session(session_id).await?;

    if flows.is_empty() {
        api.state
            .flow
            .update_session_status(session_id, "OPEN", None)
            .await?;
        return Ok(());
    }

    if flows
        .iter()
        .any(|flow| flow.status.eq_ignore_ascii_case(FLOW_STATUS_RUNNING))
    {
        api.state
            .flow
            .update_session_status(session_id, FLOW_STATUS_RUNNING, None)
            .await?;
        return Ok(());
    }

    if flows
        .iter()
        .any(|flow| flow.status.eq_ignore_ascii_case(FLOW_STATUS_FAILED))
    {
        api.state
            .flow
            .update_session_status(session_id, FLOW_STATUS_FAILED, Some(Utc::now()))
            .await?;
        return Ok(());
    }

    if flows.iter().all(|flow| {
        flow.status.eq_ignore_ascii_case(FLOW_STATUS_COMPLETED)
            || flow.status.eq_ignore_ascii_case(FLOW_STATUS_CLOSED)
    }) {
        let status = if flows
            .iter()
            .any(|flow| flow.status.eq_ignore_ascii_case(FLOW_STATUS_CLOSED))
        {
            FLOW_STATUS_CLOSED
        } else {
            FLOW_STATUS_COMPLETED
        };
        api.state
            .flow
            .update_session_status(session_id, status, Some(Utc::now()))
            .await?;
        return Ok(());
    }

    api.state
        .flow
        .update_session_status(session_id, "OPEN", None)
        .await
}

async fn ensure_session_owner(
    api: &BackendApi,
    session_id: &str,
    user_id: &str,
) -> Result<FlowSessionRow, Error> {
    let session = api
        .state
        .flow
        .get_session(session_id)
        .await?
        .ok_or_else(|| Error::not_found("SESSION_NOT_FOUND", "Session not found"))?;

    if session.user_id.as_deref() != Some(user_id) {
        return Err(Error::unauthorized("Session does not belong to caller"));
    }

    Ok(session)
}

async fn ensure_flow_owner(
    api: &BackendApi,
    flow: &FlowInstanceRow,
    user_id: &str,
) -> Result<(), Error> {
    let session = api
        .state
        .flow
        .get_session(&flow.session_id)
        .await?
        .ok_or_else(|| Error::not_found("SESSION_NOT_FOUND", "Session not found"))?;

    if session.user_id.as_deref() != Some(user_id) {
        return Err(Error::unauthorized("Flow does not belong to caller"));
    }

    Ok(())
}

fn append_step_id(step_ids: &Value, step_id: &str) -> Value {
    let mut values = step_ids
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|value| value.as_str() != Some(step_id))
        .collect::<Vec<_>>();

    values.push(Value::String(step_id.to_owned()));
    Value::Array(values)
}

async fn handle_completed_step(
    api: &BackendApi,
    session: &FlowSessionRow,
    flow: FlowInstanceRow,
    created_step: &backend_model::db::FlowStepRow,
    step_type: &str,
    input_value: Value,
    actual_output: Value,
    updates: Option<Box<backend_flow_sdk::ContextUpdates>>,
    branch: Option<String>,
) -> Result<FlowInstanceRow, Error> {
    debug!("Step completed: {} branch={:?}", step_type, branch);

    api.state
        .flow
        .patch_step(
            &created_step.id,
            FlowStepPatch::new()
                .status(FLOW_STATUS_COMPLETED)
                .input(input_value)
                .output(actual_output.clone())
                .clear_error()
                .finished_at(Utc::now()),
        )
        .await?;

    let mut context = store_step_output(flow.context.clone(), step_type, &actual_output);
    if let Some(updates) = updates {
        if let Some(flow_patch) = updates.flow_context_patch.as_ref() {
            context = merged_json(context, flow_patch);
        }
        apply_context_updates(api, session, updates).await?;
    }

    api.state
        .flow
        .update_flow(&flow.id, None, None, None, Some(context))
        .await
}

pub(crate) async fn apply_context_updates(
    api: &BackendApi,
    session: &FlowSessionRow,
    updates: Box<backend_flow_sdk::ContextUpdates>,
) -> Result<(), Error> {
    let backend_flow_sdk::ContextUpdates {
        session_context_patch,
        user_metadata_patch,
        user_metadata_eager_patch,
        notifications,
        ..
    } = *updates;

    if let Some(session_patch) = session_context_patch.as_ref() {
        let current_session = api
            .state
            .flow
            .get_session(&session.id)
            .await?
            .ok_or_else(|| Error::internal("SESSION_NOT_FOUND", "Session not found"))?;
        let new_session_context = merged_json(current_session.context.clone(), session_patch);
        api.state
            .flow
            .update_session_context(&session.id, new_session_context)
            .await?;
    }

    if let Some(metadata_patch) = user_metadata_patch
        && let Some(user_id) = session.user_id.as_deref()
    {
        api.state
            .user
            .update_metadata(user_id, metadata_patch, user_metadata_eager_patch)
            .await?;
    }

    if let Some(notifications) = notifications {
        for notification in notifications {
            match serde_json::from_value::<backend_core::NotificationJob>(notification.clone()) {
                Ok(job) => {
                    if let Err(error) = api.state.notification_queue.enqueue(job).await {
                        tracing::warn!("Failed to enqueue notification: {}", error);
                    }
                }
                Err(error) => {
                    tracing::warn!("Failed to deserialize notification job: {}", error);
                }
            }
        }
    }

    Ok(())
}

pub(crate) fn store_step_output(mut context: Value, step_type: &str, input: &Value) -> Value {
    if !context.is_object() {
        context = json!({});
    }

    if let Some(root) = context.as_object_mut() {
        let entry = root
            .entry("step_output")
            .or_insert_with(|| Value::Object(Default::default()));

        if !entry.is_object() {
            *entry = Value::Object(Default::default());
        }

        if let Some(step_map) = entry.as_object_mut() {
            step_map.insert(step_type.to_owned(), input.clone());
        }
    }

    context
}

fn normalize_or_default_human_id(value: Option<String>, fallback: String) -> Result<String, Error> {
    let candidate = value.unwrap_or(fallback);
    HumanReadableId::parse(candidate.clone()).map_err(flow_error_to_http)?;
    Ok(candidate)
}

fn object_context(context: Option<Value>) -> Value {
    let value = context.unwrap_or_else(|| json!({}));
    if value.is_object() { value } else { json!({}) }
}

pub(crate) fn flow_error_to_http(error: FlowError) -> Error {
    match error {
        FlowError::FeatureNotEnabled { feature, .. } => Error::bad_request(
            "FEATURE_NOT_ENABLED",
            format!("Feature not enabled: {feature}"),
        ),
        FlowError::UnknownFlowType(flow_type) => Error::bad_request(
            "UNKNOWN_FLOW_TYPE",
            format!("Unknown flow type: {flow_type}"),
        ),
        FlowError::UnknownStepType(step_type) => Error::bad_request(
            "UNKNOWN_STEP_TYPE",
            format!("Unknown step type: {step_type}"),
        ),
        FlowError::UnknownSessionType(session_type) => Error::bad_request(
            "UNKNOWN_SESSION_TYPE",
            format!("Unknown session type: {session_type}"),
        ),
        FlowError::InvalidHumanReadableId(reason) => Error::bad_request("INVALID_HUMAN_ID", reason),
        FlowError::InvalidDefinition(reason) => {
            Error::bad_request("INVALID_FLOW_DEFINITION", reason)
        }
        FlowError::Serialization(reason) => Error::internal("FLOW_SERIALIZATION_ERROR", reason),
        FlowError::Io(error) => Error::internal("FLOW_IO_ERROR", error.to_string()),
    }
}

fn session_context_with_user_id(context: Option<Value>, user_id: &str) -> Value {
    let mut context = object_context(context);
    merge_json_value(&mut context, &json!({ "user_id": user_id }));
    context
}

#[instrument(skip(api))]
pub async fn get_user(
    api: &BackendApi,
    user_id: String,
    caller_id: String,
) -> Result<UserResponse, Error> {
    if user_id != caller_id {
        return Err(Error::unauthorized("Cannot access other users' data"));
    }

    let user = api
        .state
        .user
        .get_user(&user_id)
        .await?
        .ok_or_else(|| Error::not_found("USER_NOT_FOUND", "User not found"))?;
    let metadata = api.state.user.get_user_metadata(&user_id).await?;

    Ok(UserResponse::from_row_with_metadata(user, metadata))
}

#[instrument(skip(api))]
pub async fn get_completed_kyc(
    api: &BackendApi,
    user_id: String,
    caller_id: String,
) -> Result<CompletedKycResponse, Error> {
    debug!("Will get completed kyc");
    if user_id != caller_id {
        return Err(Error::unauthorized("Cannot access other users' data"));
    }

    api.state
        .user
        .get_user(&user_id)
        .await?
        .ok_or_else(|| Error::not_found("USER_NOT_FOUND", "User not found"))?;
    let metadata = api.state.user.get_user_metadata(&user_id).await?;

    let completed_kyc = metadata
        .get("kyc")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));

    Ok(CompletedKycResponse {
        user_id,
        completed_kyc,
    })
}
