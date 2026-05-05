use crate::flows::registry as flow_registry;
use crate::object_storage::{ObjectStorage, S3ObjectStorage};
use crate::worker::{NotificationQueue, RedisNotificationQueue};
use backend_auth::SignatureState;
use backend_core::Config;
use backend_repository::{
    FlowRepo, FlowRepository, UserRepo, UserRepository,
};
use diesel_async::AsyncPgConnection;
use diesel_async::pooled_connection::deadpool::Pool;
use std::sync::Arc;
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub flow: Arc<dyn FlowRepo>,
    pub flow_registry: Arc<backend_flow_sdk::FlowRegistry>,
    pub user: Arc<dyn UserRepo>,
    pub notification_queue: Arc<dyn NotificationQueue>,
    pub object_storage: Arc<dyn ObjectStorage>,
    pub config: Config,
    pub signature_state: Arc<SignatureState>,
    pub replay_guard: Arc<dyn crate::auth_signature::ReplayGuard>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("flow", &"<FlowRepository>")
            .field("flow_registry", &"<FlowRegistry>")
            .field("user", &"<UserRepository>")
            .field("object_storage", &"<ObjectStorage>")
            .field("config", &self.config)
            .field("signature_state", &"<SignatureState>")
            .finish()
    }
}

impl AppState {
    pub async fn from_config(
        cfg: &Config,
        pool: Pool<AsyncPgConnection>,
        imports: flow_registry::RegistryImports,
    ) -> backend_core::Result<Self> {
        info!("initializing application state and repositories");

        let object_storage: Arc<dyn ObjectStorage> = match cfg.storage.as_ref().map(|s| &s.r#type) {
            Some(backend_core::StorageType::Minio) => {
                let minio_cfg = cfg
                    .storage
                    .as_ref()
                    .and_then(|s| s.minio.as_ref())
                    .ok_or_else(|| {
                        backend_core::Error::Server(
                            "storage.type=minio requires storage.minio config".to_owned(),
                        )
                    })?;

                let mut builder = aws_sdk_s3::config::Builder::new()
                    .behavior_version_latest()
                    .region(aws_types::region::Region::new(minio_cfg.region.clone()))
                    .endpoint_url(minio_cfg.endpoint.clone())
                    .credentials_provider(aws_sdk_s3::config::Credentials::new(
                        minio_cfg.access_key.clone(),
                        minio_cfg.secret_key.clone(),
                        None,
                        None,
                        "minio-static",
                    ));
                if minio_cfg.force_path_style.unwrap_or(true) {
                    builder = builder.force_path_style(true);
                }
                Arc::new(S3ObjectStorage::new(aws_sdk_s3::Client::from_conf(
                    builder.build(),
                )))
            }
            _ => {
                let s3_client = {
                    let mut builder = if cfg.s3.is_some() {
                        let shared_config =
                            aws_config::defaults(aws_config::BehaviorVersion::latest())
                                .load()
                                .await;
                        aws_sdk_s3::config::Builder::from(&shared_config)
                    } else {
                        aws_sdk_s3::config::Builder::new().behavior_version_latest()
                    };
                    if let Some(s3_cfg) = &cfg.s3 {
                        if let Some(region) = &s3_cfg.region {
                            builder =
                                builder.region(aws_types::region::Region::new(region.clone()));
                        }
                        if let Some(endpoint) = &s3_cfg.endpoint {
                            builder = builder.endpoint_url(endpoint);
                        }
                        if s3_cfg.force_path_style.unwrap_or(false) {
                            builder = builder.force_path_style(true);
                        }
                    }
                    aws_sdk_s3::Client::from_conf(builder.build())
                };
                Arc::new(S3ObjectStorage::new(s3_client))
            }
        };

        let flow: Arc<dyn FlowRepo> = Arc::new(FlowRepository::new(pool.clone()));

        let flow_registry = Arc::new(
            flow_registry::build_registry(imports)
                .map_err(|e| backend_core::Error::Server(e.to_string()))?,
        );
        let user: Arc<dyn UserRepo> = Arc::new(UserRepository::new(pool.clone()));

        let signature_state = Arc::new(SignatureState {
            signature_secret: cfg.kc.signature_secret.clone(),
            max_clock_skew_seconds: cfg.kc.max_clock_skew_seconds,
            max_body_bytes: cfg.kc.max_body_bytes,
        });

        let notification_queue = Arc::new(RedisNotificationQueue::new(cfg.redis.url.clone()));

        let replay_guard = Arc::new(crate::auth_signature::RedisReplayGuard::new(
            cfg.redis.url.clone(),
        ));

        Ok(Self {
            flow,
            flow_registry,
            user,
            notification_queue,
            object_storage,
            config: cfg.clone(),
            signature_state,
            replay_guard,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::health_router;
    use axum::body::Body;
    use axum::http::Request;
    use backend_core::Config;
    use diesel_async::pooled_connection::AsyncDieselConnectionManager;
    use tower::util::ServiceExt;

    #[tokio::test]
    async fn test_app_state_from_config_minimal() {
        let config_yaml = r#"
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
"#;
        let cfg: Config = serde_yaml::from_str(config_yaml).unwrap();

        let manager = AsyncDieselConnectionManager::<diesel_async::AsyncPgConnection>::new(
            "postgres://localhost/test",
        );
        let pool = Pool::builder(manager).build().unwrap();

        let state = AppState::from_config(&cfg, pool, flow_registry::RegistryImports::default())
            .await
            .unwrap();

        assert_eq!(state.config.server.port, 8080);
    }

    #[tokio::test]
    async fn test_health_router_regression() {
        let app = health_router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), hyper::StatusCode::OK);
    }
}
