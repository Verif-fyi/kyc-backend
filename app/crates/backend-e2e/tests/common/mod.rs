use anyhow::{Context, Result, anyhow};
use base64::Engine;
use hmac::{Hmac, Mac};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::{Value, json};
use sha2::Sha256;
use std::time::{Duration, Instant};
use tokio_postgres::NoTls;

// Global BFF fixture - simplified for HMAC-only auth
static BFF_FIXTURE: tokio::sync::Mutex<Option<BffTestFixture>> = tokio::sync::Mutex::const_new(None);

#[derive(Clone)]
pub struct BffTestFixture {
    pub user_id: String,
    pub signing_key: Hmac<Sha256>,
}

impl std::fmt::Debug for BffTestFixture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BffTestFixture")
            .field("user_id", &self.user_id)
            .field("signing_key", &"<redacted>")
            .finish()
    }
}

impl BffTestFixture {
    pub fn generate(user_id: &str) -> Self {
        let signing_key = Hmac::new_from_slice(b"some-very-long-secret")
            .expect("HMAC key should be valid");

        Self {
            user_id: user_id.to_owned(),
            signing_key,
        }
    }

    pub fn get() -> Option<Self> {
        BFF_FIXTURE.try_lock().ok().and_then(|guard| guard.clone())
    }

    pub fn sign_bff_request(&self, canonical_payload: &str) -> String {
        let mut mac = self.signing_key.clone();
        mac.update(canonical_payload.as_bytes());
        let signature = mac.finalize().into_bytes();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
    }

    pub fn build_canonical_payload(
        &self,
        timestamp: i64,
        _method: &str,
        _path: &str,
        _body: &str,
        _user_id_hint: Option<&str>,
    ) -> String {
        // Simplified canonical payload for HMAC-only auth
        format!(
            "{}\n{}",
            timestamp,
            self.user_id
        )
    }

    pub fn store_global(self) -> &'static Self {
        if let Ok(mut guard) = BFF_FIXTURE.try_lock() {
            *guard = Some(self.clone());
        }
        Box::leak(Box::new(self))
    }
}

#[derive(Clone, Debug)]
pub struct Env {
    pub user_storage_url: String,
    pub user_storage_blank_base_url: Option<String>,
    pub user_storage_auth_disabled_url: Option<String>,
    pub worker_primary_url: Option<String>,
    pub worker_secondary_url: Option<String>,
    pub keycloak_url: String,
    pub cuss_url: String,
    pub sms_sink_url: String,
    pub database_url: String,
    pub keycloak_client_id: String,
    pub keycloak_client_secret: String,
    pub signature_secret: String,
}

impl Env {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            user_storage_url: must_env("BACKEND_BASE_URL")?,
            user_storage_blank_base_url: maybe_env("BACKEND_BLANK_BASE_URL"),
            user_storage_auth_disabled_url: maybe_env("BACKEND_AUTH_DISABLED_URL"),
            worker_primary_url: maybe_env("WORKER_PRIMARY_URL"),
            worker_secondary_url: maybe_env("WORKER_SECONDARY_URL"),
            keycloak_url: must_env("KEYCLOAK_URL")?,
            cuss_url: must_env("CUSS_URL")?,
            sms_sink_url: must_env("SMS_SINK_URL")?,
            database_url: must_env("DATABASE_URL")?,
            keycloak_client_id: must_env("KEYCLOAK_CLIENT_ID")?,
            keycloak_client_secret: must_env("KEYCLOAK_CLIENT_SECRET")?,
            signature_secret: must_env("SIGNATURE_SECRET")?,
        })
    }
}

fn must_env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("environment variable {key} is required"))
}

fn maybe_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn maybe_env_any(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| maybe_env(key))
}

#[derive(Debug)]
pub struct JsonResponse {
    pub status: u16,
    pub body: Option<Value>,
    pub text: String,
}

pub fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build reqwest client")
}

