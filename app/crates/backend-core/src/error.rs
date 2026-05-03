//! Error handling types and HTTP error construction.
//!
//! This module provides the core error types used throughout the application.
//! Errors are structured to support rich metadata, context, and automatic
//! HTTP response mapping.

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

/// Alias for Result with Error as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;
/// Alias for Result<T, Error> - the standard application result type.
pub type AppResult<T> = Result<T, Error>;

/// Serializable error payload for HTTP API responses.
/// Contains error key, human-readable message, and optional context.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorPayload {
    pub error_key: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
}

/// Static error metadata with compile-time error key and status code.
/// Used for constructing errors with known error codes.
#[derive(Debug, Clone)]
pub struct ErrorMeta {
    pub error_key: &'static str,
    pub status_code: u16,
    pub message: String,
    pub context: Option<Value>,
}

impl ErrorMeta {
    /// Converts static metadata into a serializable payload.
    pub fn payload(&self) -> ErrorPayload {
        ErrorPayload {
            error_key: self.error_key.to_owned(),
            message: self.message.clone(),
            context: self.context.clone(),
        }
    }
}

/// Main error enum representing all possible error conditions in the application.
/// Uses thiserror for automatic Display implementation and From implementations.
#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error")]
    Yaml(#[from] serde_yaml::Error),

    #[error("Not found")]
    NotFound,

    #[error("Any: {0}")]
    Any(#[from] anyhow::Error),

    #[error("Server Error: {0}")]
    Server(String),

    #[error("Address parse error: {0}")]
    AddrParseError(#[from] std::net::AddrParseError),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Diesel error: {0}")]
    Diesel(#[from] diesel::result::Error),

    #[error("Diesel connection error: {0}")]
    DieselConnection(#[from] diesel::result::ConnectionError),

    #[error("Diesel pool error: {0}")]
    DieselPool(String),

    #[error("S3 Error: {0}")]
    S3(String),

    #[error("S3 presign config error: {0}")]
    AwsS3PresignConfig(#[from] aws_sdk_s3::presigning::PresigningConfigError),

    #[error("S3 put object error: {0}")]
    AwsS3PutObject(
        #[from] aws_sdk_s3::error::SdkError<aws_sdk_s3::operation::put_object::PutObjectError>,
    ),

    #[error("SNS publish error: {0}")]
    AwsSnsPublish(
        #[from] aws_sdk_sns::error::SdkError<aws_sdk_sns::operation::publish::PublishError>,
    ),

    #[cfg(feature = "reqwest")]
    #[error("HTTP request error: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("{message}")]
    Http {
        error_key: &'static str,
        status_code: u16,
        message: String,
        context: Option<Value>,
    },
}

impl From<String> for Error {
    fn from(err: String) -> Self {
        Error::Server(err)
    }
}

impl Error {
    /// Creates an unauthorized (401) HTTP error.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::Http {
            error_key: "UNAUTHORIZED",
            status_code: 401,
            message: message.into(),
            context: None,
        }
    }

    /// Creates a bad request (400) HTTP error with a specific error key.
    pub fn bad_request(error_key: &'static str, message: impl Into<String>) -> Self {
        Self::Http {
            error_key,
            status_code: 400,
            message: message.into(),
            context: None,
        }
    }

    /// Creates a not found (404) HTTP error with a specific error key.
    pub fn not_found(error_key: &'static str, message: impl Into<String>) -> Self {
        Self::Http {
            error_key,
            status_code: 404,
            message: message.into(),
            context: None,
        }
    }

    /// Creates a conflict (409) HTTP error with a specific error key.
    pub fn conflict(error_key: &'static str, message: impl Into<String>) -> Self {
        Self::Http {
            error_key,
            status_code: 409,
            message: message.into(),
            context: None,
        }
    }

    /// Creates an internal server error (500) HTTP error with a specific error key.
    pub fn internal(error_key: &'static str, message: impl Into<String>) -> Self {
        Self::Http {
            error_key,
            status_code: 500,
            message: message.into(),
            context: None,
        }
    }

    /// Creates an S3-specific error.
    pub fn s3(message: impl Into<String>) -> Self {
        Self::S3(message.into())
    }

    /// Adds context data to an HTTP error, preserving the original error key and status code.
    pub fn with_context(self, context: Value) -> Self {
        match self {
            Self::Http {
                error_key,
                status_code,
                message,
                ..
            } => Self::Http {
                error_key,
                status_code,
                message,
                context: Some(context),
            },
            other => other,
        }
    }

    /// Extracts error metadata including HTTP status code, error key, and message.
    /// Maps various error types to appropriate HTTP responses.
    pub fn meta(&self) -> ErrorMeta {
        match self {
            Self::NotFound => ErrorMeta {
                error_key: "NOT_FOUND",
                status_code: 404,
                message: self.to_string(),
                context: None,
            },
            Self::Database(t) => ErrorMeta {
                error_key: "DATABASE_ERROR",
                status_code: 500,
                message: format!("Database operation failed: {t}"),
                context: None,
            },
            Self::Diesel(e) => ErrorMeta {
                error_key: "DATABASE_ERROR",
                status_code: 500,
                message: format!("Database operation failed: {e}"),
                context: None,
            },
            Self::DieselConnection(e) => ErrorMeta {
                error_key: "DATABASE_ERROR",
                status_code: 500,
                message: format!("Database connection failed: {e}"),
                context: None,
            },
            Self::DieselPool(e) => ErrorMeta {
                error_key: "DATABASE_ERROR",
                status_code: 500,
                message: format!("Database pool error: {e}"),
                context: None,
            },
            Self::S3(e) => ErrorMeta {
                error_key: "S3_ERROR",
                status_code: 500,
                message: format!("S3 operation failed: {e}"),
                context: None,
            },
            Self::Http {
                error_key,
                status_code,
                message,
                context,
            } => ErrorMeta {
                error_key,
                status_code: *status_code,
                message: message.clone(),
                context: context.clone(),
            },
            _ => ErrorMeta {
                error_key: "INTERNAL_SERVER_ERROR",
                status_code: 500,
                message: "Internal server error".to_owned(),
                context: None,
            },
        }
    }
}

/// Axum integration for converting errors to HTTP responses.
#[cfg(feature = "axum")]
mod axum_impl {
    use super::Error;
    use axum::{
        Json,
        http::StatusCode,
        response::{IntoResponse, Response},
    };

    impl IntoResponse for Error {
        fn into_response(self) -> Response {
            let meta = self.meta();
            let status =
                StatusCode::from_u16(meta.status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, Json(meta.payload())).into_response()
        }
    }
}
