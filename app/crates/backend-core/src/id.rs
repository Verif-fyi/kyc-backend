//! ID generation utilities for the KYC backend.
//!
//! Provides prefixed CUID generation for consistent ID formatting across the system.
//! All IDs use the CUID format with a specific prefix for easy identification.

use cuid;

/// Generates a user ID with the `usr_` prefix.
pub fn user_id() -> Result<String, String> {
    Ok(format!("usr_{}", cuid::cuid1().map_err(|e| e.to_string())?))
}

/// Generates a flow session ID with the `session_` prefix.
pub fn flow_session_id() -> Result<String, String> {
    Ok(format!("session_{}", cuid::cuid1().map_err(|e| e.to_string())?))
}

/// Generates a flow instance ID with the `flow_` prefix.
pub fn flow_instance_id() -> Result<String, String> {
    Ok(format!("flow_{}", cuid::cuid1().map_err(|e| e.to_string())?))
}

/// Generates a flow step ID with the `step_` prefix.
pub fn flow_step_id() -> Result<String, String> {
    Ok(format!("step_{}", cuid::cuid1().map_err(|e| e.to_string())?))
}

/// Generates a KYC upload ID with the `upload_` prefix.
pub fn kyc_upload_id() -> Result<String, String> {
    Ok(format!("upload_{}", cuid::cuid1().map_err(|e| e.to_string())?))
}

/// Generates a KYC evidence ID with the `evidence_` prefix.
pub fn kyc_evidence_id() -> Result<String, String> {
    Ok(format!("evidence_{}", cuid::cuid1().map_err(|e| e.to_string())?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_id_format() {
        let id = user_id().unwrap();
        assert!(id.starts_with("usr_"));
    }

    #[test]
    fn test_flow_session_id_format() {
        let id = flow_session_id().unwrap();
        assert!(id.starts_with("session_"));
    }

    #[test]
    fn test_flow_instance_id_format() {
        let id = flow_instance_id().unwrap();
        assert!(id.starts_with("flow_"));
    }

    #[test]
    fn test_flow_step_id_format() {
        let id = flow_step_id().unwrap();
        assert!(id.starts_with("step_"));
    }

    #[test]
    fn test_kyc_upload_id_format() {
        let id = kyc_upload_id().unwrap();
        assert!(id.starts_with("upload_"));
    }

    #[test]
    fn test_kyc_evidence_id_format() {
        let id = kyc_evidence_id().unwrap();
        assert!(id.starts_with("evidence_"));
    }
}
