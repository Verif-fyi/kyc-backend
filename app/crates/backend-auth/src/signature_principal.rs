//! HMAC signature verification for API surfaces.
//!
//! This module provides signature verification for requests.
//! The signature is an HMAC-SHA256 over a canonical payload format.

use axum::http::{HeaderMap, Method, Uri};
use backend_core::{Error, Result};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tracing::instrument;

/// State for verifying request signatures.
#[derive(Clone, Debug)]
pub struct SignatureState {
    /// Shared secret for HMAC verification
    pub signature_secret: String,
    /// Maximum allowed clock skew in seconds (for timestamp validation)
    pub max_clock_skew_seconds: i64,
    /// Maximum allowed body size in bytes
    pub max_body_bytes: usize,
}

impl SignatureState {
    #[instrument(skip(self, body))]
    pub fn verify_signature(
        &self,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<()> {
        let signature = headers
            .get("x-auth-signature")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| Error::unauthorized("Missing x-auth-signature"))?;

        let timestamp_str = headers
            .get("x-auth-timestamp")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| Error::unauthorized("Missing x-auth-timestamp"))?;

        let timestamp: i64 = timestamp_str
            .parse()
            .map_err(|_| Error::unauthorized("Invalid x-auth-timestamp"))?;

        let now = chrono::Utc::now().timestamp();
        if (now - timestamp).abs() > self.max_clock_skew_seconds {
            return Err(Error::unauthorized("Timestamp out of skew"));
        }

        if body.len() > self.max_body_bytes {
            return Err(Error::unauthorized("Body too large"));
        }

        let body_str =
            std::str::from_utf8(body).map_err(|_| Error::unauthorized("Invalid UTF-8 body"))?;

        let canonical_payload = format!(
            "{}\n{}\n{}\n{}",
            timestamp_str,
            method.as_str().to_uppercase(),
            uri.path(),
            body_str
        );

        tracing::info!("Canonical payload: {:?}", canonical_payload);

        let mut mac = Hmac::<Sha256>::new_from_slice(self.signature_secret.as_bytes())
            .map_err(|e| Error::Server(e.to_string()))?;

        mac.update(canonical_payload.as_bytes());

        let expected_signature =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

        if signature != expected_signature {
            return Err(Error::unauthorized("Invalid signature"));
        }

        Ok(())
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct SignaturePrincipal {
    pub subject: String,
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct SignatureContext {}