pub async fn wait_for_status(
    client: &reqwest::Client,
    url: &str,
    expected_status: u16,
    attempts: usize,
) -> Result<()> {
    let mut last_error = String::new();
    for _ in 0..attempts {
        match client.get(url).send().await {
            Ok(response) if response.status().as_u16() == expected_status => return Ok(()),
            Ok(response) => {
                last_error = format!("unexpected status {}", response.status());
            }
            Err(error) => {
                last_error = error.to_string();
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    Err(anyhow!(
        "service at {url} did not return {expected_status}: {last_error}"
    ))
}

pub async fn send_json(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> Result<JsonResponse> {
    let request_path = request_path(url)?;
    let mut request = client
        .request(method.clone(), url)
        .header(CONTENT_TYPE, "application/json");

    if let Some(token) = bearer {
        request = request.header(AUTHORIZATION, format!("Bearer {token}"));
    }

    // Add user ID header for BFF requests
    if request_path.starts_with("/bff") {
        if let Ok(env) = Env::from_env() {
            if let Ok((_, subject)) = get_client_token_and_subject(client, &env).await {
                request = request.header("x-bff-authenticated-user-id", subject);
            }
        }
    }

    let should_sign = should_sign_request(&request_path);
    if should_sign {
        if let Ok(env) = Env::from_env() {
            let timestamp = chrono::Utc::now().timestamp();
            let timestamp_str = timestamp.to_string();
            let payload = body
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .with_context(|| format!("failed to serialize request body for {url}"))?
                .unwrap_or_default();

            // HMAC signing matching server's canonical payload format
            // Server expects: timestamp\nMETHOD\npath\nbody
            let canonical_payload = format!(
                "{}\n{}\n{}\n{}",
                timestamp_str,
                method.as_str().to_uppercase(),
                request_path,
                payload
            );
            let mut mac = Hmac::<Sha256>::new_from_slice(env.signature_secret.as_bytes())
                .map_err(|e| anyhow!("HMAC error: {}", e))?;
            mac.update(canonical_payload.as_bytes());
            let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

            request = request
                .header("x-auth-timestamp", timestamp_str)
                .header("x-auth-signature", signature);
        }
    }

    if let Some(payload) = body.as_ref() {
        request = request.body(serde_json::to_string(payload)?);
    }

    // Add user ID header for BFF requests
    if request_path.starts_with("/bff") {
        if let Some(token) = bearer {
            // Extract user ID from the existing JWT token using the existing helper
            if let Ok(subject) = jwt_subject(token) {
                request = request.header("x-bff-authenticated-user-id", subject);
            } else {
                // Fallback for tests that use intentionally invalid tokens
                request = request.header("x-bff-authenticated-user-id", "usr_invalid");
            }
        }
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("request failed for {url}"))?;

    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .unwrap_or_default();

    let parsed = if text.is_empty() {
        None
    } else {
        serde_json::from_str::<Value>(&text).ok()
    };

    Ok(JsonResponse {
        status,
        body: parsed,
        text,
    })
}

pub async fn send_json_with_bff(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    bearer: Option<&str>,
    body: Option<Value>,
    user_id: Option<&str>,
) -> Result<JsonResponse> {
    let request_path = request_path(url)?;
    let body_json = body
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .with_context(|| format!("failed to serialize request body for {url}"))?;

    let mut request = client
        .request(method.clone(), url)
        .header(CONTENT_TYPE, "application/json");

    if let Some(token) = bearer {
        request = request.header(AUTHORIZATION, format!("Bearer {token}"));
    }

    let should_sign = should_sign_request(&request_path);
    if should_sign {
        if let Ok(env) = Env::from_env() {
            let timestamp = chrono::Utc::now().timestamp();
            let timestamp_str = timestamp.to_string();
            let payload = body_json.as_deref().unwrap_or("");

            // HMAC signing matching server's canonical payload format
            // Server expects: timestamp\nMETHOD\npath\nbody
            let canonical_payload = format!(
                "{}\n{}\n{}\n{}",
                timestamp_str,
                method.as_str().to_uppercase(),
                request_path,
                payload
            );
            let mut mac = Hmac::<Sha256>::new_from_slice(env.signature_secret.as_bytes())
                .map_err(|e| anyhow!("HMAC error: {}", e))?;
            mac.update(canonical_payload.as_bytes());
            let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

            request = request
                .header("x-auth-timestamp", timestamp_str)
                .header("x-auth-signature", signature);
        }
    }

    // Add the user ID header for BFF authentication
    if request_path.starts_with("/bff") {
        if let Some(uid) = user_id {
            // Use explicitly provided user ID
            request = request.header("x-bff-authenticated-user-id", uid);
        } else if let Some(token) = bearer {
            // Extract user ID from JWT token (the 'sub' claim)
            if let Some(subject) = extract_subject_from_token(token) {
                request = request.header("x-bff-authenticated-user-id", subject);
            }
        }
    }

    if let Some(payload) = body_json {
        request = request.body(payload);
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("request failed for {url}"))?;

    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .unwrap_or_default();

    let parsed = if text.is_empty() {
        None
    } else {
        serde_json::from_str::<Value>(&text).ok()
    };

    Ok(JsonResponse {
        status,
        body: parsed,
        text,
    })
}

fn request_path(url: &str) -> Result<String> {
    reqwest::Url::parse(url)
        .map(|parsed| parsed.path().to_owned())
        .map_err(|error| anyhow!("invalid URL `{url}`: {error}"))
}

fn should_sign_request(path: &str) -> bool {
    path.starts_with("/bff") || path.starts_with("/staff")
}

fn extract_subject_from_token(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    
    let payload = parts[1];
    if let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) {
        if let Ok(json) = serde_json::from_slice::<Value>(&decoded) {
            return json.get("sub").and_then(|v| v.as_str()).map(|s| s.to_owned());
        }
    }
    None
}

pub async fn get_client_token_and_subject(
    client: &reqwest::Client,
    env: &Env,
) -> Result<(String, String)> {
    let token_url = format!(
        "{}/realms/e2e-testing/protocol/openid-connect/token",
        env.keycloak_url
    );

    let params = [
        ("grant_type", "client_credentials"),
        ("client_id", env.keycloak_client_id.as_str()),
        ("client_secret", env.keycloak_client_secret.as_str()),
    ];

    let response = client
        .post(&token_url)
        .form(&params)
        .send()
        .await
        .context("keycloak token request failed")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("keycloak token request failed ({status}): {body}"));
    }

    let token_body: Value = response
        .json()
        .await
        .context("invalid keycloak token response JSON")?;
    let access_token = token_body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("keycloak token response missing access_token"))?
        .to_owned();

    let subject = jwt_subject(&access_token)?;
    Ok((access_token, subject))
}

