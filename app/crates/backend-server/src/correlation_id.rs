//! Correlation ID middleware.
//!
//! Extracts `X-Correlation-ID` from incoming requests (or generates a new CUID
//! if absent), records it on the current tracing span, and returns it in the
//! response header so callers can correlate logs across services.

use axum::body::Body;
use axum::http::{HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;
use tracing::Span;

/// Axum middleware that propagates or generates a correlation ID.
///
/// Reads `X-Correlation-ID` from the request headers. If absent, generates
/// a new CUID. The value is:
/// - Recorded on the active tracing span as `correlation_id`
/// - Returned on the response as `X-Correlation-ID`
pub async fn correlation_id_middleware(req: Request<Body>, next: Next) -> Response {
    let correlation_id = req
        .headers()
        .get("X-Correlation-ID")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned())
        .unwrap_or_else(|| {
            cuid::cuid1().unwrap_or_else(|_| {
                // Fallback: generate a random 16-byte hex string so every request
                // gets a unique ID even when cuid generation fails.
                let mut bytes = [0u8; 16];
                let _ = getrandom::fill(&mut bytes);
                bytes.iter().map(|b| format!("{b:02x}")).collect()
            })
        });

    Span::current().record("correlation_id", correlation_id.as_str());

    let mut response = next.run(req).await;

    if let Ok(value) = HeaderValue::from_str(&correlation_id) {
        response.headers_mut().insert("X-Correlation-ID", value);
    }

    response
}
