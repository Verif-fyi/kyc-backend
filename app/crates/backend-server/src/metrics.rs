//! Prometheus metrics endpoint.
//!
//! Installs a global Prometheus recorder and exposes a `GET /metrics` handler
//! that returns metrics in the standard Prometheus text format.

use axum::Router;
use axum::extract::State;
use axum::routing::get;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::sync::OnceLock;

static PROMETHEUS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Installs the global Prometheus metrics recorder.
///
/// Must be called once at application startup, before the HTTP server starts.
/// Panics if called more than once (safe due to `OnceLock`).
pub fn install_prometheus_recorder() -> PrometheusHandle {
    let handle = PrometheusBuilder::new()
        .install_recorder()
        .expect("failed to install Prometheus recorder");
    PROMETHEUS_HANDLE
        .set(handle.clone())
        .expect("Prometheus recorder already installed");
    handle
}

/// Builds a router with the `/metrics` endpoint.
///
/// # Arguments
/// * `handle` - The Prometheus handle returned by [`install_prometheus_recorder`]
///
/// # Returns
/// Axum `Router` with `GET /metrics` route
pub fn metrics_router(handle: PrometheusHandle) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(handle)
}

async fn metrics_handler(State(handle): State<PrometheusHandle>) -> String {
    handle.render()
}
