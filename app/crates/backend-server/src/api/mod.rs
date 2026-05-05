pub mod bff_flow;
pub mod bff_uploads;
pub mod staff_flow;

use crate::state::AppState;
use backend_auth::SignatureState;
use std::sync::Arc;

pub(crate) const BFF_AUTH_USER_ID_HEADER: &str = "x-bff-authenticated-user-id";

#[derive(Clone)]
pub struct BackendApi {
    pub(crate) state: Arc<AppState>,
    pub(crate) signature_state: Arc<SignatureState>,
}

impl AsRef<Self> for BackendApi {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl BackendApi {
    pub fn new(
        state: Arc<AppState>,
        signature_state: Arc<SignatureState>,
    ) -> Self {
        Self {
            state,
            signature_state,
        }
    }
}
