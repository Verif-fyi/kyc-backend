//! Configuration management for the tokenization backend.
//!
//! This module provides configuration structures that are loaded from YAML files
//! with environment variable expansion support. Configuration is hierarchical
//! and covers all aspects of the application including server settings,
//! authentication, storage, and external service integrations.

use crate::error::Result;
use backend_env::envsubst;
use serde::Deserialize;
use serde_yaml::from_str;
use std::fs::read_to_string;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Application runtime mode determining which components to start.
///
/// - Server: Start only the HTTP API server
/// - Worker: Start only the background worker for async tasks
/// - Shared: Start both server and worker in the same process
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
pub enum RuntimeMode {
    Server,
    Worker,
    #[default]
    Shared,
}

/// Runtime configuration controlling application mode.
#[derive(Debug, Clone, Deserialize)]
pub struct Runtime {
    #[serde(default)]
    pub mode: RuntimeMode,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::Shared,
        }
    }
}

/// Redis connection configuration for caching and queues.
#[derive(Debug, Clone, Deserialize)]
pub struct Redis {
    pub url: String,
    #[serde(
        default = "default_worker_lock_ttl_seconds",
        alias = "worker-lock-ttl-seconds"
    )]
    pub worker_lock_ttl_seconds: i64,
    #[serde(
        default = "default_worker_lock_renew_seconds",
        alias = "worker-lock-renew-seconds"
    )]
    pub worker_lock_renew_seconds: u64,
}

impl Default for Redis {
    fn default() -> Self {
        Self {
            url: "redis://127.0.0.1:6379".to_owned(),
            worker_lock_ttl_seconds: default_worker_lock_ttl_seconds(),
            worker_lock_renew_seconds: default_worker_lock_renew_seconds(),
        }
    }
}

/// HTTP server configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Server {
    pub address: String,
    pub port: u16,
    pub tls: Tls,
}

/// Logging configuration with support for structured JSON output.
#[derive(Debug, Clone, Deserialize)]
pub struct Logging {
    pub level: String,
    pub data_dir: Option<String>,
    pub json: Option<bool>,
    #[serde(default)]
    pub log_requests_enabled: bool,
}

/// PostgreSQL database connection configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Database {
    pub url: String,
    pub pool_size: Option<u32>,
}

/// OAuth2/OIDC configuration for JWT verification.
#[derive(Debug, Clone, Deserialize)]
pub struct Oauth2 {
    pub issuer: String,
    #[serde(default, alias = "jwks-uri")]
    pub jwks_uri: Option<String>,
    #[serde(default, alias = "base-paths")]
    pub base_paths: Vec<String>,
}

/// Swagger UI and OpenAPI documentation configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct SwaggerConfig {
    /// HTTP host URL for the API server (e.g., "http://localhost:3000").
    /// Used to configure server URLs in OpenAPI specs.
    #[serde(alias = "http-host")]
    pub http_host: Option<String>,
    /// OAuth2 client configuration for Swagger UI authentication.
    #[serde(default)]
    pub oauth2_client: Option<SwaggerOauth2Client>,
}

impl Default for SwaggerConfig {
    fn default() -> Self {
        Self {
            http_host: None,
            oauth2_client: None,
        }
    }
}

/// OAuth2 client credentials for Swagger UI to authenticate against the IdP.
#[derive(Debug, Clone, Deserialize)]
pub struct SwaggerOauth2Client {
    #[serde(default, alias = "token-url")]
    pub token_url: Option<String>
}

#[derive(Debug, Clone, Deserialize)]
pub struct AwsS3 {
    /// Optional region override for S3.
    pub region: Option<String>,
    /// Optional region override for S3.
    pub force_path_style: Option<bool>,
    pub bucket: String,
    /// Optional custom S3 endpoint (e.g., LocalStack).
    pub endpoint: Option<String>,
    /// TTL for S3 presigned URLs.
    pub presign_ttl_seconds: u64,
}

/// Storage backend type selection.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageType {
    S3,
    Minio,
}

/// MinIO/S3-compatible storage configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct MinioStorage {
    pub endpoint: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub bucket: String,
    pub force_path_style: Option<bool>,
    pub presign_ttl_seconds: u64,
}

