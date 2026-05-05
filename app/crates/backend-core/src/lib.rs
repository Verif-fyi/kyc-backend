//! Core library for the tokenization backend system.
#![allow(clippy::result_large_err)]
//!
//! This crate provides foundational types and abstractions used across all other crates
//! in the workspace, including:
//! - Configuration management with environment variable support
//! - Error handling with rich metadata and HTTP status mapping
//! - Data transfer objects (DTOs) for account and project management
//! - CLI types for command-line parsing
//! - Telemetry initialization for structured logging
//! - Re-exports of commonly used dependencies

pub mod config;
mod dto;
mod error;
pub mod id;
pub mod notifications;

#[cfg(feature = "cli")]
mod cli;

#[cfg(feature = "telemetry")]
mod telemetry;

// Re-export DTOs for account and project management
pub use crate::dto::{
    Account, CreateAccount, CreateProject, Project, UpdateAccount, UpdateProject,
};

// Re-export error handling types
pub use crate::error::{AppResult, Error, ErrorMeta, ErrorPayload, Result};
pub use backend_env::envsubst;

// Re-export configuration types
pub use config::*;

// Re-export notification types
pub use notifications::NotificationJob;

// Re-export CLI types when feature is enabled
#[cfg(feature = "cli")]
pub use crate::cli::{Cli, Commands};

// Re-export telemetry types when feature is enabled
#[cfg(feature = "telemetry")]
pub use crate::telemetry::init_tracing;

// Re-export commonly used external crates
pub use anyhow;
pub use async_trait::async_trait;
pub use cuid;
