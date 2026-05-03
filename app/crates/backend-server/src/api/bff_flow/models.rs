use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    pub session_type: String,
    #[serde(default)]
    pub human_id: Option<String>,
    #[serde(default)]
    pub context: Option<Value>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddFlowRequest {
    pub flow_type: String,
    #[serde(default)]
    pub human_id: Option<String>,
    #[serde(default)]
    pub context: Option<Value>,
    #[serde(default)]
    pub initial_step: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubmitStepRequest {
    #[serde(default)]
    pub input: Value,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserResponse {
    pub user_id: String,
    pub username: String,
    pub full_name: Option<String>,
    pub email: Option<String>,
    pub email_verified: bool,
    pub phone_number: Option<String>,
    pub disabled: bool,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompletedKycResponse {
    pub user_id: String,
    pub completed_kyc: Value,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionResponse {
    pub id: String,
    pub human_id: String,
    pub session_type: String,
    pub status: String,
    pub user_id: Option<String>,
    pub context: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetailResponse {
    pub session: SessionResponse,
    pub flows: Vec<FlowResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FlowResponse {
    pub id: String,
    pub human_id: String,
    pub session_id: String,
    pub flow_type: String,
    pub status: String,
    pub current_step: Option<String>,
    pub step_ids: Value,
    pub context: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FlowDetailResponse {
    pub flow: FlowResponse,
    pub steps: Vec<StepResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepResponse {
    pub id: String,
    pub human_id: String,
    pub flow_id: String,
    pub step_type: String,
    pub actor: String,
    pub status: String,
    pub attempt_no: i32,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub error: Option<Value>,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl From<backend_model::db::FlowSessionRow> for SessionResponse {
    fn from(row: backend_model::db::FlowSessionRow) -> Self {
        Self {
            id: row.id,
            human_id: row.human_id,
            session_type: row.session_type,
            status: row.status,
            user_id: row.user_id,
            context: row.context,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

impl From<backend_model::db::FlowInstanceRow> for FlowResponse {
    fn from(row: backend_model::db::FlowInstanceRow) -> Self {
        Self {
            id: row.id,
            human_id: row.human_id,
            session_id: row.session_id,
            flow_type: row.flow_type,
            status: row.status,
            current_step: row.current_step,
            step_ids: row.step_ids,
            context: row.context,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

impl From<backend_model::db::FlowStepRow> for StepResponse {
    fn from(row: backend_model::db::FlowStepRow) -> Self {
        Self {
            id: row.id,
            human_id: row.human_id,
            flow_id: row.flow_id,
            step_type: row.step_type,
            actor: row.actor,
            status: row.status,
            attempt_no: row.attempt_no,
            input: row.input,
            output: row.output,
            error: row.error,
            next_retry_at: row.next_retry_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
            finished_at: row.finished_at,
        }
    }
}

impl UserResponse {
    pub fn from_row_with_metadata(row: backend_model::db::UserRow, metadata: Value) -> Self {
        Self {
            user_id: row.user_id,
            username: row.username,
            full_name: row.full_name,
            email: row.email,
            email_verified: row.email_verified,
            phone_number: row.phone_number,
            disabled: row.disabled,
            metadata,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}
