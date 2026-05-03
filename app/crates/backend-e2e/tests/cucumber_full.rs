mod world;

pub use world::*;

use anyhow::{Result, anyhow};
use cucumber::{World, given, then, when};
use tokio_postgres::NoTls;
use reqwest::Method;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const FIXTURE_FULL_NAME: &str = "E2E Subject";
const FIXTURE_PHONE_NUMBER: &str = "+237690123456";

#[derive(Debug, Default)]
pub struct FlowState {
    pub session_id: Option<String>,
    pub phone_flow_id: Option<String>,
    pub phone_verify_step_id: Option<String>,
    pub deposit_flow_id: Option<String>,
    pub admin_step_id: Option<String>,
    pub id_document_flow_id: Option<String>,
    pub id_document_step_id: Option<String>,
    pub deposit_amount: Option<i64>,
    pub phone_number: Option<String>,
    pub otp_code: Option<String>,
}

#[derive(Debug, Default, World)]
#[world(init = Self::new)]
pub struct FullE2eWorld {
    pub env: Option<Env>,
    pub client: Option<reqwest::Client>,
    pub token: Option<String>,
    pub subject: Option<String>,
    pub last_response: Option<JsonResponse>,
    pub error: Option<String>,
    pub flow: FlowState,
}

impl FullE2eWorld {
    pub async fn new() -> Result<Self, anyhow::Error> {
        Ok(Self {
            env: Some(Env::from_env()?),
            client: Some(http_client()?),
            ..Default::default()
        })
    }

    fn env(&self) -> Result<&Env> {
        self.env
            .as_ref()
            .ok_or_else(|| anyhow!("env not initialized"))
    }

    fn client(&self) -> Result<&reqwest::Client> {
        self.client
            .as_ref()
            .ok_or_else(|| anyhow!("client not initialized"))
    }

    fn subject(&self) -> Result<&str> {
        self.subject
            .as_deref()
            .ok_or_else(|| anyhow!("subject not initialized"))
    }

    fn token(&self) -> Result<&str> {
        self.token
            .as_deref()
            .ok_or_else(|| anyhow!("token not initialized"))
    }

    fn bff_base(&self) -> Result<String> {
        Ok(format!("{}/bff", self.env()?.user_storage_url))
    }

    fn staff_base(&self) -> Result<String> {
        Ok(format!("{}/staff", self.env()?.user_storage_url))
    }
}

fn require_response_field<'a>(body: &'a Option<Value>, field: &str) -> Result<&'a Value> {
    body.as_ref()
        .and_then(|json| json.get(field))
        .ok_or_else(|| anyhow!("response body missing field `{field}`"))
}

fn require_id(body: &Option<Value>, field: &str) -> Result<String> {
    require_response_field(body, field)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("{field} must be a string"))
}

async fn reset_cuss(client: &reqwest::Client, env: &Env) -> Result<()> {
    let response = send_json(
        client,
        Method::POST,
        &format!("{}/__admin/reset", env.cuss_url),
        None,
        Some(json!({})),
    )
    .await?;

    if response.status != 200 {
        return Err(anyhow!(
            "cuss reset failed ({}): {}",
            response.status,
            response.text
        ));
    }

    Ok(())
}

async fn set_cuss_fault(
    client: &reqwest::Client,
    env: &Env,
    endpoint: &str,
    status: u16,
    count: u32,
) -> Result<()> {
    let response = send_json(
        client,
        Method::POST,
        &format!("{}/__admin/faults", env.cuss_url),
        None,
        Some(json!({
            "endpoint": endpoint,
            "status": status,
            "count": count,
            "body": {
                "error": format!("forced_{endpoint}_failure")
            }
        })),
    )
    .await?;

    if response.status != 200 {
        return Err(anyhow!(
            "cuss fault update failed ({}): {}",
            response.status,
            response.text
        ));
    }

    Ok(())
}

async fn list_flow_steps(world: &FullE2eWorld, flow_id: &str) -> Result<Vec<Value>> {
    let response = send_json(
        world.client()?,
        Method::GET,
        &format!("{}/flows/{}", world.bff_base()?, flow_id),
        Some(world.token()?),
        None,
    )
    .await?;

    if response.status != 200 {
        return Err(anyhow!(
            "get flow failed ({}): {}",
            response.status,
            response.text
        ));
    }

    response
        .body
        .as_ref()
        .and_then(|body| body.get("steps"))
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| anyhow!("steps missing from flow response"))
}

fn find_step_id(steps: &[Value], step_type: &str) -> Result<String> {
    steps
        .iter()
        .find(|step| step.get("stepType").and_then(Value::as_str) == Some(step_type))
        .and_then(|step| step.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("step `{step_type}` not found"))
}