fn jwt_subject(token: &str) -> Result<String> {
    let payload_segment = token
        .split('.')
        .nth(1)
        .ok_or_else(|| anyhow!("invalid jwt token format"))?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_segment)
        .context("failed to decode jwt payload")?;
    let payload_json: Value = serde_json::from_slice(&payload).context("invalid jwt payload")?;
    payload_json
        .get("sub")
        .and_then(Value::as_str)
        .map(normalize_user_id)
        .ok_or_else(|| anyhow!("jwt payload missing sub"))
}

pub async fn ensure_bff_fixtures(database_url: &str, user_id: &str) -> Result<()> {
    let normalized_user_id = normalize_user_id(user_id);

    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .context("failed to connect to postgres")?;

    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("postgres connection task failed: {error}");
        }
    });

    client
        .execute("TRUNCATE flow_step, flow_instance, flow_session, app_user_data CASCADE", &[])
        .await
        .context("failed to truncate flow tables")?;

    let username = format!("subject-{normalized_user_id}");

    client
        .execute(
            r#"
            INSERT INTO app_user (
                user_id,
                username,
                full_name,
                phone_number,
                email_verified,
                disabled,
                attributes,
                created_at,
                updated_at
            ) VALUES (
                $1,
                $2,
                'E2E Subject',
                '+237690123456',
                true,
                false,
                '{}'::jsonb,
                NOW(),
                NOW()
            )
            ON CONFLICT (user_id) DO UPDATE
            SET
                username = EXCLUDED.username,
                full_name = EXCLUDED.full_name,
                phone_number = EXCLUDED.phone_number,
                email_verified = EXCLUDED.email_verified,
                disabled = false,
                attributes = '{}'::jsonb,
                updated_at = NOW()
            "#,
            &[&normalized_user_id, &username],
        )
        .await
        .context("failed to upsert bff user fixture")?;

    client
        .execute(
            r#"
            INSERT INTO app_user (
                user_id,
                username,
                full_name,
                phone_number,
                email_verified,
                disabled,
                attributes,
                created_at,
                updated_at
            ) VALUES (
                'usr_e2e_staff_001',
                'e2e-staff',
                'E2E Staff',
                '+237690000001',
                true,
                false,
                '{}'::jsonb,
                NOW(),
                NOW()
            )
            ON CONFLICT (user_id) DO UPDATE
            SET
                username = EXCLUDED.username,
                full_name = EXCLUDED.full_name,
                phone_number = EXCLUDED.phone_number,
                email_verified = EXCLUDED.email_verified,
                disabled = false,
                attributes = '{}'::jsonb,
                updated_at = NOW()
            "#,
            &[],
        )
        .await
        .context("failed to upsert staff user fixture")?;

    Ok(())
}

