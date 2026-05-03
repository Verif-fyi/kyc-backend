use std::iter;

use utoipa::OpenApi;
use utoipa::openapi::Server;
use utoipa::openapi::security::SecurityRequirement;
use utoipa::openapi::security::{
    ApiKey, ApiKeyValue, SecurityScheme,
};
use utoipa_swagger_ui::SwaggerUi;

use crate::api::{
    bff_flow::BffFlowOpenApi, bff_uploads::BffUploadsOpenApi,
    staff_flow::StaffFlowOpenApi,
};
use backend_core::config::Config;

/// Main API documentation
#[derive(OpenApi)]
#[openapi(
    info(
        title = "KYC Tokenization Backend API",
        version = "0.2.4",
        description = "KYC orchestration backend with signature auth, flows, and webhook integration"
    ),
    tags(
        (name = "users", description = "User profile endpoints"),
        (name = "sessions", description = "Session management endpoints"),
        (name = "flows", description = "Flow execution endpoints"),
        (name = "steps", description = "Step submission endpoints"),
    )
)]
pub struct ApiDoc;

/// Creates a SwaggerUi with HMAC signature and server URLs configured from the app config
pub fn swagger_ui(config: &Config) -> SwaggerUi {
    // Create the specs
    let mut bff_spec = BffFlowOpenApi::openapi();
    let mut uploads_spec = BffUploadsOpenApi::openapi();
    let mut staff_spec = StaffFlowOpenApi::openapi();

    // Create the Signature security scheme
    let signature_scheme = SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("x-auth-signature")));

    // Build host URL from swagger config or fall back to server config
    let http_host = if let Some(http_host) = &config.swagger.http_host {
        http_host.clone()
    } else {
        format!("http://{}:{}", config.server.address, config.server.port)
    };

    // Build base URLs from BFF and Staff config
    let bff_base = config.bff.base_path.trim();
    let staff_base = config.staff.base_path.trim();

    // Helper to create security requirements
    let make_signature_req = || SecurityRequirement::new("AuthSignature", iter::empty::<String>());

    // Add security scheme and requirement to BFF spec
    if let Some(components) = bff_spec.components.as_mut() {
        components
            .security_schemes
            .insert("AuthSignature".to_string(), signature_scheme.clone());
    }
    bff_spec.security = Some(vec![make_signature_req()]);
    // Add server URL with /bff prefix so paths resolve correctly
    bff_spec.servers = Some(vec![Server::new(&format!("{}{}", http_host, bff_base))]);

    // Add security scheme and requirement to uploads spec
    if let Some(components) = uploads_spec.components.as_mut() {
        components
            .security_schemes
            .insert("AuthSignature".to_string(), signature_scheme.clone());
    }
    uploads_spec.security = Some(vec![make_signature_req()]);
    uploads_spec.servers = Some(vec![Server::new(&http_host)]);

    // Add security scheme and requirement to staff spec
    if let Some(components) = staff_spec.components.as_mut() {
        components
            .security_schemes
            .insert("AuthSignature".to_string(), signature_scheme.clone());
    }
    staff_spec.security = Some(vec![make_signature_req()]);
    staff_spec.servers = Some(vec![Server::new(&format!("{}{}", http_host, staff_base))]);

    let json = include_str!("../../../gen/kc_openapi/openapi.json")
        .parse::<serde_json::Value>()
        .expect("KC OpenAPI JSON is invalid");

    SwaggerUi::new("/swagger-ui/")
        .url("/api-docs/bff/openapi.json", bff_spec)
        .url("/api-docs/uploads/openapi.json", uploads_spec)
        .url("/api-docs/staff/openapi.json", staff_spec)
        .url("/api-docs/core/openapi.json", ApiDoc::openapi())
        .external_url_unchecked("/api-docs/kc/openapi.json", json)
}
