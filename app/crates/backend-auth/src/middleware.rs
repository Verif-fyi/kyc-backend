//! Authentication middleware for HTTP requests.
//!
//! Provides middleware layers for:
//! - HMAC signature verification for all API surfaces

use crate::signature_principal::SignatureState;
use axum::body::{Body, to_bytes};
use axum::extract::OriginalUri;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};
use tracing::{debug, instrument};

/// Validates HMAC signature on incoming requests.
/// Returns the request if valid, or an error response if signature is invalid or missing.
#[instrument(skip(state, req))]
pub async fn require_signature(
    enabled: bool,
    state: &SignatureState,
    req: Request<Body>,
) -> Result<Request<Body>, Response> {
    if !enabled {
        return Ok(req);
    }

    let method = req.method().clone();
    let uri = req
        .extensions()
        .get::<OriginalUri>()
        .map(|u| u.0.clone())
        .unwrap_or_else(|| req.uri().clone());

    debug!("Checking signature for {} {}", method, uri.path());

    let (parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, state.max_body_bytes).await {
        Ok(value) => value,
        Err(_) => return Err(unauthorized("invalid request body")),
    };

    if let Err(e) = state.verify_signature(&method, &uri, &parts.headers, &body_bytes) {
        return Err(unauthorized(&e.to_string()));
    }

    Ok(Request::from_parts(parts, Body::from(body_bytes)))
}

pub fn signature_layer(enabled: bool, state: Arc<SignatureState>) -> SignatureLayer {
    SignatureLayer::new(enabled, state)
}

#[derive(Clone)]
pub struct SignatureLayer {
    enabled: bool,
    state: Arc<SignatureState>,
}

impl SignatureLayer {
    fn new(enabled: bool, state: Arc<SignatureState>) -> Self {
        Self { enabled, state }
    }
}

impl<S> Layer<S> for SignatureLayer {
    type Service = SignatureService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        SignatureService {
            inner,
            enabled: self.enabled,
            state: Arc::clone(&self.state),
        }
    }
}

#[derive(Clone)]
pub struct SignatureService<S> {
    inner: S,
    enabled: bool,
    state: Arc<SignatureState>,
}

impl<S> Service<Request<Body>> for SignatureService<S>
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let enabled = self.enabled;
        let state = Arc::clone(&self.state);
        let mut inner = self.inner.clone();

        Box::pin(async move {
            match require_signature(enabled, &state, req).await {
                Ok(req) => inner.call(req).await,
                Err(resp) => Ok(resp),
            }
        })
    }
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({
            "error": "unauthorized",
            "message": message
        })),
    )
        .into_response()
}