/// Storage configuration wrapper that selects between S3 and MinIO.
#[derive(Debug, Clone, Deserialize)]
pub struct Storage {
    #[serde(rename = "type")]
    pub r#type: StorageType,
    #[serde(default)]
    pub minio: Option<MinioStorage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AwsSns {
    /// Optional region override for SNS.
    pub region: Option<String>,
    /// Maximum publish attempts before giving up.
    pub max_attempts: u32,
    /// Initial retry backoff in seconds.
    pub initial_backoff_seconds: u64,
}

/// SMS provider selection for OTP delivery.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SmsProviderType {
    Console,
    Sns,
    Api,
    Avlytext,
    Orange,
}

/// SMS configuration for OTP delivery.
#[derive(Debug, Clone, Deserialize)]
pub struct SmsConfig {
    pub provider: SmsProviderType,
    #[serde(default)]
    pub api: Option<SmsApi>,
    #[serde(default)]
    pub avlytext: Option<AvlytextConfig>,
    #[serde(default)]
    pub orange: Option<OrangeConfig>,
}

/// Third-party SMS API configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct SmsApi {
    pub base_url: String,
    pub auth_token: Option<String>,
}

/// Avlytext SMS provider configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct AvlytextConfig {
    pub base_url: String,
    pub api_key: String,
    pub sender_id: String,
}

/// Orange SMS provider configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct OrangeConfig {
    pub client_id: String,
    pub client_secret: String,
    pub token_url: String,
    pub sms_base_url: String,
    pub contract_url: String,
    pub default_sender: String,
}

/// Flow configuration for YAML-based flow and session definitions.
#[derive(Debug, Clone, Deserialize)]
pub struct FlowConfig {
    #[serde(default = "default_flows_dir")]
    pub flows_dir: String,
    #[serde(default = "default_sessions_dir")]
    pub sessions_dir: String,
}

fn default_flows_dir() -> String {
    "flows".to_owned()
}

fn default_sessions_dir() -> String {
    "sessions".to_owned()
}

fn default_worker_lock_ttl_seconds() -> i64 {
    30
}

fn default_worker_lock_renew_seconds() -> u64 {
    10
}

impl Default for FlowConfig {
    fn default() -> Self {
        Self {
            flows_dir: default_flows_dir(),
            sessions_dir: default_sessions_dir(),
        }
    }
}

/// Main application configuration containing all sub-systems.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: Server,
    pub logging: Logging,
    pub database: Database,
    pub oauth2: Oauth2,
    #[serde(default)]
    pub runtime: Runtime,
    #[serde(default)]
    pub redis: Redis,
    pub s3: Option<AwsS3>,
    #[serde(default)]
    pub storage: Option<Storage>,
    pub sns: Option<AwsSns>,
    pub sms: Option<SmsConfig>,

    pub kc: KcAuth,
    pub bff: BffAuth,
    pub staff: StaffAuth,
    #[serde(default)]
    pub auth: AuthApi,
    pub deposit_flow: Option<DepositFlow>,
    pub cuss: Cuss,
    #[serde(default)]
    pub flow: FlowConfig,
    #[serde(default)]
    pub swagger: SwaggerConfig,
}

/// TLS certificate configuration for HTTPS.
#[derive(Debug, Clone, Deserialize)]
pub struct Tls {
    pub cert_path: String,
    pub key_path: String,
}

/// Keycloak API surface authentication configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct KcAuth {
    pub enabled: bool,
    #[serde(alias = "base-path")]
    pub base_path: String,
    pub signature_secret: String,
    pub max_clock_skew_seconds: i64,
    pub max_body_bytes: usize,
}

/// BFF (Backend-for-Frontend) API surface authentication configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct BffAuth {
    pub enabled: bool,
    #[serde(alias = "base-path")]
    pub base_path: String,
}

/// Staff API surface authentication configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct StaffAuth {
    pub enabled: bool,
    #[serde(alias = "base-path")]
    pub base_path: String,
}

