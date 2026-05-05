use chrono::{DateTime, Utc};
use serde_json::Value;

pub type RepoResult<T> = backend_core::Result<T>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageFilter {
    pub page: i32,
    pub limit: i32,
}

impl PageFilter {
    pub fn normalized(self) -> Self {
        Self {
            page: self.page.max(1),
            limit: self.limit.clamp(1, 100),
        }
    }

    pub fn offset(&self) -> i64 {
        i64::from((self.page - 1) * self.limit)
    }
}

#[derive(Debug, Clone)]
pub struct FlowSessionCreateInput {
    pub id: String,
    pub human_id: String,
    pub user_id: Option<String>,
    pub session_type: String,
    pub status: String,
    pub context: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowSessionFilter {
    pub user_id: Option<String>,
    pub user_ids: Option<Vec<String>>,
    pub session_type: Option<String>,
    pub status: Option<String>,
    pub page: i32,
    pub limit: i32,
}

impl FlowSessionFilter {
    pub fn normalized(self) -> Self {
        let page = self.page.max(1);
        let limit = self.limit.clamp(1, 100);
        let user_id = self
            .user_id
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty());
        let user_ids = self
            .user_ids
            .map(|values| {
                let mut normalized: Vec<String> = values
                    .into_iter()
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
                    .collect();
                normalized.sort();
                normalized.dedup();
                normalized
            })
            .filter(|values| !values.is_empty());
        let session_type = self
            .session_type
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty());
        let status = self
            .status
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty());

        Self {
            user_id,
            user_ids,
            session_type,
            status,
            page,
            limit,
        }
    }

    pub fn offset(&self) -> i64 {
        i64::from((self.page - 1) * self.limit)
    }
}

#[derive(Debug, Clone)]
pub struct FlowInstanceCreateInput {
    pub id: String,
    pub human_id: String,
    pub session_id: String,
    pub flow_type: String,
    pub status: String,
    pub current_step: Option<String>,
    pub step_ids: Value,
    pub context: Value,
}

#[derive(Debug, Clone)]
pub struct FlowStepCreateInput {
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
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
pub struct FlowStepPatch {
    pub status: Option<String>,
    pub attempt_no: Option<i32>,
    pub input: Option<Option<Value>>,
    pub output: Option<Option<Value>>,
    pub error: Option<Option<Value>>,
    pub next_retry_at: Option<Option<DateTime<Utc>>>,
    pub finished_at: Option<Option<DateTime<Utc>>>,
}

impl FlowStepPatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    pub fn attempt_no(mut self, attempt_no: i32) -> Self {
        self.attempt_no = Some(attempt_no);
        self
    }

    pub fn input(mut self, input: Value) -> Self {
        self.input = Some(Some(input));
        self
    }

    pub fn output(mut self, output: Value) -> Self {
        self.output = Some(Some(output));
        self
    }

    pub fn error(mut self, error: Value) -> Self {
        self.error = Some(Some(error));
        self
    }

    pub fn clear_error(mut self) -> Self {
        self.error = Some(None);
        self
    }

    pub fn next_retry_at(mut self, next_retry_at: DateTime<Utc>) -> Self {
        self.next_retry_at = Some(Some(next_retry_at));
        self
    }

    pub fn finished_at(mut self, finished_at: DateTime<Utc>) -> Self {
        self.finished_at = Some(Some(finished_at));
        self
    }
}

#[derive(Debug, Clone)]
pub struct UserDataUpsertInput {
    pub user_id: String,
    pub name: String,
    pub data_type: String,
    pub content: Value,
    pub eager_fetch: bool,
}

#[derive(Debug, Clone)]
pub struct DepositRecipientUpsertInput {
    pub provider: String,
    pub full_name: String,
    pub phone_number: String,
    pub phone_regex: String,
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepositRecipientContact {
    pub provider: String,
    pub full_name: String,
    pub phone_number: String,
    pub currency: String,
}

#[backend_core::async_trait]
pub trait FlowRepo: Send + Sync {
    async fn create_session(
        &self,
        input: FlowSessionCreateInput,
    ) -> RepoResult<backend_model::db::FlowSessionRow>;