async fn wait_for_flow_status(
    world: &FullE2eWorld,
    flow_id: &str,
    expected_status: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let url = format!("{}/flows/{}", world.bff_base()?, flow_id);
    let mut last_status: Option<String> = None;

    while Instant::now() < deadline {
        let response = send_json(
            world.client()?,
            Method::GET,
            &url,
            Some(world.token()?),
            None,
        )
        .await?;

        if response.status == 200 {
            let status = response.body.as_ref().and_then(|body| {
                body.get("status")
                    .or_else(|| body.get("flow").and_then(|flow| flow.get("status")))
            });

            if let Some(status) = status.and_then(Value::as_str) {
                last_status = Some(status.to_owned());
                if status == expected_status {
                    return Ok(());
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    Err(anyhow!(
        "flow {flow_id} did not reach status {expected_status} within {:?} (last_status={})",
        timeout,
        last_status.unwrap_or_else(|| "unknown".to_owned())
    ))
}

async fn wait_for_session_close_reason(
    world: &FullE2eWorld,
    session_id: &str,
    expected_status: &str,
    expected_reason: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let url = format!("{}/sessions/{}", world.bff_base()?, session_id);

    while Instant::now() < deadline {
        let response = send_json(
            world.client()?,
            Method::GET,
            &url,
            Some(world.token()?),
            None,
        )
        .await?;

        if response.status == 200
            && let Some(session) = response.body.as_ref().and_then(|body| body.get("session"))
        {
            let status = session.get("status").and_then(Value::as_str);
            let reason = session
                .pointer("/context/close_reason")
                .and_then(Value::as_str);
            if status == Some(expected_status) && reason == Some(expected_reason) {
                return Ok(());
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    Err(anyhow!(
        "session {session_id} did not reach {expected_status}/{expected_reason}"
    ))
}

async fn find_waiting_admin_step(world: &FullE2eWorld, user_id: &str) -> Result<String> {
    let response = send_json(
        world.client()?,
        Method::GET,
        &format!(
            "{}/flow/steps?status=WAITING&userId={}&flowType=first_deposit",
            world.staff_base()?,
            user_id
        ),
        Some(world.token()?),
        None,
    )
    .await?;

    if response.status != 200 {
        return Err(anyhow!(
            "list admin steps failed ({}): {}",
            response.status,
            response.text
        ));
    }

    let steps = response
        .body
        .as_ref()
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    find_step_id(&steps, "await_admin_decision")
}

async fn cuss_requests(world: &FullE2eWorld) -> Result<Vec<Value>> {
    let response = send_json(
        world.client()?,
        Method::GET,
        &format!("{}/__admin/requests", world.env()?.cuss_url),
        None,
        None,
    )
    .await?;

    if response.status != 200 {
        return Err(anyhow!(
            "cuss requests failed ({}): {}",
            response.status,
            response.text
        ));
    }

    Ok(response
        .body
        .as_ref()
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

async fn get_staff_flow_detail(world: &FullE2eWorld, flow_id: &str) -> Result<Value> {
    let response = send_json(
        world.client()?,
        Method::GET,
        &format!("{}/flow/flows/{}", world.staff_base()?, flow_id),
        Some(world.token()?),
        None,
    )
    .await?;

    if response.status != 200 {
        return Err(anyhow!(
            "get staff flow failed ({}): {}",
            response.status,
            response.text
        ));
    }

    response
        .body
        .ok_or_else(|| anyhow!("staff flow response body missing"))
}

async fn get_staff_step_detail(world: &FullE2eWorld, step_id: &str) -> Result<Value> {
    let response = send_json(
        world.client()?,
        Method::GET,
        &format!("{}/flow/steps/{}", world.staff_base()?, step_id),
        Some(world.token()?),
        None,
    )
    .await?;

    if response.status != 200 {
        return Err(anyhow!(
            "get staff step failed ({}): {}",
            response.status,
            response.text
        ));
    }

    response
        .body
        .ok_or_else(|| anyhow!("staff step response body missing"))
}

fn find_step<'a>(steps: &'a [Value], step_type: &str) -> Result<&'a Value> {
    steps
        .iter()
        .find(|step| step.get("stepType").and_then(Value::as_str) == Some(step_type))
        .ok_or_else(|| anyhow!("step `{step_type}` not found"))
}

#[given("the e2e test environment is initialized")]
async fn init_environment(world: &mut FullE2eWorld) {
    match FullE2eWorld::new().await {
        Ok(next) => {
            world.env = next.env;
            world.client = next.client;
        }
        Err(error) => world.error = Some(error.to_string()),
    }
}

#[given("I have a valid authentication token")]
async fn get_auth_token(world: &mut FullE2eWorld) {
    match get_client_token_and_subject(world.client().unwrap(), world.env().unwrap()).await {
        Ok((token, subject)) => {
            world.token = Some(token);
            world.subject = Some(subject);
        }
        Err(error) => world.error = Some(error.to_string()),
    }
}

#[given("the database fixtures are set up")]
async fn setup_fixtures(world: &mut FullE2eWorld) {
    let subject = match world.subject() {
        Ok(value) => value.to_owned(),
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };

    if let Err(error) = ensure_bff_fixtures(&world.env().unwrap().database_url, &subject).await {
        world.error = Some(error.to_string());
    }
}

#[given("the SMS sink is reset")]
async fn given_reset_sms(world: &mut FullE2eWorld) {
    if let Err(error) = reset_sms_sink(world.client().unwrap(), world.env().unwrap()).await {
        world.error = Some(error.to_string());
    }
}

#[given("the CUSS sink is reset")]
async fn given_reset_cuss(world: &mut FullE2eWorld) {
    if let Err(error) = reset_cuss(world.client().unwrap(), world.env().unwrap()).await {
        world.error = Some(error.to_string());
    }
}

#[given("the CUSS register endpoint fails with 500 for 4 attempts")]
async fn given_cuss_register_fault(world: &mut FullE2eWorld) {
    if let Err(error) = set_cuss_fault(
        world.client().unwrap(),
        world.env().unwrap(),
        "register",
        500,
        4,
    )
    .await
    {
        world.error = Some(error.to_string());
    }
}

#[given("the CUSS approve endpoint fails with 500 for 3 attempts")]
async fn given_cuss_approve_fault(world: &mut FullE2eWorld) {
    if let Err(error) = set_cuss_fault(
        world.client().unwrap(),
        world.env().unwrap(),
        "approve",
        500,
        3,
    )
    .await
    {
        world.error = Some(error.to_string());
    }
}

#[when(regex = r"^I send a (\w+) request to ([^\s]+) without authentication$")]
async fn send_request_no_auth(world: &mut FullE2eWorld, method: String, path: String) {
    let url = format!("{}{}", world.env().unwrap().user_storage_url, path);
    let result = match method.to_uppercase().as_str() {
        "POST" => {
            world
                .client()
                .unwrap()
                .post(url)
                .json(&json!({}))
                .send()
                .await
        }
        _ => world.client().unwrap().get(url).send().await,
    };

    match result {
        Ok(response) => {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            world.last_response = Some(JsonResponse {
                status,
                body: serde_json::from_str(&text).ok(),
                text,
            });
        }
        Err(error) => world.error = Some(error.to_string()),
    }
}

#[when(regex = r"^I send a (\w+) request to ([^\s]+) with an invalid Bearer token$")]
async fn send_request_invalid_bearer(world: &mut FullE2eWorld, method: String, path: String) {
    let url = format!("{}{}", world.env().unwrap().user_storage_url, path);
    let result = match method.to_uppercase().as_str() {
        "POST" => {
            world
                .client()
                .unwrap()
                .post(url)
                .header("Authorization", "Bearer definitely-invalid-token")
                .json(&json!({}))
                .send()
                .await
        }
        _ => {
            world
                .client()
                .unwrap()
                .get(url)
                .header("Authorization", "Bearer definitely-invalid-token")
                .send()
                .await
        }
    };

    match result {
        Ok(response) => {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            world.last_response = Some(JsonResponse {
                status,
                body: serde_json::from_str(&text).ok(),
                text,
            });
        }
        Err(error) => world.error = Some(error.to_string()),
    }
}

#[when(regex = r"^I send a (\w+) request to ([^\s]+) with Basic auth$")]
async fn send_request_basic_auth(world: &mut FullE2eWorld, method: String, path: String) {
    let url = format!("{}{}", world.env().unwrap().user_storage_url, path);
    let result = match method.to_uppercase().as_str() {
        "POST" => {
            world
                .client()
                .unwrap()
                .post(url)
                .header("Authorization", "Basic dGVzdDp0ZXN0")
                .json(&json!({}))
                .send()
                .await
        }
        _ => {
            world
                .client()
                .unwrap()
                .get(url)
                .header("Authorization", "Basic dGVzdDp0ZXN0")
                .send()
                .await
        }
    };

    match result {
        Ok(response) => {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            world.last_response = Some(JsonResponse {
                status,
                body: serde_json::from_str(&text).ok(),
                text,
            });
        }
        Err(error) => world.error = Some(error.to_string()),
    }
}

#[when(regex = r"^I send a (\w+) request to ([^\s]+) with valid authentication$")]
async fn send_request_valid_auth(world: &mut FullE2eWorld, method: String, path: String) {
    let url = format!("{}{}", world.env().unwrap().user_storage_url, path);
    let result = send_json(
        world.client().unwrap(),
        match method.to_uppercase().as_str() {
            "POST" => Method::POST,
            _ => Method::GET,
        },
        &url,
        Some(world.token().unwrap()),
        Some(json!({ "sessionType": "kyc_full" })),
    )
    .await;

    match result {
        Ok(response) => world.last_response = Some(response),
        Err(error) => world.error = Some(error.to_string()),
    }
}

#[given("I complete phone OTP verification")]
async fn complete_phone_otp(world: &mut FullE2eWorld) {
    let subject = world.subject().unwrap().to_owned();
    let bff_base = world.bff_base().unwrap();
    let phone_number = "+237690000033".to_owned();

    let session = send_json(
        world.client().unwrap(),
        Method::POST,
        &format!("{}/sessions", bff_base),
        Some(world.token().unwrap()),
        Some(json!({ "sessionType": "kyc_full" })),
    )
    .await;

    let session = match session {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };

    if session.status != 201 {
        world.error = Some(format!(
            "create phone_otp session failed ({}): {}",
            session.status, session.text
        ));
        return;
    }

    let session_id = match require_id(&session.body, "id") {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(format!(
                "create phone_otp session missing id: {} | body={}",
                error, session.text
            ));
            return;
        }
    };

    let flow = send_json(
        world.client().unwrap(),
        Method::POST,
        &format!("{}/sessions/{}/flows", bff_base, session_id),
        Some(world.token().unwrap()),
        Some(json!({ "flowType": "phone_otp" })),
    )
    .await;

    let flow = match flow {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };

    if flow.status != 201 {
        world.error = Some(format!(
            "create phone_otp flow failed ({}): {}",
            flow.status, flow.text
        ));
        return;
    }

    let flow_id = match require_id(&flow.body, "id") {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(format!(
                "create phone_otp flow missing id: {} | body={}",
                error, flow.text
            ));
            return;
        }
    };

    let steps = match list_flow_steps(world, &flow_id).await {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };
    let init_step_id = match find_step_id(&steps, "init_phone") {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };

    if let Err(error) = send_json(
        world.client().unwrap(),
        Method::POST,
        &format!("{}/steps/{}", bff_base, init_step_id),
        Some(world.token().unwrap()),
        Some(json!({ "input": { "phone_number": phone_number } })),
    )
    .await
    {
        world.error = Some(error.to_string());
        return;
    }

    let otp = match wait_for_otp(
        world.client().unwrap(),
        world.env().unwrap(),
        "+237690000033",
        Duration::from_secs(15),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };

    let verify_steps = match list_flow_steps(world, &flow_id).await {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };
    let verify_step_id = match find_step_id(&verify_steps, "verify_otp") {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };

    match send_json(
        world.client().unwrap(),
        Method::POST,
        &format!("{}/steps/{}", bff_base, verify_step_id),
        Some(world.token().unwrap()),
        Some(json!({ "input": { "code": otp } })),
    )
    .await
    {
        Ok(response) => world.last_response = Some(response),
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    }

    if let Err(error) =
        wait_for_flow_status(world, &flow_id, "COMPLETED", Duration::from_secs(10)).await
    {
        world.error = Some(error.to_string());
        return;
    }

    world.flow.session_id = Some(session_id);
    world.flow.phone_flow_id = Some(flow_id);
    world.flow.phone_verify_step_id = Some(verify_step_id);
    world.flow.phone_number = Some(phone_number);
    world.flow.otp_code = Some(otp);
    let _ = subject;
}

#[given("I start a first deposit flow for 5000 XAF")]
async fn start_first_deposit_flow(world: &mut FullE2eWorld) {
    start_first_deposit(world, 5000).await;
}

#[given("I start a first deposit flow for 7000 XAF")]
async fn start_first_deposit_flow_reject(world: &mut FullE2eWorld) {
    start_first_deposit(world, 7000).await;
}

async fn start_first_deposit(world: &mut FullE2eWorld, amount: i64) {
    let bff_base = world.bff_base().unwrap();
    let session_id = if let Some(existing) = world.flow.session_id.clone() {
        existing
    } else {
        match send_json(
            world.client().unwrap(),
            Method::POST,
            &format!("{}/sessions", bff_base),
            Some(world.token().unwrap()),
            Some(json!({ "sessionType": "kyc_full" })),
        )
        .await
        {
            Ok(response) => {
                if response.status != 201 {
                    world.error = Some(format!(
                        "create session failed ({}): {}",
                        response.status, response.text
                    ));
                    return;
                }
                match require_id(&response.body, "id") {
                    Ok(id) => id,
                    Err(error) => {
                        world.error = Some(format!(
                            "create session missing id: {} | body={}",
                            error, response.text
                        ));
                        return;
                    }
                }
            }
            Err(error) => {
                world.error = Some(error.to_string());
                return;
            }
        }
    };

    let flow = match send_json(
        world.client().unwrap(),
        Method::POST,
        &format!("{}/sessions/{}/flows", bff_base, session_id),
        Some(world.token().unwrap()),
        Some(json!({ "flowType": "first_deposit" })),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };
    if flow.status != 201 {
        world.error = Some(format!(
            "create first_deposit flow failed ({}): {}",
            flow.status, flow.text
        ));
        return;
    }

    let flow_id = match require_id(&flow.body, "id") {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(format!(
                "create first_deposit flow missing id: {} | body={}",
                error, flow.text
            ));
            return;
        }
    };

    let steps = match list_flow_steps(world, &flow_id).await {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };
    let init_step_id = match find_step_id(&steps, "init_first_deposit") {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };

    match send_json(
        world.client().unwrap(),
        Method::POST,
        &format!("{}/steps/{}", bff_base, init_step_id),
        Some(world.token().unwrap()),
        Some(json!({ "input": { "amount": amount, "currency": "XAF" } })),
    )
    .await
    {
        Ok(response) => world.last_response = Some(response),
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    }

    match find_waiting_admin_step(world, world.subject().unwrap()).await {
        Ok(step_id) => world.flow.admin_step_id = Some(step_id),
        Err(error) => world.error = Some(error.to_string()),
    }

    world.flow.session_id = Some(session_id);
    world.flow.deposit_flow_id = Some(flow_id);
    world.flow.deposit_amount = Some(amount);
}

#[when("I approve the pending first deposit admin step")]
async fn approve_first_deposit(world: &mut FullE2eWorld) {
    let admin_step_id = match world.flow.admin_step_id.clone() {
        Some(value) => value,
        None => {
            world.error = Some("admin step id missing".to_owned());
            return;
        }
    };

    match send_json(
        world.client().unwrap(),
        Method::POST,
        &format!(
            "{}/flow/steps/{}",
            world.staff_base().unwrap(),
            admin_step_id
        ),
        Some(world.token().unwrap()),
        Some(json!({ "input": { "decision": "APPROVED", "validated_deposited_amount": 500.0 } })),
    )
    .await
    {
        Ok(response) => world.last_response = Some(response),
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    }

    if let Some(flow_id) = world.flow.deposit_flow_id.clone()
        && let Err(error) =
            wait_for_flow_status(world, &flow_id, "COMPLETED", Duration::from_secs(15)).await
    {
        world.error = Some(error.to_string());
    }
}

#[when("I approve the pending first deposit admin step expecting flow failure")]
async fn approve_first_deposit_expect_failure(world: &mut FullE2eWorld) {
    let admin_step_id = match world.flow.admin_step_id.clone() {
        Some(value) => value,
        None => {
            world.error = Some("admin step id missing".to_owned());
            return;
        }
    };

    match send_json(
        world.client().unwrap(),
        Method::POST,
        &format!(
            "{}/flow/steps/{}",
            world.staff_base().unwrap(),
            admin_step_id
        ),
        Some(world.token().unwrap()),
        Some(json!({ "input": { "decision": "APPROVED", "validated_deposited_amount": 500.0 } })),
    )
    .await
    {
        Ok(response) => world.last_response = Some(response),
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    }

    if let Some(flow_id) = world.flow.deposit_flow_id.clone()
        && let Err(error) =
            wait_for_flow_status(world, &flow_id, "FAILED", Duration::from_secs(20)).await
    {
        world.error = Some(error.to_string());
    }
}

#[when("I reject the pending first deposit admin step")]
async fn reject_first_deposit(world: &mut FullE2eWorld) {
    let admin_step_id = match world.flow.admin_step_id.clone() {
        Some(value) => value,
        None => {
            world.error = Some("admin step id missing".to_owned());
            return;
        }
    };

    match send_json(
        world.client().unwrap(),
        Method::POST,
        &format!(
            "{}/flow/steps/{}",
            world.staff_base().unwrap(),
            admin_step_id
        ),
        Some(world.token().unwrap()),
        Some(json!({ "input": { "decision": "REJECTED" } })),
    )
    .await
    {
        Ok(response) => world.last_response = Some(response),
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    }

    if let Some(flow_id) = world.flow.deposit_flow_id.clone()
        && let Err(error) =
            wait_for_flow_status(world, &flow_id, "CLOSED", Duration::from_secs(10)).await
    {
        world.error = Some(error.to_string());
    }
}

#[given("the OAuth2 token endpoint is configured")]
async fn given_oauth2_token_endpoint(world: &mut FullE2eWorld) {
    let client = world.client().unwrap();
    let env = world.env().unwrap();

    let stub = json!({
        "request": {
            "method": "POST",
            "url": "/oauth2/token"
        },
        "response": {
            "status": 200,
            "headers": {
                "Content-Type": "application/json"
            },
            "jsonBody": {
                "access_token": "oauth2-test-access-token",
                "token_type": "Bearer",
                "expires_in": 3600
            }
        }
    });

    let result = send_json(
        client,
        Method::POST,
        &format!("{}/__admin/mappings", env.cuss_url),
        None,
        Some(stub),
    )
    .await;

    if let Err(error) = result {
        world.error = Some(format!(
            "Failed to configure OAuth2 token endpoint: {}",
            error
        ));
    }
}

#[when("I create a session and start a webhook auth test flow with valid Bearer token")]
async fn start_webhook_auth_test_flow_valid(world: &mut FullE2eWorld) {
    start_webhook_auth_test_flow(world, "webhook_auth_test").await;
}

#[when("I create a session and start a webhook auth test flow with invalid Bearer token")]
async fn start_webhook_auth_test_flow_invalid(world: &mut FullE2eWorld) {
    start_webhook_auth_test_flow(world, "webhook_auth_test_invalid").await;
}

#[when("I create a session and start a webhook basic auth test flow")]
async fn start_webhook_basic_auth_test_flow(world: &mut FullE2eWorld) {
    start_webhook_auth_test_flow(world, "webhook_auth_test_basic").await;
}

#[when("I create a session and start a webhook OAuth2 auth test flow")]
async fn start_webhook_oauth2_auth_test_flow(world: &mut FullE2eWorld) {
    start_webhook_auth_test_flow(world, "webhook_auth_test_oauth2").await;
}

async fn start_webhook_auth_test_flow(world: &mut FullE2eWorld, flow_type: &str) {
    let bff_base = world.bff_base().unwrap();

    let session = match send_json(
        world.client().unwrap(),
        Method::POST,
        &format!("{}/sessions", bff_base),
        Some(world.token().unwrap()),
        Some(json!({ "sessionType": "webhook_test" })),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };

    if session.status != 201 {
        world.error = Some(format!(
            "create session failed ({}): {}",
            session.status, session.text
        ));
        return;
    }

    let session_id = match require_id(&session.body, "id") {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(format!(
                "create session missing id: {} | body={}",
                error, session.text
            ));
            return;
        }
    };

    let flow = match send_json(
        world.client().unwrap(),
        Method::POST,
        &format!("{}/sessions/{}/flows", bff_base, session_id),
        Some(world.token().unwrap()),
        Some(json!({ "flowType": flow_type })),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };

    if flow.status != 201 {
        world.error = Some(format!(
            "create {} flow failed ({}): {}",
            flow_type, flow.status, flow.text
        ));
        return;
    }

    let flow_id = match require_id(&flow.body, "id") {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(format!(
                "create {} flow missing id: {} | body={}",
                flow_type, error, flow.text
            ));
            return;
        }
    };

    world.flow.session_id = Some(session_id);
    world.flow.deposit_flow_id = Some(flow_id);
}

#[then("the webhook auth test flow completes successfully")]
async fn webhook_auth_flow_completes(world: &mut FullE2eWorld) {
    let flow_id = match world.flow.deposit_flow_id.clone() {
        Some(value) => value,
        None => {
            world.error = Some("flow id missing".to_owned());
            return;
        }
    };

    if let Err(error) =
        wait_for_flow_status(world, &flow_id, "COMPLETED", Duration::from_secs(10)).await
    {
        world.error = Some(error.to_string());
    }
}

#[then("the webhook auth test flow fails with authentication error")]
async fn webhook_auth_flow_fails(world: &mut FullE2eWorld) {
    let flow_id = match world.flow.deposit_flow_id.clone() {
        Some(value) => value,
        None => {
            world.error = Some("flow id missing".to_owned());
            return;
        }
    };

    if let Err(error) =
        wait_for_flow_status(world, &flow_id, "FAILED", Duration::from_secs(10)).await
    {
        world.error = Some(error.to_string());
        return;
    }

    let flow_detail = match get_staff_flow_detail(world, &flow_id).await {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };

    let steps = flow_detail
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if let Ok(webhook_step) = find_step(&steps, "webhook_step") {
        let status = webhook_step.get("status").and_then(Value::as_str);
        assert_eq!(status, Some("FAILED"), "webhook_step should be FAILED");
    } else {
        world.error = Some("webhook_step not found".to_owned());
    }
}

#[then("the webhook basic auth test flow completes successfully")]
async fn webhook_basic_auth_flow_completes(world: &mut FullE2eWorld) {
    webhook_auth_flow_completes(world).await;
}

#[then("the webhook OAuth2 auth test flow completes successfully")]
async fn webhook_oauth2_auth_flow_completes(world: &mut FullE2eWorld) {
    webhook_auth_flow_completes(world).await;
}

#[when("I get the current user")]
async fn get_current_user(world: &mut FullE2eWorld) {
    match send_json(
        world.client().unwrap(),
        Method::GET,
        &format!(
            "{}/users/{}",
            world.bff_base().unwrap(),
            world.subject().unwrap()
        ),
        Some(world.token().unwrap()),
        None,
    )
    .await
    {
        Ok(response) => world.last_response = Some(response),
        Err(error) => world.error = Some(error.to_string()),
    }
}

#[when("I get completed KYC")]
async fn get_completed_kyc(world: &mut FullE2eWorld) {
    match send_json(
        world.client().unwrap(),
        Method::GET,
        &format!(
            "{}/users/{}/completed-kyc",
            world.bff_base().unwrap(),
            world.subject().unwrap()
        ),
        Some(world.token().unwrap()),
        None,
    )
    .await
    {
        Ok(response) => world.last_response = Some(response),
        Err(error) => world.error = Some(error.to_string()),
    }
}

#[then(regex = r"^the response status is (\d+)$")]
async fn response_status_is(world: &mut FullE2eWorld, expected: u16) {
    let response = world.last_response.as_ref().expect("No response recorded");
    assert_eq!(response.status, expected, "{}", response.text);
}

#[then(regex = r"^the response status is not (\d+)$")]
async fn response_status_is_not(world: &mut FullE2eWorld, unexpected: u16) {
    let response = world.last_response.as_ref().expect("No response recorded");
    assert_ne!(response.status, unexpected, "{}", response.text);
}

#[then("the response contains the correct user ID")]
async fn response_contains_correct_user_id(world: &mut FullE2eWorld) {
    let response = world.last_response.as_ref().expect("No response recorded");
    let subject = world.subject().expect("subject should be set");
    let user_id = response
        .body
        .as_ref()
        .and_then(|body| body.get("userId"))
        .and_then(Value::as_str);
    assert_eq!(user_id, Some(subject));
}

fn require_completed_kyc(body: &Option<Value>) -> Value {
    body.as_ref()
        .and_then(|value| value.get("completedKyc"))
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

fn completed_kyc_contains_flow(body: &Option<Value>, flow_type: &str) -> bool {
    let completed_kyc = require_completed_kyc(body);
    completed_kyc
        .as_object()
        .into_iter()
        .flatten()
        .any(|(_, flows)| {
            flows
                .as_object()
                .and_then(|entries| entries.get(flow_type))
                .and_then(Value::as_object)
                .and_then(|details| details.get("completed"))
                .and_then(Value::as_bool)
                == Some(true)
        })
}

#[then("completed KYC is empty")]
async fn completed_kyc_is_empty(world: &mut FullE2eWorld) {
    let completed_kyc = require_completed_kyc(&world.last_response.as_ref().unwrap().body);
    assert_eq!(
        completed_kyc
            .as_object()
            .map(|obj| obj.len())
            .unwrap_or_default(),
        0
    );
}

#[then(regex = "^completed KYC contains flow \"([^\"]+)\"$")]
async fn completed_kyc_contains(world: &mut FullE2eWorld, flow_type: String) {
    let has_flow =
        completed_kyc_contains_flow(&world.last_response.as_ref().unwrap().body, &flow_type);
    assert!(has_flow, "completedKyc does not contain flow `{flow_type}`");
}

#[then(regex = "^completed KYC does not contain flow \"([^\"]+)\"$")]
async fn completed_kyc_not_contains(world: &mut FullE2eWorld, flow_type: String) {
    let has_flow =
        completed_kyc_contains_flow(&world.last_response.as_ref().unwrap().body, &flow_type);
    assert!(
        !has_flow,
        "completedKyc unexpectedly contains flow `{flow_type}`"
    );
}

#[then("the first deposit metadata is persisted")]
async fn first_deposit_metadata_persisted(world: &mut FullE2eWorld) {
    let response = send_json(
        world.client().unwrap(),
        Method::GET,
        &format!(
            "{}/users/{}",
            world.bff_base().unwrap(),
            world.subject().unwrap()
        ),
        Some(world.token().unwrap()),
        None,
    )
    .await
    .expect("user request should succeed");

    let metadata = response
        .body
        .as_ref()
        .and_then(|body| body.get("metadata"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    assert_eq!(metadata.pointer("/fineractId"), Some(&json!(1)));
    assert_eq!(metadata.pointer("/savingsAccountId"), Some(&json!(2)));
    assert_eq!(
        metadata.pointer("/firstDeposit/status"),
        Some(&json!("APPROVED"))
    );
    assert_eq!(
        metadata.pointer("/firstDeposit/transactionId"),
        Some(&json!(20))
    );
}

#[then("the first deposit metadata is not persisted")]
async fn first_deposit_metadata_not_persisted(world: &mut FullE2eWorld) {
    let response = send_json(
        world.client().unwrap(),
        Method::GET,
        &format!(
            "{}/users/{}",
            world.bff_base().unwrap(),
            world.subject().unwrap()
        ),
        Some(world.token().unwrap()),
        None,
    )
    .await
    .expect("user request should succeed");

    let metadata = response
        .body
        .as_ref()
        .and_then(|body| body.get("metadata"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    assert_eq!(metadata.pointer("/fineractId"), None);
    assert_eq!(metadata.pointer("/savingsAccountId"), None);
    assert_eq!(metadata.pointer("/firstDeposit/status"), None);
    assert_eq!(metadata.pointer("/firstDeposit/transactionId"), None);
}

#[then("the first deposit flow is waiting for admin review")]
async fn first_deposit_waiting_for_admin(world: &mut FullE2eWorld) {
    let flow_id = world
        .flow
        .deposit_flow_id
        .clone()
        .expect("deposit flow id should be set");
    let admin_step_id = world
        .flow
        .admin_step_id
        .clone()
        .expect("admin step id should be set");

    let staff_step = get_staff_step_detail(world, &admin_step_id)
        .await
        .expect("staff step should load");
    assert_eq!(
        staff_step.get("stepType").and_then(Value::as_str),
        Some("await_admin_decision")
    );
    assert_eq!(
        staff_step.get("actor").and_then(Value::as_str),
        Some("ADMIN")
    );
    assert_eq!(
        staff_step.get("status").and_then(Value::as_str),
        Some("WAITING")
    );

    let flow_detail = get_staff_flow_detail(world, &flow_id)
        .await
        .expect("staff flow should load");
    let steps = flow_detail
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    assert_eq!(
        find_step(&steps, "init_first_deposit")
            .ok()
            .and_then(|step| step.get("status"))
            .and_then(Value::as_str),
        Some("COMPLETED")
    );
    assert_eq!(
        find_step(&steps, "get_user")
            .ok()
            .and_then(|step| step.get("status"))
            .and_then(Value::as_str),
        Some("COMPLETED")
    );
    assert_eq!(
        find_step(&steps, "await_admin_decision")
            .ok()
            .and_then(|step| step.get("status"))
            .and_then(Value::as_str),
        Some("WAITING")
    );
}

#[then(regex = r"^the first deposit step ([a-z_]+) is failed and retryable$")]
async fn first_deposit_step_failed_retryable(world: &mut FullE2eWorld, step_type: String) {
    let flow_id = world
        .flow
        .deposit_flow_id
        .clone()
        .expect("deposit flow id should be set");
    let flow_detail = get_staff_flow_detail(world, &flow_id)
        .await
        .expect("staff flow should load");
    let steps = flow_detail
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let step = find_step(&steps, &step_type).expect("step should exist");
    assert_eq!(step.get("status").and_then(Value::as_str), Some("FAILED"));
    assert_eq!(step.pointer("/error/retryable"), Some(&json!(true)));
}

#[then("the staff flow detail shows the completed deposit path")]
async fn completed_deposit_path_visible(world: &mut FullE2eWorld) {
    let flow_id = world
        .flow
        .deposit_flow_id
        .clone()
        .expect("deposit flow id should be set");
    let flow_detail = get_staff_flow_detail(world, &flow_id)
        .await
        .expect("staff flow should load");
    let steps = flow_detail
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for step_type in [
        "await_admin_decision",
        "decide_admin",
        "cuss_register_customer",
        "cuss_approve_and_deposit",
        "update_deposit_metadata",
    ] {
        assert_eq!(
            find_step(&steps, step_type)
                .ok()
                .and_then(|step| step.get("status"))
                .and_then(Value::as_str),
            Some("COMPLETED"),
            "step `{step_type}` should be completed"
        );
    }
    assert_ne!(
        find_step(&steps, "close_session")
            .ok()
            .and_then(|step| step.get("status"))
            .and_then(Value::as_str),
        Some("COMPLETED"),
        "close_session should not be completed on approve"
    );
}

#[then("the staff flow detail shows the rejected deposit path")]
async fn rejected_deposit_path_visible(world: &mut FullE2eWorld) {
    let flow_id = world
        .flow
        .deposit_flow_id
        .clone()
        .expect("deposit flow id should be set");
    let flow_detail = get_staff_flow_detail(world, &flow_id)
        .await
        .expect("staff flow should load");
    let steps = flow_detail
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for step_type in ["await_admin_decision", "decide_admin", "close_session"] {
        assert_eq!(
            find_step(&steps, step_type)
                .ok()
                .and_then(|step| step.get("status"))
                .and_then(Value::as_str),
            Some("COMPLETED"),
            "step `{step_type}` should be completed"
        );
    }

    for step_type in [
        "cuss_register_customer",
        "cuss_approve_and_deposit",
        "update_deposit_metadata",
    ] {
        assert_ne!(
            find_step(&steps, step_type)
                .ok()
                .and_then(|step| step.get("status"))
                .and_then(Value::as_str),
            Some("COMPLETED"),
            "step `{step_type}` should not be completed on reject"
        );
    }
}

#[then("the CUSS payloads match the first deposit flow")]
async fn cuss_payloads_match_first_deposit(world: &mut FullE2eWorld) {
    let requests = cuss_requests(world)
        .await
        .expect("cuss requests should load");
    let subject = world.subject().expect("subject should be set").to_owned();
    let deposit_amount = world
        .flow
        .deposit_amount
        .expect("deposit amount should be set");

    let register_request = requests
        .iter()
        .find(|item| item.get("endpoint").and_then(Value::as_str) == Some("register"))
        .expect("register request missing");
    let approve_request = requests
        .iter()
        .find(|item| item.get("endpoint").and_then(Value::as_str) == Some("approve"))
        .expect("approve request missing");

    assert_eq!(
        register_request.pointer("/payload/externalId"),
        Some(&json!(subject))
    );
    assert_eq!(
        register_request.pointer("/payload/fullName"),
        Some(&json!(FIXTURE_FULL_NAME))
    );
    let expected_phone = world
        .flow
        .phone_number
        .clone()
        .unwrap_or_else(|| FIXTURE_PHONE_NUMBER.to_owned());
    assert_eq!(
        register_request.pointer("/payload/phone"),
        Some(&json!(expected_phone))
    );
    assert_eq!(
        approve_request.pointer("/payload/depositAmount"),
        Some(&json!(500))
    );
    assert_eq!(
        approve_request.pointer("/payload/savingsAccountId"),
        Some(&json!(2))
    );

    let register_auth_header = register_request
        .pointer("/payload/authHeader")
        .and_then(Value::as_str)
        .expect("missing authHeader in register payload");
    assert!(
        register_auth_header.starts_with("Bearer "),
        "expected authHeader to start with 'Bearer ', got '{register_auth_header}'"
    );

    let approve_auth_header = approve_request
        .pointer("/payload/authHeader")
        .and_then(Value::as_str)
        .expect("missing authHeader in approve payload");
    assert!(
        approve_auth_header.starts_with("Bearer "),
        "expected authHeader to start with 'Bearer ', got '{approve_auth_header}'"
    );
}

#[then("the reject path closes the session with reason REJECTED_BY_ADMIN")]
async fn reject_path_closes_session(world: &mut FullE2eWorld) {
    let session_id = world
        .flow
        .session_id
        .clone()
        .expect("session id should be set");
    wait_for_session_close_reason(
        world,
        &session_id,
        "CLOSED",
        "REJECTED_BY_ADMIN",
        Duration::from_secs(10),
    )
    .await
    .expect("session should be closed with reject reason");
}

#[then("no CUSS request was recorded")]
async fn no_cuss_request_recorded(world: &mut FullE2eWorld) {
    let requests = cuss_requests(world)
        .await
        .expect("cuss requests should load");
    assert!(
        requests.is_empty(),
        "unexpected CUSS requests: {requests:?}"
    );
}

#[then("CUSS register and approve requests were recorded")]
async fn cuss_requests_recorded(world: &mut FullE2eWorld) {
    let requests = cuss_requests(world)
        .await
        .expect("cuss requests should load");
    assert_eq!(requests.len(), 2, "unexpected CUSS requests: {requests:?}");
    assert!(
        requests
            .iter()
            .any(|item| { item.get("endpoint").and_then(Value::as_str) == Some("register") })
    );
    assert!(
        requests
            .iter()
            .any(|item| { item.get("endpoint").and_then(Value::as_str) == Some("approve") })
    );
}

#[then("no error occurred")]
async fn no_error(world: &mut FullE2eWorld) {
    assert!(world.error.is_none(), "Unexpected error: {:?}", world.error);
}

#[given("a registered device with JWT token")]
async fn registered_device_with_jwt(world: &mut FullE2eWorld) {
    if world.token.is_none() {
        world.error = Some("JWT token not available".to_string());
    }
}

#[given("I have created an id_document session")]
async fn create_id_document_session(world: &mut FullE2eWorld) {
    let bff_base = match world.bff_base() {
        Ok(base) => base,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let token = match world.token() {
        Ok(t) => t,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let client = match world.client() {
        Ok(c) => c,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let session = send_json(
        client,
        Method::POST,
        &format!("{}/sessions", bff_base),
        Some(token),
        Some(json!({ "sessionType": "kyc_full" })),
    )
    .await;

    let session = match session {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };

    if session.status != 201 {
        world.error = Some(format!(
            "create session failed ({}): {}",
            session.status, session.text
        ));
        return;
    }

    let session_id = match require_id(&session.body, "id") {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(format!(
                "session missing id: {} | body={}",
                error, session.text
            ));
            return;
        }
    };

    let flow = send_json(
        client,
        Method::POST,
        &format!("{}/sessions/{}/flows", bff_base, session_id),
        Some(token),
        Some(json!({ "flowType": "id_document" })),
    )
    .await;

    world.flow.session_id = Some(session_id);

    let flow = match flow {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(error.to_string());
            return;
        }
    };

    if flow.status != 201 {
        world.error = Some(format!(
            "create id_document flow failed ({}): {}",
            flow.status, flow.text
        ));
        return;
    }

    let flow_id = match require_id(&flow.body, "id") {
        Ok(value) => value,
        Err(error) => {
            world.error = Some(format!(
                "flow missing id: {} | body={}",
                error, flow.text
            ));
            return;
        }
    };

    world.flow.id_document_flow_id = Some(flow_id);
}

#[when(regex = r#"^I upload valid id_document documents for session "([^"]+)"$"#)]
async fn upload_id_document(world: &mut FullE2eWorld, session_id: String, raw_docstring: String) {
    // session_id is passed from the step, do not overwrite
    let bff_base = match world.bff_base() {
        Ok(base) => base,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let token = match world.token() {
        Ok(t) => t,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let client = match world.client() {
        Ok(c) => c,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let upload_payload: Value = match serde_json::from_str(raw_docstring.as_str()) {
        Ok(v) => v,
        Err(e) => {
            world.error = Some(format!("failed to parse upload payload: {}", e));
            return;
        }
    };

    let upload_response = send_json(
        client,
        Method::POST,
        &format!("{}/sessions/{}/flows/{}/steps", bff_base, session_id, world.flow.id_document_flow_id.as_ref().expect("flow_id not set")),
        Some(token),
        Some(json!({
            "action": "uploadDocument",
            "input": upload_payload
        })),
    )
    .await;

    world.last_response = Some(upload_response.unwrap_or(JsonResponse {
        status: 0,
        text: "".to_string(),
        body: None,
    }));
}

#[given("an id_document session AWAITING_REVIEW")]
async fn id_document_awaiting_review(world: &mut FullE2eWorld) {
    create_id_document_session(world).await;
    
    let session_id = match &world.flow.session_id {
        Some(id) => id.clone(),
        None => return,
    };

    let flow_id = match &world.flow.id_document_flow_id {
        Some(id) => id.clone(),
        None => return,
    };

    let bff_base = match world.bff_base() {
        Ok(base) => base,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let client = match world.client() {
        Ok(c) => c,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let steps_response = send_json(
        client,
        Method::GET,
        &format!("{}/sessions/{}/flows/{}/steps", bff_base, session_id, flow_id),
        world.token().ok().as_deref(),
        None,
    )
    .await;

    if let Ok(steps) = steps_response {
        if let Some(body) = &steps.body {
            if let Some(steps_arr) = body.as_array() {
                for step in steps_arr {
                    if let Some(step_id) = step.get("id").and_then(|v| v.as_str()) {
                        if let Some(flags) = step.get("flags").and_then(|v| v.as_array()) {
                            for flag in flags {
                                if let Some(flag_str) = flag.as_str() {
                                    if flag_str == "AWAITING_REVIEW" {
                                        world.flow.id_document_step_id = Some(step_id.to_string());
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[when("I approve the id_document session via staff API")]
async fn approve_id_document_session(world: &mut FullE2eWorld) {
    let step_id = match &world.flow.id_document_step_id {
        Some(id) => id.clone(),
        None => {
            world.error = Some("step_id not set".to_string());
            return;
        }
    };

    let staff_base = match world.staff_base() {
        Ok(base) => base,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let client = match world.client() {
        Ok(c) => c,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let token = match world.token() {
        Ok(t) => t,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let approve_response = send_json(
        client,
        Method::POST,
        &format!("{}/flow/steps/{}/actions", staff_base, step_id),
        Some(token),
        Some(json!({
            "action": "approve",
            "payload": {}
        })),
    )
    .await;

    world.last_response = Some(approve_response.unwrap_or(JsonResponse {
        status: 0,
        text: "".to_string(),
        body: None,
    }));
}

#[when(regex = r#"^I reject the id_document session via staff API with reason "([^"]+)"$"#)]
async fn reject_id_document_session(world: &mut FullE2eWorld, reason: String) {
    let step_id = match &world.flow.id_document_step_id {
        Some(id) => id.clone(),
        None => {
            world.error = Some("step_id not set".to_string());
            return;
        }
    };

    let staff_base = match world.staff_base() {
        Ok(base) => base,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let client = match world.client() {
        Ok(c) => c,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let token = match world.token() {
        Ok(t) => t,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let reject_response = send_json(
        client,
        Method::POST,
        &format!("{}/flow/steps/{}/actions", staff_base, step_id),
        Some(token),
        Some(json!({
            "action": "reject",
            "payload": { "reason": reason }
        })),
    )
    .await;

    world.last_response = Some(reject_response.unwrap_or(JsonResponse {
        status: 0,
        text: "".to_string(),
        body: None,
    }));
}

#[given("an approved id_document session")]
async fn approved_id_document_session(world: &mut FullE2eWorld) {
    create_id_document_session(world).await;
    
    let step_id = match &world.flow.id_document_step_id {
        Some(id) => id.clone(),
        None => return,
    };

    let staff_base = match world.staff_base() {
        Ok(base) => base,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let client = match world.client() {
        Ok(c) => c,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    let token = match world.token() {
        Ok(t) => t,
        Err(e) => {
            world.error = Some(e.to_string());
            return;
        }
    };

    send_json(
        client,
        Method::POST,
        &format!("{}/flow/steps/{}/actions", staff_base, step_id),
        Some(token),
        Some(json!({
            "action": "approve",
            "payload": {}
        })),
    )
    .await
    .ok();
}

#[then("the response contains session ID")]
async fn response_contains_session_id(world: &mut FullE2eWorld) {
    if let Some(response) = &world.last_response {
        if let Some(body) = &response.body {
            if body.get("id").is_none() {
                world.error = Some(format!("response missing session ID: {}", response.text));
            } else {
                world.flow.session_id = body.get("id").and_then(|v| v.as_str()).map(String::from);
            }
        }
    }
}

#[then("sm_instance row persisted for KYC_ID_DOCUMENT with import_id = 1")]
async fn sm_instance_persisted_kyc_id_document(world: &mut FullE2eWorld) {
    let session_id = match &world.flow.session_id {
        Some(id) => id.clone(),
        None => {
            world.error = Some("session_id not set".to_string());
            return;
        }
    };

    let env = match world.env() {
        Ok(e) => e,
        Err(err) => {
            world.error = Some(err.to_string());
            return;
        }
    };

    let conn = match tokio_postgres::connect(&env.database_url, NoTls).await {
        Ok((conn, _)) => conn,
        Err(e) => {
            world.error = Some(format!("failed to connect to database: {}", e));
            return;
        }
    };

    let row = conn
        .query_one(
            "SELECT id, flow_type, import_id FROM sm_instance WHERE session_id = $1 AND flow_type = 'KYC_ID_DOCUMENT' LIMIT 1",
            &[&session_id],
        )
        .await;

    match row {
        Ok(record) => {
            let import_id: i32 = record.get::<usize, i32>(2);
            if import_id != 1 {
                world.error = Some(format!("expected import_id = 1, got {}", import_id));
            }
        }
        Err(e) => {
            world.error = Some(format!("sm_instance row not found: {}", e));
        }
    }
}

#[then("sm_event entries exist for DocumentUploaded event")]
async fn sm_event_document_uploaded(world: &mut FullE2eWorld) {
    let session_id = match &world.flow.session_id {
        Some(id) => id.clone(),
        None => {
            world.error = Some("session_id not set".to_string());
            return;
        }
    };

    let env = match world.env() {
        Ok(e) => e,
        Err(err) => {
            world.error = Some(err.to_string());
            return;
        }
    };

    let conn = match tokio_postgres::connect(&env.database_url, NoTls).await {
        Ok((conn, _)) => conn,
        Err(e) => {
            world.error = Some(format!("failed to connect to database: {}", e));
            return;
        }
    };

    let count: i64 = conn
        .query_one(
            "SELECT COUNT(*) FROM sm_event WHERE session_id = $1 AND event_type = 'DocumentUploaded'",
            &[&session_id],
        )
        .await
        .map(|row| row.get(0))
        .unwrap_or(0);

    if count == 0 {
        world.error = Some("no DocumentUploaded event found".to_string());
    }
}

#[then("session still exists in sm_instance")]
async fn session_still_exists_in_sm_instance(world: &mut FullE2eWorld) {
    let session_id = match &world.flow.session_id {
        Some(id) => id.clone(),
        None => {
            world.error = Some("session_id not set".to_string());
            return;
        }
    };

    let env = match world.env() {
        Ok(e) => e,
        Err(err) => {
            world.error = Some(err.to_string());
            return;
        }
    };

    let conn = match tokio_postgres::connect(&env.database_url, NoTls).await {
        Ok((conn, _)) => conn,
        Err(e) => {
            world.error = Some(format!("failed to connect to database: {}", e));
            return;
        }
    };

    let exists: bool = conn
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM sm_instance WHERE session_id = $1)",
            &[&session_id],
        )
        .await
        .map(|row| row.get(0))
        .unwrap_or(false);

    if !exists {
        world.error = Some("session does not exist in sm_instance".to_string());
    }
}

#[then("sm_event entries exist for DocumentApproved event")]
async fn sm_event_document_approved(world: &mut FullE2eWorld) {
    let session_id = match &world.flow.session_id {
        Some(id) => id.clone(),
        None => {
            world.error = Some("session_id not set".to_string());
            return;
        }
    };

    let env = match world.env() {
        Ok(e) => e,
        Err(err) => {
            world.error = Some(err.to_string());
            return;
        }
    };

    let conn = match tokio_postgres::connect(&env.database_url, NoTls).await {
        Ok((conn, _)) => conn,
        Err(e) => {
            world.error = Some(format!("failed to connect to database: {}", e));
            return;
        }
    };

    let count: i64 = conn
        .query_one(
            "SELECT COUNT(*) FROM sm_event WHERE session_id = $1 AND event_type = 'DocumentApproved'",
            &[&session_id],
        )
        .await
        .map(|row| row.get(0))
        .unwrap_or(0);

    if count == 0 {
        world.error = Some("no DocumentApproved event found".to_string());
    }
}

#[then("session state transitions to DocumentApproved")]
async fn session_state_document_approved(world: &mut FullE2eWorld) {
    let session_id = match &world.flow.session_id {
        Some(id) => id.clone(),
        None => {
            world.error = Some("session_id not set".to_string());
            return;
        }
    };

    let env = match world.env() {
        Ok(e) => e,
        Err(err) => {
            world.error = Some(err.to_string());
            return;
        }
    };

    let conn = match tokio_postgres::connect(&env.database_url, NoTls).await {
        Ok((conn, _)) => conn,
        Err(e) => {
            world.error = Some(format!("failed to connect to database: {}", e));
            return;
        }
    };

    let state: Result<String, _> = conn
        .query_one(
            "SELECT state FROM sm_instance WHERE session_id = $1 LIMIT 1",
            &[&session_id],
        )
        .await
        .map(|row| row.get(0));

    match state {
        Ok(s) => {
            if !s.contains("DocumentApproved") {
                world.error = Some(format!("expected state to contain DocumentApproved, got {}", s));
            }
        }
        Err(e) => {
            world.error = Some(format!("failed to get session state: {}", e));
        }
    }
}

#[then("sm_event entries exist for DocumentRejected event")]
async fn sm_event_document_rejected(world: &mut FullE2eWorld) {
    let session_id = match &world.flow.session_id {
        Some(id) => id.clone(),
        None => {
            world.error = Some("session_id not set".to_string());
            return;
        }
    };

    let env = match world.env() {
        Ok(e) => e,
        Err(err) => {
            world.error = Some(err.to_string());
            return;
        }
    };

    let conn = match tokio_postgres::connect(&env.database_url, NoTls).await {
        Ok((conn, _)) => conn,
        Err(e) => {
            world.error = Some(format!("failed to connect to database: {}", e));
            return;
        }
    };

    let count: i64 = conn
        .query_one(
            "SELECT COUNT(*) FROM sm_event WHERE session_id = $1 AND event_type = 'DocumentRejected'",
            &[&session_id],
        )
        .await
        .map(|row| row.get(0))
        .unwrap_or(0);

    if count == 0 {
        world.error = Some("no DocumentRejected event found".to_string());
    }
}

#[then("session state transitions to DocumentRejected")]
async fn session_state_document_rejected(world: &mut FullE2eWorld) {
    let session_id = match &world.flow.session_id {
        Some(id) => id.clone(),
        None => {
            world.error = Some("session_id not set".to_string());
            return;
        }
    };

    let env = match world.env() {
        Ok(e) => e,
        Err(err) => {
            world.error = Some(err.to_string());
            return;
        }
    };

    let conn = match tokio_postgres::connect(&env.database_url, NoTls).await {
        Ok((conn, _)) => conn,
        Err(e) => {
            world.error = Some(format!("failed to connect to database: {}", e));
            return;
        }
    };

    let state: Result<String, _> = conn
        .query_one(
            "SELECT state FROM sm_instance WHERE session_id = $1 LIMIT 1",
            &[&session_id],
        )
        .await
        .map(|row| row.get(0));

    match state {
        Ok(s) => {
            if !s.contains("DocumentRejected") {
                world.error = Some(format!("expected state to contain DocumentRejected, got {}", s));
            }
        }
        Err(e) => {
            world.error = Some(format!("failed to get session state: {}", e));
        }
    }
}

#[then("the user profile contains id_document verification status")]
async fn user_profile_contains_id_document_status(world: &mut FullE2eWorld) {
    if let Some(response) = &world.last_response {
        if let Some(body) = &response.body {
            if let Some(kyc) = body.get("kyc") {
                if kyc.get("id_document").is_none() {
                    world.error = Some("user profile missing id_document verification status".to_string());
                }
            }
        }
    }
}

#[then("completed KYC contains flow \"id_document\"")]
async fn completed_kyc_contains_id_document(world: &mut FullE2eWorld) {
    if let Some(response) = &world.last_response {
        if let Some(body) = &response.body {
            if let Some(completed) = body.get("completed") {
                if let Some(arr) = completed.as_array() {
                    if !arr.iter().any(|v| v.as_str() == Some("id_document")) {
                        world.error = Some("completed KYC does not contain id_document".to_string());
                    }
                }
            }
        }
    }
}

#[then("KYC tier level is updated appropriately")]
async fn kyc_tier_updated_appropritately(world: &mut FullE2eWorld) {
    if let Some(response) = &world.last_response {
        if let Some(body) = &response.body {
            if let Some(tier) = body.get("tier") {
                if tier.is_null() {
                    world.error = Some("KYC tier level is null".to_string());
                }
            } else {
                world.error = Some("user profile missing tier field".to_string());
            }
        }
    }
}

#[then("the response contains flow ID")]
async fn response_contains_flow_id(world: &mut FullE2eWorld) {
    if let Some(response) = &world.last_response {
        if let Some(body) = &response.body {
            if body.get("id").is_none() {
                world.error = Some(format!("response missing flow ID: {}", response.text));
            } else {
                world.flow.id_document_flow_id = body.get("id").and_then(|v| v.as_str()).map(String::from);
            }
        }
    }
}

#[tokio::main]
async fn main() {
    FullE2eWorld::run("tests/features/full").await;
}
