use super::BackendApi;
use super::bff_flow::{
    FlowDetailResponse, FlowResponse, SessionResponse, StepResponse, SubmitStepRequest,
    service as bff_service,
};
use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing::get};
use backend_core::Error;
use backend_flow_sdk::{StepContext, StepOutcome};
use backend_repository::{FlowSessionFilter, FlowStepPatch};
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use tracing::instrument;
use utoipa::{OpenApi, ToSchema};

#[derive(OpenApi)]
#[openapi(
    paths(
        list_staff_sessions,
        get_staff_session,
        get_staff_flow,
        list_admin_steps,
        get_admin_step,
        submit_admin_step,
    ),
    components(schemas(
        StaffSessionQuery,
        StaffSessionResponse,
        StaffSessionDetailResponse,
        StaffSessionListResponse,
        AdminStepQuery,
        SubmitStepRequest,
        SessionResponse,
        FlowResponse,
        FlowDetailResponse,
        StepResponse
    )),
    tags((name = "staff-flow", description = "Staff flow v2 endpoints"))
)]
pub struct StaffFlowOpenApi;

pub fn router(api: BackendApi) -> Router {
    Router::new()
        .route("/sessions", get(list_staff_sessions))
        .route("/sessions/{session_id}", get(get_staff_session))
        .route("/flows/{flow_id}", get(get_staff_flow))
        .route("/steps", get(list_admin_steps))
        .route(
            "/steps/{step_id}",
            get(get_admin_step).post(submit_admin_step),
        )
        .with_state(api)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StaffSessionQuery {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub phone_number: Option<String>,
    #[serde(default)]
    pub session_type: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default = "default_page")]
    pub page: i32,
    #[serde(default = "default_limit")]
    pub limit: i32,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminStepQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub phone_number: Option<String>,
    #[serde(default)]
    pub flow_type: Option<String>,
}

#[derive(Debug, serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StaffSessionResponse {
    pub id: String,
    pub human_id: String,
    pub session_type: String,
    pub status: String,
    pub user_id: Option<String>,
    pub phone_number: Option<String>,
    pub full_name: Option<String>,
    pub context: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StaffSessionDetailResponse {
    pub session: StaffSessionResponse,
    pub flows: Vec<FlowResponse>,
}

#[derive(Debug, serde::Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StaffSessionListResponse {
    pub items: Vec<StaffSessionResponse>,
    pub page: i32,
    pub limit: i32,
    pub total: i64,
}

fn default_page() -> i32 {
    1
}

fn default_limit() -> i32 {
    50
}

#[utoipa::path(
    get,
    path = "/flow/sessions",
    params(
        ("userId" = Option<String>, Query),
        ("phoneNumber" = Option<String>, Query),
        ("sessionType" = Option<String>, Query),
        ("status" = Option<String>, Query),
        ("page" = Option<i32>, Query),
        ("limit" = Option<i32>, Query)
    ),
    responses((status = 200, body = StaffSessionListResponse)),
    tag = "staff-flow",
    security(("KcSignature" = []))
)]
#[instrument(skip(api))]
async fn list_staff_sessions(
    State(api): State<BackendApi>,
    Query(query): Query<StaffSessionQuery>,
) -> Result<Json<StaffSessionListResponse>, Error> {
    let user_ids = resolve_user_ids_for_filters(
        &api,
        query.user_id.as_deref(),
        query.phone_number.as_deref(),
    )
    .await?;
    if query.phone_number.is_some() && user_ids.is_empty() {
        return Ok(Json(StaffSessionListResponse {
            items: Vec::new(),
            page: query.page.max(1),
            limit: query.limit.clamp(1, 100),
            total: 0,
        }));
    }

    let filter = FlowSessionFilter {
        user_id: query.user_id.clone(),
        user_ids: if query.phone_number.is_some() {
            Some(user_ids)
        } else {
            None
        },
        session_type: query.session_type.clone(),
        status: query.status.clone(),
        page: query.page,
        limit: query.limit,
    }
    .normalized();

    let (rows, total) = api.state.flow.list_sessions(filter.clone()).await?;
    let users = load_users_for_sessions(&api, &rows).await?;
    let items = rows
        .into_iter()
        .map(|row| build_staff_session_response(row, &users))
        .collect();

    Ok(Json(StaffSessionListResponse {
        items,
        page: filter.page,
        limit: filter.limit,
        total,
    }))
}

