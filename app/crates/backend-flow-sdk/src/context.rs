use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct StepContext {
    pub session_id: String,
    pub session_user_id: Option<String>,
    pub flow_id: String,
    pub step_id: String,
    pub input: Value,
    pub session_context: Value,
    pub flow_context: Value,
    pub services: StepServices,
}

#[derive(Debug, Clone, Default)]
pub struct StepServices {
    pub storage: Option<Arc<dyn StorageService>>,
    pub user_lookup: Option<Arc<dyn UserLookupService>>,
    pub user_contact: Option<Arc<dyn UserContactService>>,
    pub config: Option<HashMap<String, Value>>,
}

#[async_trait::async_trait]
pub trait StorageService: Send + Sync + std::fmt::Debug {
    async fn generate_upload_url(
        &self,
        document_type: &str,
        session_id: &str,
    ) -> Result<UploadUrlResult, String>;
}

#[derive(Debug, Clone)]
pub struct UploadUrlResult {
    pub url: String,
    pub key: String,
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct UserRecord {
    pub user_id: String,
    pub username: String,
    pub full_name: Option<String>,
    pub email: Option<String>,
    pub phone_number: Option<String>,
    pub metadata: Value,
}

#[async_trait::async_trait]
pub trait UserLookupService: Send + Sync + std::fmt::Debug {
    async fn get_user(&self, user_id: &str) -> Result<Option<UserRecord>, String>;
}

#[async_trait::async_trait]
pub trait UserContactService: Send + Sync + std::fmt::Debug {
    async fn update_phone_number(&self, user_id: &str, phone_number: &str) -> Result<(), String>;
    async fn update_full_name(&self, user_id: &str, full_name: &str) -> Result<(), String>;
}

impl StepContext {
    pub fn previous_step_output(&self, step_type: &str) -> Option<&Value> {
        self.flow_context
            .get("step_output")
            .and_then(|v| v.get(step_type))
    }

    pub fn session_config(&self, key: &str) -> Option<&Value> {
        self.session_context.get(key)
    }

    pub fn flow_config(&self, key: &str) -> Option<&Value> {
        self.flow_context.get(key)
    }

    pub fn flow_pointer(&self, pointer: &str) -> Option<&Value> {
        self.flow_context.pointer(pointer)
    }

    pub fn session_pointer(&self, pointer: &str) -> Option<&Value> {
        self.session_context.pointer(pointer)
    }

    pub fn step_output_pointer(&self, step_name: &str, pointer: &str) -> Option<&Value> {
        let base = self
            .flow_context
            .get("step_output")
            .and_then(|v| v.get(step_name))?;

        if pointer.is_empty() {
            return Some(base);
        }

        if pointer.starts_with('/') {
            base.pointer(pointer)
        } else {
            base.get(pointer)
        }
    }

    pub fn step_config(&self, key: &str) -> Option<&Value> {
        self.services
            .config
            .as_ref()
            .and_then(|c| c.get(key))
            .or_else(|| self.input.get(key))
    }

    pub fn step_config_or<T: serde::de::DeserializeOwned + Default>(&self, key: &str) -> T {
        self.step_config(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn step_config_or_default<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
        default: T,
    ) -> T {
        self.step_config(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(default)
    }
}