fn normalize_user_id(raw: &str) -> String {
    if raw.starts_with("usr_") {
        return raw.to_owned();
    }

    if let Some(segment) = raw.rsplit(':').find(|segment| segment.starts_with("usr_")) {
        return segment.to_owned();
    }

    raw.rsplit(':').next().unwrap_or(raw).to_owned()
}

pub async fn create_foreign_deposit_fixture(
    database_url: &str,
    foreign_user_id: &str,
) -> Result<String> {
    let (client, connection) = tokio_postgres::connect(database_url, NoTls)
        .await
        .context("failed to connect to postgres")?;

    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("postgres connection task failed: {error}");
        }
    });

    let username = format!("subject-{foreign_user_id}");
    client
        .execute(
            r#"
            INSERT INTO app_user (
                user_id,
                username,
                email_verified,
                disabled,
                attributes,
                created_at,
                updated_at
            ) VALUES ($1, $2, true, false, '{}'::jsonb, NOW(), NOW())
            ON CONFLICT (user_id) DO UPDATE
            SET
                username = EXCLUDED.username,
                email_verified = EXCLUDED.email_verified,
                disabled = false,
                attributes = '{}'::jsonb,
                updated_at = NOW()
            "#,
            &[&foreign_user_id, &username],
        )
        .await
        .context("failed to upsert foreign user fixture")?;

    let deposit_id = format!("smi_e2e_foreign_{}", chrono::Utc::now().timestamp_millis());
    let idempotency_key = format!(
        "KYC_FIRST_DEPOSIT:{foreign_user_id}:{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    client
        .execute(
            r#"
            INSERT INTO flow_session (
                id,
                human_id,
                user_id,
                session_type,
                status,
                context,
                created_at,
                updated_at,
                completed_at
            ) VALUES (
                $1,
                $1,
                $2,
                'kyc_full',
                'COMPLETED',
                '{}'::jsonb,
                NOW(),
                NOW(),
                NOW()
            )
            ON CONFLICT (id) DO NOTHING
            "#,
            &[&deposit_id, &foreign_user_id],
        )
        .await
        .context("failed to insert foreign deposit session fixture")?;

    client
        .execute(
            r#"
            INSERT INTO flow_instance (
                id,
                human_id,
                session_id,
                flow_type,
                status,
                step_ids,
                context,
                created_at,
                updated_at
            ) VALUES (
                $1,
                $1,
                $2,
                'first_deposit',
                'COMPLETED',
                '[]'::jsonb,
                '{}'::jsonb,
                NOW(),
                NOW()
            )
            ON CONFLICT (id) DO NOTHING
            "#,
            &[&deposit_id, &deposit_id],
        )
        .await
        .context("failed to insert foreign deposit flow fixture")?;

    Ok(deposit_id)
}

pub async fn reset_sms_sink(client: &reqwest::Client, env: &Env) -> Result<()> {
    let url = format!("{}/__admin/reset", env.sms_sink_url);
    let response = send_json(client, reqwest::Method::POST, &url, None, Some(json!({}))).await?;

    if response.status != 200 {
        return Err(anyhow!(
            "sms sink reset failed ({}): {}",
            response.status,
            response.text
        ));
    }

    Ok(())
}

pub async fn wait_for_otp(
    client: &reqwest::Client,
    env: &Env,
    phone: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let url = format!("{}/__admin/messages", env.sms_sink_url);

    while Instant::now() < deadline {
        let response = send_json(client, reqwest::Method::GET, &url, None, None).await?;
        if response.status == 200 {
            let messages = response.body.unwrap_or_else(|| json!([]));
            if let Some(items) = messages.as_array() {
                for item in items {
                    let item_phone = item
                        .get("phone")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if item_phone == phone {
                        let otp = item
                            .get("otp")
                            .and_then(Value::as_str)
                            .ok_or_else(|| anyhow!("otp field missing in sms sink message"))?;
                        return Ok(otp.to_owned());
                    }
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    Err(anyhow!("otp for {phone} not received within timeout"))
}

pub fn require_json_field<'a>(body: &'a Option<Value>, field: &str) -> Result<&'a Value> {
    body.as_ref()
        .and_then(|json| json.get(field))
        .ok_or_else(|| anyhow!("response body missing field `{field}`"))
}