#[utoipa::path(
    get,
    path = "/flow/sessions/{session_id}",
    params(("session_id" = String, Path)),
    responses((status = 200, body = StaffSessionDetailResponse)),
    tag = "staff-flow",
    security(("KcSignature" = []))
)]
#[instrument(skip(api))]
async fn get_staff_session(
    State(api): State<BackendApi>,
    Path(session_id): Path<String>,
) -> Result<Json<StaffSessionDetailResponse>, Error> {
    let session = api
        .state
        .flow
        .get_session(&session_id)
        .await?
        .ok_or_else(|| Error::not_found("SESSION_NOT_FOUND", "Session not found"))?;
    let flows = api.state.flow.list_flows_for_session(&session.id).await?;
    let users = load_users_for_sessions(&api, std::slice::from_ref(&session)).await?;

    Ok(Json(StaffSessionDetailResponse {
        session: build_staff_session_response(session, &users),
        flows: flows.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/flow/flows/{flow_id}",
    params(("flow_id" = String, Path)),
    responses((status = 200, body = FlowDetailResponse)),
    tag = "staff-flow",
    security(("KcSignature" = []))
)]
#[instrument(skip(api))]
async fn get_staff_flow(
    State(api): State<BackendApi>,
    Path(flow_id): Path<String>,
) -> Result<Json<FlowDetailResponse>, Error> {
    let flow = api
        .state
        .flow
        .get_flow(&flow_id)
        .await?
        .ok_or_else(|| Error::not_found("FLOW_NOT_FOUND", "Flow not found"))?;
    let steps = api.state.flow.list_steps_for_flow(&flow_id).await?;

    Ok(Json(FlowDetailResponse {
        flow: flow.into(),
        steps: steps.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/flow/steps",
    params(
        ("status" = Option<String>, Query),
        ("userId" = Option<String>, Query),
        ("phoneNumber" = Option<String>, Query),
        ("flowType" = Option<String>, Query)
    ),
    responses((status = 200, body = [StepResponse])),
    tag = "staff-flow",
    security(("KcSignature" = []))
)]
#[instrument(skip(api))]
async fn list_admin_steps(
    State(api): State<BackendApi>,
    Query(query): Query<AdminStepQuery>,
) -> Result<Json<Vec<StepResponse>>, Error> {
    let user_ids = resolve_user_ids_for_filters(
        &api,
        query.user_id.as_deref(),
        query.phone_number.as_deref(),
    )
    .await?;
    if query.phone_number.is_some() && user_ids.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let (sessions, _) = api
        .state
        .flow
        .list_sessions(FlowSessionFilter {
            user_id: query.user_id.clone(),
            user_ids: if query.phone_number.is_some() {
                Some(user_ids)
            } else {
                None
            },
            session_type: None,
            status: None,
            page: 1,
            limit: 500,
        })
        .await?;

    let mut steps: Vec<StepResponse> = Vec::new();
    for session in sessions {
        let flows = api.state.flow.list_flows_for_session(&session.id).await?;
        for flow in flows {
            if let Some(flow_type) = query.flow_type.as_deref()
                && !flow.flow_type.eq_ignore_ascii_case(flow_type)
            {
                continue;
            }

            let flow_steps = api.state.flow.list_steps_for_flow(&flow.id).await?;
            for step in flow_steps {
                if !step.actor.eq_ignore_ascii_case("ADMIN") {
                    continue;
                }
                if let Some(status) = query.status.as_deref()
                    && !step.status.eq_ignore_ascii_case(status)
                {
                    continue;
                }
                steps.push(step.into());
            }
        }
    }

    steps.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });

    Ok(Json(steps))
}

#[utoipa::path(
    get,
    path = "/flow/steps/{step_id}",
    params(("step_id" = String, Path)),
    responses((status = 200, body = StepResponse)),
    tag = "staff-flow",
    security(("KcSignature" = []))
)]
#[instrument(skip(api))]
async fn get_admin_step(
    State(api): State<BackendApi>,
    Path(step_id): Path<String>,
) -> Result<Json<StepResponse>, Error> {
    let step = get_admin_step_row(&api, &step_id).await?;
    Ok(Json(step.into()))
}

