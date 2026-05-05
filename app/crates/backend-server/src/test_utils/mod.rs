use crate::flows::registry as flow_registry;
use crate::object_storage::{EncryptionMode, ObjectStorage, PresignedUpload};
use crate::state::AppState;
use crate::worker::NotificationQueue;
use backend_auth::SignatureState;
use backend_core::NotificationJob;
use backend_core::async_trait;
use backend_core::{Config, Error};
use backend_repository::{
    FlowInstanceCreateInput, FlowRepo, FlowSessionCreateInput, FlowSessionFilter,
    FlowStepCreateInput, FlowStepPatch, RepoResult, UserDataUpsertInput,
    UserRepo,
};
use bytes::Bytes;
use mockall::mock;
use std::sync::Arc;
use std::time::Duration;

mock! {
    pub NotificationQueue {}
    #[async_trait]
    impl NotificationQueue for NotificationQueue {
        async fn enqueue(&self, job: NotificationJob) -> backend_core::Result<()>;
    }
}

mock! {
    pub ObjectStorage {}
    #[async_trait]
    impl ObjectStorage for ObjectStorage {
        async fn head_object(&self, bucket: &str, key: &str) -> std::result::Result<(), Error>;

        async fn upload(
            &self,
            bucket: &str,
            key: &str,
            mime_type: &str,
            encryption: EncryptionMode,
            body: Bytes,
        ) -> std::result::Result<(), Error>;

        async fn upload_presigned(
            &self,
            bucket: &str,
            key: &str,
            mime_type: &str,
            encryption: EncryptionMode,
            expires_in: Duration,
        ) -> std::result::Result<PresignedUpload, Error>;

        async fn download(&self, bucket: &str, key: &str) -> std::result::Result<Bytes, Error>;

        async fn download_presigned(
            &self,
            bucket: &str,
            key: &str,
            expires_in: Duration,
            content_disposition: Option<String>,
        ) -> std::result::Result<String, Error>;
    }
}

mock! {
    pub FlowRepo {}
    #[async_trait]
    impl FlowRepo for FlowRepo {
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
            completed_at: Option<chrono::DateTime<chrono::Utc>>,
        ) -> RepoResult<()>;
        async fn update_session_context(
            &self,
            session_id: &str,
            context: serde_json::Value,
        ) -> RepoResult<()>;
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
            step_ids: Option<serde_json::Value>,
            context: Option<serde_json::Value>,
        ) -> RepoResult<backend_model::db::FlowInstanceRow>;
        async fn create_step(
            &self,
            input: FlowStepCreateInput,
        ) -> RepoResult<backend_model::db::FlowStepRow>;
        async fn get_step(
            &self,
            step_id: &str,
        ) -> RepoResult<Option<backend_model::db::FlowStepRow>>;
        async fn list_steps_for_flow(
            &self,
            flow_id: &str,
        ) -> RepoResult<Vec<backend_model::db::FlowStepRow>>;
        async fn patch_step(
            &self,
            step_id: &str,
            patch: FlowStepPatch,
        ) -> RepoResult<backend_model::db::FlowStepRow>;
        async fn claim_next_system_step(
            &self,
        ) -> RepoResult<Option<backend_model::db::FlowStepRow>>;
    }
}

mock! {
    pub UserRepo {}
    #[async_trait]
    impl UserRepo for UserRepo {
        async fn create_user(
            &self,
            req: &backend_model::user::UserUpsert,
        ) -> RepoResult<backend_model::db::UserRow>;
        async fn get_user(
            &self,
            user_id: &str,
        ) -> RepoResult<Option<backend_model::db::UserRow>>;
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
        async fn get_user_metadata(&self, user_id: &str) -> RepoResult<serde_json::Value>;

        async fn update_metadata(&self, user_id: &str, metadata_patch: serde_json::Value, eager_patch: Option<serde_json::Value>) -> RepoResult<()>;
    }
}

#[derive(Default)]
pub struct TestAppStateBuilder {
    pub flow: Option<Arc<dyn FlowRepo>>,
    pub user: Option<Arc<dyn UserRepo>>,
    pub notification_queue: Option<Arc<dyn NotificationQueue>>,
    pub object_storage: Option<Arc<dyn ObjectStorage>>,
    pub config: Option<Config>,
}

impl TestAppStateBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_flow(mut self, flow: Arc<dyn FlowRepo>) -> Self {
        self.flow = Some(flow);
        self
    }

    pub fn with_user(mut self, user: Arc<dyn UserRepo>) -> Self {
        self.user = Some(user);
        self
    }

    pub fn with_notification_queue(mut self, queue: Arc<dyn NotificationQueue>) -> Self {
        self.notification_queue = Some(queue);
        self
    }

    pub fn with_object_storage(mut self, object_storage: Arc<dyn ObjectStorage>) -> Self {
        self.object_storage = Some(object_storage);
        self
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    pub fn build(self) -> AppState {
        let config = self.config.unwrap_or_else(|| {
            serde_yaml::from_str(
                r#"
server:
  address: "127.0.0.1"
  port: 8080
  tls:
    cert_path: "cert.pem"
    key_path: "key.pem"
logging:
  level: "info"
database:
  url: "postgres://localhost/test"
oauth2:
  issuer: "http://localhost:8081/realms/test"
kc:
  enabled: true
  base_path: "/kc"
  signature_secret: "test-secret"
  max_clock_skew_seconds: 30
  max_body_bytes: 1048576
bff:
  enabled: true
  base_path: "/bff"
staff:
  enabled: true
  base_path: "/staff"
cuss:
  api_url: "http://localhost:8082"
"#,
            )
            .unwrap()
        });

        let signature_state = Arc::new(SignatureState {
            signature_secret: config.kc.signature_secret.clone(),
            max_clock_skew_seconds: config.kc.max_clock_skew_seconds,
            max_body_bytes: config.kc.max_body_bytes,
        });

        AppState {
            flow: self.flow.unwrap_or_else(|| Arc::new(MockFlowRepo::new())),
            flow_registry: Arc::new(
                flow_registry::build_registry(flow_registry::RegistryImports::default()).unwrap(),
            ),
            user: self.user.unwrap_or_else(|| Arc::new(MockUserRepo::new())),
            notification_queue: self
                .notification_queue
                .unwrap_or_else(|| Arc::new(MockNotificationQueue::new())),
            object_storage: self
                .object_storage
                .unwrap_or_else(|| Arc::new(MockObjectStorage::new())),
            config,
            signature_state,
            replay_guard: Arc::new(crate::auth_signature::InMemoryReplayGuard::new()),
        }
    }
}