    async fn get_session(
        &self,
        session_id: &str,
    ) -> RepoResult<Option<backend_model::db::FlowSessionRow>>;

    async fn list_sessions(
        &self,
        filter: FlowSessionFilter,
    ) -> RepoResult<(Vec<backend_model::db::FlowSessionRow>, i64)>;

    async fn update_session_status(
        &self,
        session_id: &str,
        status: &str,
        completed_at: Option<DateTime<Utc>>,
    ) -> RepoResult<()>;

    async fn update_session_context(&self, session_id: &str, context: Value) -> RepoResult<()>;

    async fn create_flow(
        &self,
        input: FlowInstanceCreateInput,
    ) -> RepoResult<backend_model::db::FlowInstanceRow>;

    async fn get_flow(
        &self,
        flow_id: &str,
    ) -> RepoResult<Option<backend_model::db::FlowInstanceRow>>;

    async fn list_flows_for_session(
        &self,
        session_id: &str,
    ) -> RepoResult<Vec<backend_model::db::FlowInstanceRow>>;

    async fn update_flow(
        &self,
        flow_id: &str,
        status: Option<String>,
        current_step: Option<Option<String>>,
        step_ids: Option<Value>,
        context: Option<Value>,
    ) -> RepoResult<backend_model::db::FlowInstanceRow>;

    async fn create_step(
        &self,
        input: FlowStepCreateInput,
    ) -> RepoResult<backend_model::db::FlowStepRow>;

    async fn get_step(&self, step_id: &str) -> RepoResult<Option<backend_model::db::FlowStepRow>>;

    async fn list_steps_for_flow(
        &self,
        flow_id: &str,
    ) -> RepoResult<Vec<backend_model::db::FlowStepRow>>;

    async fn patch_step(
        &self,
        step_id: &str,
        patch: FlowStepPatch,
    ) -> RepoResult<backend_model::db::FlowStepRow>;

    async fn claim_next_system_step(&self) -> RepoResult<Option<backend_model::db::FlowStepRow>>;
}

#[backend_core::async_trait]
pub trait UserRepo: Send + Sync {
    async fn create_user(
        &self,
        req: &backend_model::user::UserUpsert,
    ) -> RepoResult<backend_model::db::UserRow>;

    async fn get_user(&self, user_id: &str) -> RepoResult<Option<backend_model::db::UserRow>>;

    async fn update_user(
        &self,
        user_id: &str,
        req: &backend_model::user::UserUpsert,
    ) -> RepoResult<Option<backend_model::db::UserRow>>;

    async fn delete_user(&self, user_id: &str) -> RepoResult<u64>;

    async fn search_users(
        &self,
        req: &backend_model::user::UserSearch,
    ) -> RepoResult<Vec<backend_model::db::UserRow>>;

    async fn resolve_user_by_phone(
        &self,
        phone: &str,
    ) -> RepoResult<Option<backend_model::db::UserRow>>;

    async fn find_users_by_phone(
        &self,
        phone: &str,
    ) -> RepoResult<Vec<backend_model::db::UserRow>>;

    async fn resolve_or_create_user_by_phone(
        &self,
        phone: &str,
    ) -> RepoResult<(backend_model::db::UserRow, bool)>;

    async fn upsert_user_data(
        &self,
        input: UserDataUpsertInput,
    ) -> RepoResult<backend_model::db::UserDataRow>;

    async fn list_user_data(
        &self,
        user_id: &str,
        eager_fetch_only: bool,
    ) -> RepoResult<Vec<backend_model::db::UserDataRow>>;

    async fn update_phone_number(&self, user_id: &str, phone_number: &str) -> RepoResult<()>;
    async fn update_full_name(&self, user_id: &str, full_name: &str) -> RepoResult<()>;
    async fn get_user_metadata(&self, user_id: &str) -> RepoResult<Value>;

    async fn update_metadata(
        &self,
        user_id: &str,
        metadata_patch: Value,
        eager_patch: Option<Value>,
    ) -> RepoResult<()>;
}