/// Auth API surface configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthApi {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_auth_base_path", alias = "base-path")]
    pub base_path: String,
    #[serde(default)]
    pub token_issuer: Option<String>,
    #[serde(default)]
    pub token_audience: Option<String>,
    #[serde(default = "default_token_ttl_seconds")]
    pub token_ttl_seconds: i64,
    #[serde(default = "default_auth_max_clock_skew_seconds")]
    pub max_clock_skew_seconds: i64,
}

impl Default for AuthApi {
    fn default() -> Self {
        Self {
            enabled: true,
            base_path: default_auth_base_path(),
            token_issuer: None,
            token_audience: None,
            token_ttl_seconds: default_token_ttl_seconds(),
            max_clock_skew_seconds: default_auth_max_clock_skew_seconds(),
        }
    }
}

/// Deposit flow routing configuration for provider-specific recipients.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DepositFlow {
    #[serde(default)]
    pub staff: DepositFlowStaff,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DepositFlowStaff {
    #[serde(default)]
    pub recipients: Vec<DepositRecipient>,
}

/// Static recipient row loaded from YAML and synced into app_deposit_recipients.
#[derive(Debug, Clone, Deserialize)]
pub struct DepositRecipient {
    pub provider: String,
    #[serde(rename = "fullname", alias = "full-name")]
    pub full_name: String,
    #[serde(alias = "phone-number")]
    pub phone_number: String,
    pub regex: String,
    pub currency: String,
}

/// CUSS (Customer Service System) integration configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Cuss {
    pub api_url: String,
}

fn default_true() -> bool {
    true
}

fn default_auth_base_path() -> String {
    "/auth".to_owned()
}

fn default_token_ttl_seconds() -> i64 {
    3600
}

fn default_auth_max_clock_skew_seconds() -> i64 {
    60
}

/// Load configuration from a YAML file with environment variable expansion.
///
/// Uses `envsubst`-style substitution for `$VAR` and `${VAR}`.
/// Missing variables expand to an empty string and shell default syntax is left untouched.
pub fn load_from_path<P: AsRef<std::path::Path>>(path: P) -> Result<Config> {
    let content = read_to_string(path)?;
    let expanded = envsubst(&content);
    let cfg: Config = from_str(&expanded)?;
    Ok(cfg)
}

impl Config {
    /// Returns the combined address and port for the HTTP server to listen on.
    pub fn api_listen_addr(&self) -> Result<SocketAddr> {
        Ok(format!("{}:{}", self.server.address, self.server.port).parse()?)
    }

    /// Returns the TLS certificate and key file paths if both files exist.
    /// Returns None if either file is missing, indicating TLS should be disabled.
    pub fn api_tls_files(&self) -> Option<(PathBuf, PathBuf)> {
        let cert_path: PathBuf = self.server.tls.cert_path.clone().into();
        let key_path: PathBuf = self.server.tls.key_path.clone().into();

        if Path::new(&cert_path).exists() && Path::new(&key_path).exists() {
            Some((cert_path, key_path))
        } else {
            None
        }
    }

    /// Returns the database connection pool size, defaulting to 10 if not configured.
    pub fn database_pool_size(&self) -> u32 {
        self.database.pool_size.unwrap_or(10)
    }
}

#[cfg(test)]
mod tests {
    use backend_env::envsubst;
    use std::env;
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn test_envsubst_expands_variables() {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        unsafe { env::set_var("TEST_VAR_1", "value1") };
        let expanded = envsubst("var1: $TEST_VAR_1, var2: ${TEST_VAR_1}");
        assert_eq!(expanded, "var1: value1, var2: value1");
    }

    #[test]
    fn test_envsubst_missing_is_empty() {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        unsafe { env::remove_var("TEST_ENV_MISSING") };
        let expanded = envsubst("endpoint: $TEST_ENV_MISSING");
        assert_eq!(expanded, "endpoint: ");
    }

    #[test]
    fn test_envsubst_leaves_default_syntax_unchanged() {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        unsafe { env::set_var("TEST_VAR_WITH_DEFAULT", "http://override:9000") };
        let content = "endpoint: ${TEST_VAR_WITH_DEFAULT:-http://minio:9000}";
        assert_eq!(envsubst(content), content);
    }
}
