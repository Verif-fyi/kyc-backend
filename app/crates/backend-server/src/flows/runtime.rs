use backend_flow_sdk::{Flow, StepServices, UserContactService, UserLookupService, UserRecord};
use backend_repository::UserRepo;
use serde_json::Value;
use std::sync::Arc;

pub struct RepoUserLookup {
    user_repo: Arc<dyn UserRepo>,
}

impl RepoUserLookup {
    pub fn new(user_repo: Arc<dyn UserRepo>) -> Self {
        Self { user_repo }
    }
}

pub struct RepoUserContact {
    user_repo: Arc<dyn UserRepo>,
}

impl RepoUserContact {
    pub fn new(user_repo: Arc<dyn UserRepo>) -> Self {
        Self { user_repo }
    }
}

impl std::fmt::Debug for RepoUserLookup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepoUserLookup")
            .field("user_repo", &"<UserRepo>")
            .finish()
    }
}

impl std::fmt::Debug for RepoUserContact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepoUserContact")
            .field("user_repo", &"<UserRepo>")
            .finish()
    }
}

#[backend_core::async_trait]
impl UserLookupService for RepoUserLookup {
    async fn get_user(&self, user_id: &str) -> Result<Option<UserRecord>, String> {
        let user = self
            .user_repo
            .get_user(user_id)
            .await
            .map_err(|error| error.to_string())?;

        let Some(row) = user else {
            return Ok(None);
        };

        let metadata = self
            .user_repo
            .get_user_metadata(&row.user_id)
            .await
            .map_err(|error| error.to_string())?;

        Ok(Some(UserRecord {
            user_id: row.user_id,
            username: row.username,
            full_name: row.full_name,
            email: row.email,
            phone_number: row.phone_number,
            metadata,
        }))
    }
}

#[backend_core::async_trait]
impl UserContactService for RepoUserContact {
    async fn update_phone_number(&self, user_id: &str, phone_number: &str) -> Result<(), String> {
        self.user_repo
            .update_phone_number(user_id, phone_number)
            .await
            .map_err(|error| error.to_string())
    }

    async fn update_full_name(&self, user_id: &str, full_name: &str) -> Result<(), String> {
        self.user_repo
            .update_full_name(user_id, full_name)
            .await
            .map_err(|error| error.to_string())
    }
}

pub fn step_services(user_repo: Arc<dyn UserRepo>) -> StepServices {
    StepServices {
        user_lookup: Some(Arc::new(RepoUserLookup::new(user_repo.clone()))),
        user_contact: Some(Arc::new(RepoUserContact::new(user_repo))),
        ..Default::default()
    }
}

pub fn merge_json_value(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base_obj), Value::Object(patch_obj)) => {
            for (key, value) in patch_obj {
                if value.is_null() {
                    base_obj.remove(key);
                    continue;
                }

                if let Some(existing) = base_obj.get_mut(key) {
                    merge_json_value(existing, value);
                } else {
                    base_obj.insert(key.clone(), value.clone());
                }
            }
        }
        (slot, value) => {
            *slot = value.clone();
        }
    }
}

pub fn merged_json(mut base: Value, patch: &Value) -> Value {
    merge_json_value(&mut base, patch);
    base
}

pub fn resolve_transition(
    flow: &dyn Flow,
    step_type: &str,
    branch: Option<&str>,
    failed: bool,
) -> Option<String> {
    let transition = flow.transitions().get(step_type)?;
    if let Some(branch_name) = branch
        && let Some(target) = transition.branches.get(branch_name)
    {
        return Some(target.clone());
    }

    if failed {
        return transition.on_failure.clone();
    }

    Some(transition.on_success.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use backend_flow_sdk::{Actor, Step, StepTransition};
    use std::collections::HashMap;

    struct TestStep;

    #[async_trait::async_trait]
    impl Step for TestStep {
        fn step_type(&self) -> &str {
            "start"
        }

        fn actor(&self) -> Actor {
            Actor::System
        }

        fn human_id(&self) -> &str {
            "start"
        }
    }

    struct TestFlow {
        transitions: HashMap<String, StepTransition>,
        steps: Vec<std::sync::Arc<dyn Step>>,
    }

    impl Flow for TestFlow {
        fn flow_type(&self) -> &str {
            "test"
        }

        fn human_id(&self) -> &str {
            "test"
        }

        fn feature(&self) -> Option<&str> {
            None
        }

        fn steps(&self) -> &[std::sync::Arc<dyn Step>] {
            &self.steps
        }

        fn initial_step(&self) -> &str {
            "start"
        }

        fn transitions(&self) -> &HashMap<String, StepTransition> {
            &self.transitions
        }
    }

    #[test]
    fn resolve_transition_prefers_named_branch() {
        let mut branches = HashMap::new();
        branches.insert("approved".to_owned(), "approve".to_owned());
        let flow = TestFlow {
            transitions: HashMap::from([(
                "start".to_owned(),
                StepTransition {
                    on_success: "next".to_owned(),
                    on_failure: Some("FAILED".to_owned()),
                    branches,
                },
            )]),
            steps: vec![std::sync::Arc::new(TestStep)],
        };

        assert_eq!(
            resolve_transition(&flow, "start", Some("approved"), false).as_deref(),
            Some("approve")
        );
        assert_eq!(
            resolve_transition(&flow, "start", None, true).as_deref(),
            Some("FAILED")
        );
    }

    #[test]
    fn resolve_transition_falls_back_to_success_branch() {
        let flow = TestFlow {
            transitions: HashMap::from([(
                "start".to_owned(),
                StepTransition {
                    on_success: "next".to_owned(),
                    on_failure: Some("FAILED".to_owned()),
                    branches: HashMap::new(),
                },
            )]),
            steps: vec![std::sync::Arc::new(TestStep)],
        };

        assert_eq!(
            resolve_transition(&flow, "start", Some("missing"), false).as_deref(),
            Some("next")
        );
    }
}
