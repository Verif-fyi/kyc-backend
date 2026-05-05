mod handlers;
mod models;
pub(crate) mod service;

use axum::{Router, routing::get};

use super::BackendApi;

pub use models::*;

/// BFF Flow API OpenAPI specification with Bearer auth
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        handlers::get_user,
        handlers::get_completed_kyc,
        handlers::list_sessions,
        handlers::create_session,
        handlers::get_session,
        handlers::list_session_flows,
        handlers::add_flow_to_session,
        handlers::get_flow,
        handlers::list_flow_steps,
        handlers::get_step,
        handlers::submit_step,
    ),
    components(schemas(
        UserResponse,
        CompletedKycResponse,
        CreateSessionRequest,
        AddFlowRequest,
        SubmitStepRequest,
        SessionResponse,
        SessionDetailResponse,
        FlowResponse,
        FlowDetailResponse,
        StepResponse,
    )),
    // Security requirement for all endpoints
    security(
        ("KcSignature" = [])
    ),
    tags(
        (name = "users", description = "User profile and completed KYC endpoints"),
        (name = "sessions", description = "Session management endpoints"),
        (name = "flows", description = "Flow execution endpoints"),
        (name = "steps", description = "Step submission endpoints"),
    )
)]
pub struct BffFlowOpenApi;

pub fn router(api: BackendApi) -> Router {
    Router::new()
        .route("/users/{user_id}", get(handlers::get_user))
        .route(
            "/users/{user_id}/completed-kyc",
            get(handlers::get_completed_kyc),
        )
        .route(
            "/sessions",
            get(handlers::list_sessions).post(handlers::create_session),
        )
        .route("/sessions/{session_id}", get(handlers::get_session))
        .route(
            "/sessions/{session_id}/flows",
            get(handlers::list_session_flows).post(handlers::add_flow_to_session),
        )
        .route("/flows/{flow_id}", get(handlers::get_flow))
        .route("/flows/{flow_id}/steps", get(handlers::list_flow_steps))
        .route(
            "/steps/{step_id}",
            get(handlers::get_step).post(handlers::submit_step),
        )
        .with_state(api)
}