#[utoipa::path(
    post,
    path = "/flow/steps/{step_id}",
    params(("step_id" = String, Path)),
    request_body = SubmitStepRequest,
    responses((status = 200, body = StepResponse)),
    tag = "staff-flow",
    security(("KcSignature" = []))
)]
#[instrument(skip(api, body))]
async fn submit_admin_step(
    State(api): State<BackendApi>,
    Path(step_id): Path<String>,
    Json(body): Json<SubmitStepRequest>,
) -> Result<Json<StepResponse>, Error> {
    let step = get_admin_step_row(&api, &step_id).await?;
    if !step.status.eq_ignore_ascii_case("WAITING") {
        return Err(Error::conflict(
            "STEP_NOT_WAITING",
            "Admin step is not waiting for input",
        ));
    }
    let flow = api
        .state
        .flow
        .get_flow(&step.flow_id)
        .await?
        .ok_or_else(|| Error::not_found("FLOW_NOT_FOUND", "Flow not found"))?;
    let session = api
        .state
        .flow
        .get_session(&flow.session_id)
        .await?
        .ok_or_else(|| Error::not_found("SESSION_NOT_FOUND", "Session not found"))?;

    let flow_definition = bff_service::get_flow_definition(&api, &flow.flow_type)?;
    let step_definition = bff_service::get_step_definition(flow_definition, &step.step_type)?;

    step_definition
        .validate_input(&body.input)
        .await
        .map_err(bff_service::flow_error_to_http)?;

    let verify_context = StepContext {
        session_id: session.id.clone(),
        session_user_id: session.user_id.clone(),
        flow_id: flow.id.clone(),
        step_id: step.id.clone(),
        input: body.input.clone(),
        session_context: session.context.clone(),
        flow_context: flow.context.clone(),
        services: crate::flows::runtime::step_services(api.state.user.clone()),
    };

    let verify_outcome = step_definition
        .verify_input(&verify_context, &body.input)
        .await
        .map_err(bff_service::flow_error_to_http)?;

    let (output_value, context_updates, branch, status) = match verify_outcome {
        StepOutcome::Done { output, updates } => (
            output.unwrap_or_else(|| json!({"verified": true})),
            updates,
            None,
            "COMPLETED",
        ),
        StepOutcome::Branched {
            branch,
            output,
            updates,
        } => (
            output.unwrap_or_else(|| json!({"verified": true})),
            updates,
            Some(branch),
            "COMPLETED",
        ),
        StepOutcome::Failed { error, retryable } => {
            let session_id = flow.session_id.clone();
            let updated = api
                .state
                .flow
                .patch_step(
                    &step_id,
                    FlowStepPatch::new()
                        .status("FAILED")
                        .input(body.input.clone())
                        .error(json!({"error": error, "retryable": retryable}))
                        .finished_at(Utc::now()),
                )
                .await?;

            if let Some(next_step) = crate::flows::runtime::resolve_transition(
                flow_definition,
                &step.step_type,
                None,
                true,
            ) {
                if bff_service::has_flow_step(flow_definition, &next_step) {
                    bff_service::create_step_chain(&api, &session, flow, next_step, None).await?;
                } else {
                    bff_service::finalize_flow(
                        &api,
                        &flow,
                        bff_service::terminal_status(&next_step),
                    )
                    .await?;
                }
                bff_service::refresh_session_status(&api, &session_id).await?;
            } else {
                bff_service::finalize_flow(&api, &flow, "FAILED").await?;
            }

            return Ok(Json(updated.into()));
        }
        StepOutcome::Waiting { .. } | StepOutcome::Retry { .. } => {
            return Err(Error::conflict(
                "INVALID_ADMIN_STEP_OUTCOME",
                "Admin submission must resolve to a terminal verification outcome",
            ));
        }
    };

    let mut updated_flow_context =
        bff_service::store_step_output(flow.context.clone(), &step.step_type, &body.input);
    if let Some(updates) = context_updates {
        if let Some(flow_patch) = updates.flow_context_patch.as_ref() {
            updated_flow_context =
                crate::flows::runtime::merged_json(updated_flow_context, flow_patch);
        }
        bff_service::apply_context_updates(&api, &session, updates).await?;
    }

    let updated_step = api
        .state
        .flow
        .patch_step(
            &step_id,
            FlowStepPatch::new()
                .status(status)
                .input(body.input.clone())
                .output(output_value)
                .clear_error()
                .finished_at(Utc::now()),
        )
        .await?;

    let mut current_flow = api
        .state
        .flow
        .update_flow(&flow.id, None, None, None, Some(updated_flow_context))
        .await?;

    if let Some(next_step) = crate::flows::runtime::resolve_transition(
        flow_definition,
        &updated_step.step_type,
        branch.as_deref(),
        false,
    ) {
        if bff_service::has_flow_step(flow_definition, &next_step) {
            current_flow =
                bff_service::create_step_chain(&api, &session, current_flow, next_step, None)
                    .await?;
        } else {
            current_flow = bff_service::finalize_flow(
                &api,
                &current_flow,
                bff_service::terminal_status(&next_step),
            )
            .await?;
        }
    } else {
        current_flow = bff_service::finalize_flow(&api, &current_flow, "COMPLETED").await?;
    }

    bff_service::refresh_session_status(&api, &current_flow.session_id).await?;
    Ok(Json(updated_step.into()))
}

