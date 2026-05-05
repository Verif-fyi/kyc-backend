use super::{BackendApi, bff_flow::service as bff_service};
use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
use backend_core::Error;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use utoipa::{OpenApi, ToSchema};

#[derive(OpenApi)]
#[openapi(
    paths(presign_upload, complete_upload),
    components(schemas(
        UploadMethod,
        UploadEncryptionMode,
        PresignUploadRequest,
        CompleteUploadRequest,
        PresignUploadResponse,
        EvidenceRef,
    )),
    tags((name = "uploads", description = "Presigned upload endpoints"))
)]
pub struct BffUploadsOpenApi;

pub fn router(api: BackendApi) -> Router {
    Router::new()
        .route("/uploads/presign", post(presign_upload))
        .route("/uploads/complete", post(complete_upload))
        .with_state(api)
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UploadMethod {
    Put,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UploadEncryptionMode {
    SseS3,
    SseKms,
    SseC,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadEncryption {
    pub mode: UploadEncryptionMode,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PresignUploadRequest {
    pub session_id: String,
    pub step_id: String,
    pub user_id: String,
    pub purpose: String,
    pub asset_type: String,
    pub mime: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub encryption: Option<UploadEncryption>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompleteUploadRequestPart {
    pub part_number: u32,
    pub etag: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompleteUploadRequest {
    pub session_id: String,
    pub step_id: String,
    pub upload_id: String,
    pub bucket: String,
    pub object_key: String,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub parts: Option<Vec<CompleteUploadRequestPart>>,
    #[serde(default)]
    pub computed_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PresignUploadResponse {
    pub upload_id: String,
    pub bucket: String,
    pub object_key: String,
    pub method: UploadMethod,
    pub url: String,
    pub headers: std::collections::HashMap<String, String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRef {
    pub evidence_id: String,
    pub session_id: String,
    pub step_id: String,
    pub asset_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[utoipa::path(
    post,
    path = "/uploads/presign",
    request_body = PresignUploadRequest,
    responses((status = 200, body = PresignUploadResponse)),
    tag = "uploads"
)]
async fn presign_upload(
    State(api): State<BackendApi>,
    headers: HeaderMap,
    Json(body): Json<PresignUploadRequest>,
) -> Result<Json<PresignUploadResponse>, Error> {
    let caller_id = bff_service::require_user_id(&api, &headers).await?;
    if caller_id != body.user_id {
        return Err(Error::unauthorized(
            "Cannot create uploads for another user",
        ));
    }
    validate_upload_access(&api, &caller_id, &body.session_id, &body.step_id).await?;

    let encryption = match body.encryption.as_ref().map(|value| &value.mode) {
        Some(UploadEncryptionMode::SseKms) => crate::object_storage::EncryptionMode::Kms,
        Some(UploadEncryptionMode::SseC) => {
            return Err(Error::bad_request(
                "UPLOAD_ENCRYPTION_UNSUPPORTED",
                "SSE_C uploads are not supported",
            ));
        }
        _ => crate::object_storage::EncryptionMode::S3,
    };

    let upload_id = backend_core::id::kyc_upload_id()?;
    let extension = extension_for_mime(&body.mime);
    let _size_bytes = body.size_bytes;
    let object_key = format!(
        "{}/{}/{}/{}/{}.{}",
        body.session_id,
        body.step_id,
        upload_id,
        sanitize_path_segment(&body.purpose),
        sanitize_path_segment(&body.asset_type),
        extension
    );

    let (bucket, ttl) = upload_bucket_and_ttl(&api);
    let presigned = api
        .state
        .object_storage
        .upload_presigned(&bucket, &object_key, &body.mime, encryption, ttl)
        .await?;

    Ok(Json(PresignUploadResponse {
        upload_id,
        bucket,
        object_key,
        method: UploadMethod::Put,
        url: presigned.url,
        headers: presigned.headers,
        expires_at: Utc::now() + Duration::from_std(ttl).unwrap_or_else(|_| Duration::hours(1)),
    }))
}

#[utoipa::path(
    post,
    path = "/uploads/complete",
    request_body = CompleteUploadRequest,
    responses(
        (status = 200, body = EvidenceRef),
        (status = 404, description = "Upload object not found")
    ),
    tag = "uploads"
)]
async fn complete_upload(
    State(api): State<BackendApi>,
    headers: HeaderMap,
    Json(body): Json<CompleteUploadRequest>,
) -> Result<Json<EvidenceRef>, Error> {
    let caller_id = bff_service::require_user_id(&api, &headers).await?;
    validate_upload_access(&api, &caller_id, &body.session_id, &body.step_id).await?;
    if !body.object_key.contains(&body.upload_id) {
        return Err(Error::bad_request(
            "UPLOAD_KEY_MISMATCH",
            "Object key does not match the upload id",
        ));
    }
    validate_complete_upload_parts(body.etag.as_deref(), body.parts.as_deref())?;

    api.state
        .object_storage
        .head_object(&body.bucket, &body.object_key)
        .await
        .map_err(|_| Error::not_found("UPLOAD_NOT_FOUND", "Uploaded object not found"))?;

    Ok(Json(EvidenceRef {
        evidence_id: backend_core::id::kyc_evidence_id()?,
        session_id: body.session_id,
        step_id: body.step_id,
        asset_type: asset_type_from_key(&body.object_key),
        sha256: body.computed_sha256,
        created_at: Utc::now(),
    }))
}

async fn validate_upload_access(
    api: &BackendApi,
    caller_id: &str,
    session_id: &str,
    step_id: &str,
) -> Result<(), Error> {
    let session = api
        .state
        .flow
        .get_session(session_id)
        .await?
        .ok_or_else(|| Error::not_found("SESSION_NOT_FOUND", "Session not found"))?;

    if session.user_id.as_deref() != Some(caller_id) {
        return Err(Error::unauthorized("Cannot access other users' uploads"));
    }

    let step = api
        .state
        .flow
        .get_step(step_id)
        .await?
        .ok_or_else(|| Error::not_found("STEP_NOT_FOUND", "Step not found"))?;
    let flow = api
        .state
        .flow
        .get_flow(&step.flow_id)
        .await?
        .ok_or_else(|| Error::not_found("FLOW_NOT_FOUND", "Flow not found"))?;

    if flow.session_id != session_id {
        return Err(Error::bad_request(
            "STEP_SESSION_MISMATCH",
            "Step does not belong to the provided session",
        ));
    }

    Ok(())
}

fn upload_bucket_and_ttl(api: &BackendApi) -> (String, std::time::Duration) {
    if let Some(storage) = api.state.config.storage.as_ref()
        && let Some(minio) = storage.minio.as_ref()
    {
        return (
            minio.bucket.clone(),
            std::time::Duration::from_secs(minio.presign_ttl_seconds),
        );
    }

    if let Some(s3) = api.state.config.s3.as_ref() {
        return (
            s3.bucket.clone(),
            std::time::Duration::from_secs(s3.presign_ttl_seconds),
        );
    }

    (
        "user-storage-dev".to_owned(),
        std::time::Duration::from_secs(3600),
    )
}

fn sanitize_path_segment(value: &str) -> String {
    value.replace(
        |ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_',
        "_",
    )
}

fn extension_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "application/pdf" => "pdf",
        _ => "jpg",
    }
}

fn asset_type_from_key(object_key: &str) -> String {
    object_key
        .rsplit('/')
        .next()
        .and_then(|name| name.split('.').next())
        .unwrap_or("UNKNOWN")
        .to_owned()
}

fn validate_complete_upload_parts(
    etag: Option<&str>,
    parts: Option<&[CompleteUploadRequestPart]>,
) -> Result<(), Error> {
    if let Some(value) = etag
        && value.trim().is_empty()
    {
        return Err(Error::bad_request(
            "UPLOAD_ETAG_INVALID",
            "ETag cannot be empty",
        ));
    }

    let Some(parts) = parts else {
        return Ok(());
    };

    let mut seen = HashSet::with_capacity(parts.len());
    for part in parts {
        if part.part_number == 0 {
            return Err(Error::bad_request(
                "UPLOAD_PART_NUMBER_INVALID",
                "Part numbers must be greater than zero",
            ));
        }

        if part.etag.trim().is_empty() {
            return Err(Error::bad_request(
                "UPLOAD_PART_ETAG_INVALID",
                "Part ETag cannot be empty",
            ));
        }

        if !seen.insert(part.part_number) {
            return Err(Error::bad_request(
                "UPLOAD_PART_DUPLICATE",
                "Duplicate part numbers are not allowed",
            ));
        }
    }

    Ok(())
}
