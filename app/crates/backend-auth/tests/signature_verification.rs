use axum::http::{HeaderMap, Method, Uri};
use backend_auth::SignatureState;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

#[test]
fn test_signature_verification_success() {
    let secret = "test-secret";
    let state = SignatureState {
        signature_secret: secret.to_string(),
        max_clock_skew_seconds: 60,
        max_body_bytes: 1024,
    };

    let timestamp = chrono::Utc::now().timestamp().to_string();
    let method = Method::POST;
    let uri = "/kc/enroll".parse::<Uri>().unwrap();
    let body = r#"{"device_id":"dvc_123"}"#;

    let canonical_payload = format!(
        "{}\n{}\n{}\n{}",
        timestamp,
        method.as_str().to_uppercase(),
        uri.path(),
        body
    );

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(canonical_payload.as_bytes());
    let signature =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

    let mut headers = HeaderMap::new();
    headers.insert("x-auth-signature", signature.parse().unwrap());
    headers.insert("x-auth-timestamp", timestamp.parse().unwrap());

    let result = state.verify_signature(&method, &uri, &headers, body.as_bytes());
    assert!(result.is_ok());
}

#[test]
fn test_signature_verification_invalid_signature() {
    let secret = "test-secret";
    let state = SignatureState {
        signature_secret: secret.to_string(),
        max_clock_skew_seconds: 60,
        max_body_bytes: 1024,
    };

    let timestamp = chrono::Utc::now().timestamp().to_string();
    let method = Method::POST;
    let uri = "/kc/enroll".parse::<Uri>().unwrap();
    let body = r#"{"device_id":"dvc_123"}"#;

    let mut headers = HeaderMap::new();
    headers.insert("x-auth-signature", "invalid-signature".parse().unwrap());
    headers.insert("x-auth-timestamp", timestamp.parse().unwrap());

    let result = state.verify_signature(&method, &uri, &headers, body.as_bytes());
    assert!(result.is_err());
}

#[test]
fn test_signature_verification_timestamp_skew() {
    let secret = "test-secret";
    let state = SignatureState {
        signature_secret: secret.to_string(),
        max_clock_skew_seconds: 60,
        max_body_bytes: 1024,
    };

    let timestamp = (chrono::Utc::now().timestamp() - 100).to_string();
    let method = Method::POST;
    let uri = "/kc/enroll".parse::<Uri>().unwrap();
    let body = r#"{"device_id":"dvc_123"}"#;

    let mut headers = HeaderMap::new();
    headers.insert("x-auth-signature", "some-sig".parse().unwrap());
    headers.insert("x-auth-timestamp", timestamp.parse().unwrap());

    let result = state.verify_signature(&method, &uri, &headers, body.as_bytes());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("skew"));
}