async fn get_admin_step_row(
    api: &BackendApi,
    step_id: &str,
) -> Result<backend_model::db::FlowStepRow, Error> {
    let step = api
        .state
        .flow
        .get_step(step_id)
        .await?
        .ok_or_else(|| Error::not_found("STEP_NOT_FOUND", "Step not found"))?;

    if !step.actor.eq_ignore_ascii_case("ADMIN") {
        return Err(Error::bad_request(
            "STEP_NOT_ADMIN",
            "Step is not an admin-managed step",
        ));
    }

    Ok(step)
}

async fn resolve_user_ids_for_filters(
    api: &BackendApi,
    user_id: Option<&str>,
    phone_number: Option<&str>,
) -> Result<Vec<String>, Error> {
    let requested_user_id = user_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let requested_phone = phone_number
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);

    let Some(phone_number) = requested_phone else {
        return Ok(requested_user_id.into_iter().collect());
    };

    let mut user_ids: Vec<String> = api
        .state
        .user
        .find_users_by_phone(&phone_number)
        .await?
        .into_iter()
        .map(|user| user.user_id)
        .collect();
    user_ids.sort();
    user_ids.dedup();

    if let Some(user_id) = requested_user_id {
        user_ids.retain(|candidate| candidate == &user_id);
    }

    Ok(user_ids)
}

async fn load_users_for_sessions(
    api: &BackendApi,
    sessions: &[backend_model::db::FlowSessionRow],
) -> Result<HashMap<String, backend_model::db::UserRow>, Error> {
    let mut users = HashMap::new();

    for user_id in sessions
        .iter()
        .filter_map(|session| session.user_id.clone())
    {
        if users.contains_key(&user_id) {
            continue;
        }
        if let Some(user) = api.state.user.get_user(&user_id).await? {
            users.insert(user_id, user);
        }
    }

    Ok(users)
}

fn build_staff_session_response(
    row: backend_model::db::FlowSessionRow,
    users: &HashMap<String, backend_model::db::UserRow>,
) -> StaffSessionResponse {
    let user_id = row.user_id.clone();
    let user = user_id.as_ref().and_then(|candidate| users.get(candidate));
    let phone_number = user.and_then(|value| value.phone_number.clone());
    let full_name = user.and_then(|value| value.full_name.clone());

    StaffSessionResponse {
        id: row.id,
        human_id: row.human_id,
        session_type: row.session_type,
        status: row.status,
        user_id,
        phone_number,
        full_name,
        context: row.context,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}
