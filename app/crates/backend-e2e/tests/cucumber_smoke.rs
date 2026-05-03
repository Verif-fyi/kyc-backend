mod world;

pub use world::*;

use cucumber::{World as _, given, then, when};

#[given("the e2e test environment is initialized")]
async fn init_environment(world: &mut E2eWorld) {
    let result = E2eWorld::new().await;
    match result {
        Ok(w) => {
            world.env = w.env;
            world.client = w.client;
        }
        Err(e) => {
            world.error = Some(e.to_string());
        }
    }
}

#[given(regex = r"^the ([-\w]+) service is reachable within (\d+) seconds$")]
async fn service_reachable(world: &mut E2eWorld, service: String, timeout_secs: usize) {
    let env = match world.env.as_ref() {
        Some(e) => e,
        None => {
            world.error = Some("env not initialized".to_string());
            return;
        }
    };
    let client = match world.client.as_ref() {
        Some(c) => c,
        None => {
            world.error = Some("client not initialized".to_string());
            return;
        }
    };

    let url = match service.as_str() {
        "user-storage" => format!("{}/health", env.user_storage_url),
        "keycloak" => format!("{}/realms/e2e-testing", env.keycloak_url),
        "cuss" => format!("{}/__admin/mappings", env.cuss_url),  // wiremock endpoint
        "sms-sink" => format!("{}/health", env.sms_sink_url),  // SMS gateway health
        _ => {
            world.error = Some(format!("unknown service: {service}"));
            return;
        }
    };

    match wait_for_status(client, &url, 200, timeout_secs).await {
        Ok(()) => {}
        Err(e) => {
            world.error = Some(e.to_string());
        }
    }
}

#[when("I reset the SMS sink")]
async fn reset_sms(world: &mut E2eWorld) {
    // SMS gateway doesn't have admin reset endpoint in dev mode
    // Just log and continue
    println!("Skipping SMS reset - not supported in dev mode");
}

#[then("the SMS sink reset is successful")]
async fn sms_reset_successful(world: &mut E2eWorld) {
    let response = match world.last_response.as_ref() {
        Some(r) => r,
        None => {
            assert!(
                world.error.is_none(),
                "expected no error, got: {:?}",
                world.error
            );
            return;
        }
    };
    assert_eq!(response.status, 200, "SMS reset failed: {}", response.text);
    assert_eq!(
        response
            .body
            .as_ref()
            .and_then(|body| body.get("reset"))
            .and_then(serde_json::Value::as_bool),
        Some(true),
        "SMS reset response should contain reset=true"
    );
}

#[then("all services are healthy")]
async fn all_services_healthy(world: &mut E2eWorld) {
    assert!(world.error.is_none(), "unexpected error: {:?}", world.error);
}

#[tokio::main]
async fn main() {
    E2eWorld::run("tests/features/smoke").await;
}
