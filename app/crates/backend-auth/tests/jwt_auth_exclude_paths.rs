use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode};
use axum::{body::to_bytes, response::Response};
use backend_auth::{
    SignatureState, signature_layer,
    require_signature,
};
use backend_core::KcAuth;
use base64::Engine;
use ring::hmac;
use serde_json::Value;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

fn build_kc_auth() -> KcAuth {
    KcAuth {
        enabled: true,
        base_path: "/v1".to_owned(),
        signature_secret: "test-secret".to_owned(),
        max_clock_skew_seconds: 120,
        max_body_bytes: 1024,
    }
}

fn build_signature_state(cfg: &KcAuth) -> SignatureState {
    SignatureState {
        signature_secret: cfg.signature_secret.clone(),
        max_clock_skew_seconds: cfg.max_clock_skew_seconds,
        max_body_bytes: cfg.max_body_bytes,
    }
}

async fn read_error_body(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn hmac_signature(secret: &str, timestamp: i64, method: &str, path: &str, body: &str) -> String {
    let payload = format!("{timestamp}\n{}\n{path}\n{body}", method.to_uppercase());
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let digest = hmac::sign(&key, payload.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.as_ref())
}

#[tokio::test]
async fn signature_bypasses_when_disabled() {
    let mut cfg = build_kc_auth();
    cfg.enabled = false;
    let request = Request::builder()
        .uri("/v1/users")
        .body(Body::empty())
        .unwrap();

    let state = build_signature_state(&cfg);
    let result = require_signature(cfg.enabled, &state, request).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn signature_rejects_when_timestamp_header_is_missing() {
    let cfg = build_kc_auth();
    let mut request = Request::builder()
        .uri("/v1/users")
        .body(Body::empty())
        .unwrap();
    request
        .headers_mut()
        .insert("x-auth-signature", HeaderValue::from_static("any"));

    let state = build_signature_state(&cfg);
    let result = require_signature(cfg.enabled, &state, request).await;

    assert!(result.is_err());
    let payload = read_error_body(result.err().unwrap()).await;
    assert_eq!(payload["message"], "Missing x-auth-timestamp");
}

#[tokio::test]
async fn signature_rejects_when_signature_header_is_missing() {
    let cfg = build_kc_auth();
    let request = Request::builder()
        .uri("/v1/users")
        .body(Body::empty())
        .unwrap();

    let state = build_signature_state(&cfg);
    let result = require_signature(cfg.enabled, &state, request).await;

    assert!(result.is_err());
    let payload = read_error_body(result.err().unwrap()).await;
    assert_eq!(payload["message"], "Missing x-auth-signature");
}

#[tokio::test]
async fn signature_layer_rejects_requests_without_headers() {
    let cfg = build_kc_auth();
    let state = Arc::new(build_signature_state(&cfg));
    let router = Router::new()
        .route("/v1/users", axum::routing::post(|| async { "ok" }))
        .layer(signature_layer(cfg.enabled, state));

    let response = router
        .oneshot(
            Request::builder()
                .uri("/v1/users")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn signature_accepts_valid_signature() {
    let cfg = build_kc_auth();
    let timestamp = now_unix_seconds();
    let body = "{\"hello\":\"world\"}";
    let signature = hmac_signature(&cfg.signature_secret, timestamp, "POST", "/v1/users", body);
    
    let state = Arc::new(build_signature_state(&cfg));
    let router = Router::new()
        .route("/v1/users", axum::routing::post(|| async { "ok" }))
        .layer(signature_layer(cfg.enabled, state));

    let mut request = Request::builder()
        .method("POST")
        .uri("/v1/users")
        .body(Body::from(body))
        .unwrap();
    request.headers_mut().insert(
        "x-auth-timestamp",
        HeaderValue::from_str(&timestamp.to_string()).unwrap(),
    );
    request
        .headers_mut()
        .insert("x-auth-signature", HeaderValue::from_str(&signature).unwrap());

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
