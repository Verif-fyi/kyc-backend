//! Main backend server library for the tokenization backend.
#![allow(clippy::result_large_err)]
//!
//! This crate provides the HTTP server, API surfaces (BFF, Staff),
//! state machine engine, background workers, and all application logic.
//! It handles user storage, KYC flows, and integrations.

pub(crate) mod api;
pub(crate) mod auth_signature;
pub(crate) mod flows;
pub(crate) mod health;
pub mod metrics;
pub(crate) mod object_storage;
pub(crate) mod state;
pub(crate) mod swagger;
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
pub(crate) mod worker;
pub use flows::registry as flow_registry;

use axum::Router;
use axum::body::Body;
use axum::http::Request as HttpRequest;
use axum::response::Response;
use axum_server::tls_rustls::RustlsConfig;
use backend_auth::signature_layer;
use backend_core::{Config, Result};
use backend_migrate::connect_postgres_and_migrate;
use hyper::StatusCode;
use rustls::server::WebPkiClientVerifier;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing::{info, error};

/// Starts the HTTP server with all API surfaces and background workers.
pub async fn serve(core_config: &Config, imports: flow_registry::RegistryImports) -> Result<()> {
    let listen_addr = core_config.api_listen_addr()?;
    let _prometheus_handle = metrics::install_prometheus_recorder();
    let pool = connect_postgres_and_migrate(&core_config.database.url).await?;
    let state = Arc::new(state::AppState::from_config(core_config, pool, imports).await?);

    let api = api::BackendApi::new(
        state.clone(),
        state.signature_state.clone(),
    );
    let app = build_router(api, &state.config);

    info!("Listening on {}", listen_addr);

    let handle = axum_server::Handle::new();
    let shutdown_handle = handle.clone();

    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        shutdown_handle.graceful_shutdown(None);
    });

    match core_config.api_tls_files() {
        Some((cert_path, key_path)) => {
            let mut rustls_config =
                RustlsConfig::from_pem_file(cert_path.clone(), key_path.clone()).await?;

            if let Some(client_ca_path) = &core_config.server.tls.client_ca_path {
                info!("Enabling mTLS with client CA: {}", client_ca_path);
                let ca_file = std::fs::File::open(client_ca_path).map_err(|e| {
                    error!("Failed to open client CA file: {}", e);
                    backend_core::Error::Server(e.to_string())
                })?;
                let mut reader = std::io::BufReader::new(ca_file);
                let certs = rustls_pemfile::certs(&mut reader)
                    .map(|res| res.map_err(|e| e.into()))
                    .collect::<std::result::Result<Vec<_>, backend_core::Error>>()?;

                let mut root_store = rustls::RootCertStore::empty();
                for cert in certs {
                    root_store.add(cert).map_err(|e| {
                        error!("Failed to add client CA cert: {}", e);
                        backend_core::Error::Server(e.to_string())
                    })?;
                }

                let verifier = WebPkiClientVerifier::builder(Arc::new(root_store))
                    .build()
                    .map_err(|e| {
                        error!("Failed to build client cert verifier: {}", e);
                        backend_core::Error::Server(e.to_string())
                    })?;

                // Load server cert and key again for manual config
                let cert_file = std::fs::File::open(cert_path).map_err(|e| backend_core::Error::Server(e.to_string()))?;
                let mut cert_reader = std::io::BufReader::new(cert_file);
                let server_certs = rustls_pemfile::certs(&mut cert_reader)
                    .map(|res| res.map_err(|e| e.into()))
                    .collect::<std::result::Result<Vec<_>, backend_core::Error>>()?;

                let key_file = std::fs::File::open(key_path).map_err(|e| backend_core::Error::Server(e.to_string()))?;
                let mut key_reader = std::io::BufReader::new(key_file);
                let key = rustls_pemfile::private_key(&mut key_reader)
                    .map_err(|e| backend_core::Error::Server(e.to_string()))?
                    .ok_or_else(|| backend_core::Error::Server("No private key found".to_string()))?;

                let server_config = rustls::ServerConfig::builder()
                    .with_client_cert_verifier(verifier)
                    .with_single_cert(server_certs, key)
                    .map_err(|e| {
                        error!("Failed to create mTLS server config: {}", e);
                        backend_core::Error::Server(e.to_string())
                    })?;
                rustls_config = RustlsConfig::from_config(Arc::new(server_config));
            }

            axum_server::bind_rustls(listen_addr, rustls_config)
                .handle(handle)
                .serve(app.into_make_service())
                .await?;
        }
        None => {
            axum_server::bind(listen_addr)
                .handle(handle)
                .serve(app.into_make_service())
                .await?;
        }
    }

    Ok(())
}

/// Runs the background worker for async tasks and state machine processing.
pub async fn run_worker(
    core_config: &Config,
    imports: flow_registry::RegistryImports,
) -> Result<()> {
    let pool = connect_postgres_and_migrate(&core_config.database.url).await?;
    let _conn = pool
        .get()
        .await
        .map_err(|error| backend_core::Error::DieselPool(error.to_string()))?;
    worker::ensure_redis_ready(&core_config.redis.url).await?;
    let worker_lock = worker::acquire_worker_consumer_lock_with_retry(
        &core_config.redis.url,
        core_config.redis.worker_lock_ttl_seconds,
        core_config.redis.worker_lock_renew_seconds,
    )
    .await?;

    let state = Arc::new(state::AppState::from_config(core_config, pool, imports).await?);

    let health_server = if core_config.runtime.mode == backend_core::RuntimeMode::Worker {
        let listen_addr = core_config.api_listen_addr()?;
        let health_app = health::health_router();

        info!("Worker health check listening on {}", listen_addr);

        let handle = axum_server::Handle::new();
        let shutdown_handle = handle.clone();

        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown_handle.graceful_shutdown(None);
        });

        let tls_files = core_config.api_tls_files();
        Some(tokio::spawn(async move {
            match tls_files {
                Some((cert_path, key_path)) => {
                    let rustls_config =
                        axum_server::tls_rustls::RustlsConfig::from_pem_file(cert_path, key_path)
                            .await
                            .expect("failed to load tls config for worker health");

                    axum_server::bind_rustls(listen_addr, rustls_config)
                        .handle(handle)
                        .serve(health_app.into_make_service())
                        .await
                        .expect("worker health server failed");
                }
                None => {
                    axum_server::bind(listen_addr)
                        .handle(handle)
                        .serve(health_app.into_make_service())
                        .await
                        .expect("worker health server failed");
                }
            }
        }))
    } else {
        None
    };

    let worker_res = worker::run(state).await;
    if let Err(error) = worker_lock.release().await {
        tracing::warn!("failed to release worker consumer lock: {}", error);
    }
    if let Some(hs) = health_server {
        hs.abort();
    }
    worker_res
}

/// Builds the main Axum router with all API surfaces and middleware.
fn build_router(
    api: api::BackendApi,
    config: &Config,
) -> Router {
    let mut router = Router::new();

    // Global signature layer for all API surfaces
    let sig_layer = signature_layer(config.kc.enabled, api.signature_state.clone());

    // Mount BFF router
    let bff_base = config.bff.base_path.trim();
    if !bff_base.is_empty() && bff_base != "/" {
        let bff_router = Router::new()
            .merge(api::bff_flow::router(api.clone()))
            .merge(api::bff_uploads::router(api.clone()));
        router = router.nest(bff_base, bff_router);
    }

    // Mount Staff router
    let staff_base = config.staff.base_path.trim();
    if !staff_base.is_empty() && staff_base != "/" {
        let staff_router = Router::new().nest("/flow", api::staff_flow::router(api.clone()));
        router = router.nest(staff_base, staff_router);
    }

    // Apply the signature layer to all routers (except health and swagger)
    router = router.layer(sig_layer);

    // Merge health router AFTER signature layer so it's public
    router = router.merge(health::health_router());

    // Mount Swagger UI
    router = router.merge(Into::<Router>::into(swagger::swagger_ui(&config)));

    // 404 fallback
    router = router.fallback(|| async { (StatusCode::NOT_FOUND, "Not Found") });

    if config.logging.log_requests_enabled {
        router.layer(
            TraceLayer::new_for_http()
                .make_span_with(|req: &HttpRequest<_>| {
                    let remote_addr = req
                        .extensions()
                        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                        .map(|ci| ci.0.to_string())
                        .unwrap_or_else(|| "unknown".to_owned());
                    let correlation_id = req
                        .headers()
                        .get("X-Correlation-ID")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("-");
                    tracing::info_span!(
                        "http-request",
                        method = %req.method(),
                        path = %request_path(req),
                        remote_addr = %remote_addr,
                        correlation_id = %correlation_id,
                    )
                })
                .on_request(|req: &HttpRequest<_>, _span: &tracing::Span| {
                    tracing::info!(
                        headers = ?req.headers(),
                        "received request"
                    )
                })
                .on_response(
                    |res: &Response, latency: std::time::Duration, _span: &tracing::Span| {
                        tracing::info!(
                            status = %res.status(),
                            latency = ?latency,
                            "sending response"
                        );
                    },
                ),
        )
    } else {
        router
    }
}

fn request_path(req: &HttpRequest<Body>) -> String {
    req.extensions()
        .get::<axum::extract::OriginalUri>()
        .map(|uri| uri.0.path().to_owned())
        .unwrap_or_else(|| req.uri().path().to_owned())
}
