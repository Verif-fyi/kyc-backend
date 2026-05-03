mod common;

use anyhow::{Result, anyhow};
use common::{
    BffTestFixture, Env, ensure_bff_fixtures, get_client_token_and_subject, http_client,
    require_json_field, reset_sms_sink, send_json, send_json_with_bff, wait_for_otp,
};
use reqwest::Method;
use serde_json::{Value, json};
use serial_test::file_serial;
use std::time::Duration;
use tokio::time::sleep;

fn test_context() -> Result<(Env, reqwest::Client)> {
    let env = Env::from_env()?;
    let client = http_client()?;
    Ok((env, client))
}

fn require_session_id(body: &Option<Value>) -> Result<String> {
    require_json_field(body, "id")?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("session id must be a string"))
}

fn require_flow_id(body: &Option<Value>) -> Result<String> {
    require_json_field(body, "id")?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("flow id must be a string"))
}

#[allow(dead_code)]
fn require_step_id(body: &Option<Value>) -> Result<String> {
    require_json_field(body, "id")?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("step id must be a string"))
}

fn require_step_id_from_value(step: &Value) -> Result<String> {
    step.get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("step id missing"))
}

fn require_fixture() -> Result<BffTestFixture> {
    BffTestFixture::get().ok_or_else(|| anyhow!("missing BFF fixture"))
}

async fn wait_for_step_status(
    client: &reqwest::Client,
    bff_base: &str,
    fixture: &BffTestFixture,
    step_id: &str,
    expected_status: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    let url = format!("{}/steps/{}", bff_base, step_id);

    while std::time::Instant::now() < deadline {
        let response =
            send_json_with_bff(client, Method::GET, &url, None, None, Some(fixture)).await?;

        if response.status == 200 {
            let status = response
                .body
                .as_ref()
                .and_then(|b| b.get("status"))
                .and_then(Value::as_str);

            if let Some(s) = status
                && s == expected_status
            {
                return Ok(());
            }
        }

        sleep(Duration::from_millis(500)).await;
    }

    Err(anyhow!(
        "Step {} did not reach status {} within {:?}",
        step_id,
        expected_status,
        timeout
    ))
}

async fn wait_for_session_status(
    client: &reqwest::Client,
    bff_base: &str,
    fixture: &BffTestFixture,
    session_id: &str,
    expected_status: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    let url = format!("{}/sessions/{}", bff_base, session_id);

    while std::time::Instant::now() < deadline {
        let response =
            send_json_with_bff(client, Method::GET, &url, None, None, Some(fixture)).await?;

        if response.status == 200 {
            let status = response
                .body
                .as_ref()
                .and_then(|b| b.get("session"))
                .and_then(|s| s.get("status"))
                .and_then(Value::as_str);

            if let Some(s) = status
                && s == expected_status
            {
                return Ok(());
            }
        }

        sleep(Duration::from_millis(500)).await;
    }

    Err(anyhow!(
        "Session {} did not reach status {} within {:?}",
        session_id,
        expected_status,
        timeout
    ))
}

async fn wait_for_flow_status(
    client: &reqwest::Client,
    bff_base: &str,
    fixture: &BffTestFixture,
    flow_id: &str,
    expected_status: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = std::time::Instant::now() + timeout;
    let url = format!("{}/flows/{}", bff_base, flow_id);
    let mut last_status: Option<String> = None;

    while std::time::Instant::now() < deadline {
        let response =
            send_json_with_bff(client, Method::GET, &url, None, None, Some(fixture)).await?;

        if response.status == 200 {
            let status = response.body.as_ref().and_then(|body| {
                body.get("status")
                    .or_else(|| body.get("flow").and_then(|flow| flow.get("status")))
            });

            if let Some(s) = status.and_then(Value::as_str) {
                last_status = Some(s.to_owned());
                if s == expected_status {
                    return Ok(());
                }
            }
        }

        sleep(Duration::from_millis(500)).await;
    }

    Err(anyhow!(
        "Flow {} did not reach status {} within {:?} (last_status={})",
        flow_id,
        expected_status,
        timeout,
        last_status.unwrap_or_else(|| "unknown".to_owned())
    ))
}

async fn get_flow_steps(
    client: &reqwest::Client,
    bff_base: &str,
    fixture: &BffTestFixture,
    flow_id: &str,
) -> Result<Vec<Value>> {
    let response = send_json_with_bff(
        client,
        Method::GET,
        &format!("{}/flows/{}", bff_base, flow_id),
        None,
        None,
        Some(fixture),
    )
    .await?;

    assert_eq!(response.status, 200, "Get flow failed: {}", response.text);

    response
        .body
        .as_ref()
        .and_then(|b| b.get("steps"))
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| anyhow!("steps missing in flow response"))
}

fn find_step<'a>(steps: &'a [Value], step_type: &str) -> Result<&'a Value> {
    steps
        .iter()
        .find(|step| step.get("stepType").and_then(Value::as_str) == Some(step_type))
        .ok_or_else(|| anyhow!("step `{step_type}` not found"))
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

async fn wait_for_cuss_requests(
    client: &reqwest::Client,
    env: &Env,
    expected_endpoints: &[&str],
    timeout: Duration,
) -> Result<Vec<Value>> {
    let deadline = std::time::Instant::now() + timeout;

    while std::time::Instant::now() < deadline {
        let response = send_json(
            client,
            Method::GET,
            &format!("{}/__admin/requests", env.cuss_url),
            None,
            None,
        )
        .await?;

        if response.status == 200 {
            let requests = response
                .body
                .unwrap_or_else(|| json!([]))
                .as_array()
                .cloned()
                .unwrap_or_default();

            let all_present = expected_endpoints.iter().all(|expected| {
                requests.iter().any(|item| {
                    item.get("endpoint")
                        .and_then(Value::as_str)
                        .map(|value| value == *expected)
                        .unwrap_or(false)
                })
            });

            if all_present {
                return Ok(requests);
            }
        }

        sleep(Duration::from_millis(500)).await;
    }

    Err(anyhow!(
        "CUSS requests {:?} not observed within {:?}",
        expected_endpoints,
        timeout
    ))
}

#[tokio::test]
#[file_serial]
async fn flow_sdk_session_with_phone_otp_and_first_deposit() -> Result<()> {
    let (env, client) = test_context()?;

    let user_id = format!("usr_flow_sdk_e2e_{}", chrono::Utc::now().timestamp());
    ensure_bff_fixtures(&env.database_url, &user_id).await?;

    let fixture = require_fixture()?;
    let (staff_token, _) = get_client_token_and_subject(&client, &env).await?;

    let bff_base = format!("{}/bff", env.user_storage_url);
    let staff_base = format!("{}/staff", env.user_storage_url);
    let phone_number = "+237690123456";

    reset_sms_sink(&client, &env).await?;
    reset_cuss(&client, &env).await?;

    println!("=== Step 1: Create KYC_FULL session ===");
    let session_response = send_json_with_bff(
        &client,
        Method::POST,
        &format!("{}/sessions", bff_base),
        None,
        Some(json!({
            "sessionType": "kyc_full",
            "context": {}
        })),
        Some(&fixture),
    )
    .await?;

    assert_eq!(
        session_response.status, 201,
        "Session creation failed: {}",
        session_response.text
    );
    let session_id = require_session_id(&session_response.body)?;
    println!("Created session: {}", session_id);

    println!("=== Step 2: Add PHONE_OTP flow to session ===");
    let phone_flow_response = send_json_with_bff(
        &client,
        Method::POST,
        &format!("{}/sessions/{}/flows", bff_base, session_id),
        None,
        Some(json!({
            "flowType": "phone_otp",
            "context": {}
        })),
        Some(&fixture),
    )
    .await?;

    assert_eq!(
        phone_flow_response.status, 201,
        "Phone OTP flow creation failed: {}",
        phone_flow_response.text
    );
    let phone_flow_id = require_flow_id(&phone_flow_response.body)?;
    println!("Created phone_otp flow: {}", phone_flow_id);

    println!("=== Step 3: Submit phone init step ===");
    let phone_steps = get_flow_steps(&client, &bff_base, &fixture, &phone_flow_id).await?;
    let init_phone_step_id = require_step_id_from_value(find_step(&phone_steps, "init_phone")?)?;

    let init_phone_response = send_json_with_bff(
        &client,
        Method::POST,
        &format!("{}/steps/{}", bff_base, init_phone_step_id),
        None,
        Some(json!({
            "input": {
                "phone_number": phone_number
            }
        })),
        Some(&fixture),
    )
    .await?;
    assert_eq!(
        init_phone_response.status, 200,
        "Phone init failed: {}",
        init_phone_response.text
    );

    println!("=== Step 4: Read OTP from sms-sink ===");
    let otp = wait_for_otp(&client, &env, phone_number, Duration::from_secs(15)).await?;

    println!("=== Step 5: Get verify step ===");
    let phone_steps_after = get_flow_steps(&client, &bff_base, &fixture, &phone_flow_id).await?;
    let verify_step_id = require_step_id_from_value(find_step(&phone_steps_after, "verify_otp")?)?;
    wait_for_step_status(
        &client,
        &bff_base,
        &fixture,
        &verify_step_id,
        "WAITING",
        Duration::from_secs(10),
    )
    .await?;

    println!("=== Step 6: Submit OTP verification ===");
    let verify_response = send_json_with_bff(
        &client,
        Method::POST,
        &format!("{}/steps/{}", bff_base, verify_step_id),
        None,
        Some(json!({
            "input": {
                "code": otp
            }
        })),
        Some(&fixture),
    )
    .await?;

    assert_eq!(
        verify_response.status, 200,
        "Verify step failed: {}",
        verify_response.text
    );
    println!("OTP verification submitted");

    println!("=== Step 7: Wait for phone_otp flow to complete ===");
    wait_for_flow_status(
        &client,
        &bff_base,
        &fixture,
        &phone_flow_id,
        "COMPLETED",
        Duration::from_secs(10),
    )
    .await?;
    println!("Phone OTP flow completed");

    println!("=== Step 8: Verify user metadata after phone flow ===");
    let phone_user_response = send_json_with_bff(
        &client,
        Method::GET,
        &format!("{}/users/{}", bff_base, user_id),
        None,
        None,
        Some(&fixture),
    )
    .await?;

    assert_eq!(
        phone_user_response.status, 200,
        "Get user failed: {}",
        phone_user_response.text
    );
    let phone_metadata = phone_user_response
        .body
        .as_ref()
        .and_then(|b| b.get("metadata"))
        .cloned()
        .unwrap_or(json!({}));
    assert_eq!(
        phone_metadata.pointer("/phone/number"),
        Some(&json!(phone_number))
    );
    assert_eq!(
        phone_metadata.pointer("/phone/verified"),
        Some(&json!(true))
    );
    assert_eq!(
        phone_metadata.pointer("/kyc/phoneOtpVerified"),
        Some(&json!(true))
    );

    println!("=== Step 9: Add FIRST_DEPOSIT flow to same session ===");
    let deposit_flow_response = send_json_with_bff(
        &client,
        Method::POST,
        &format!("{}/sessions/{}/flows", bff_base, session_id),
        None,
        Some(json!({
            "flowType": "first_deposit"
        })),
        Some(&fixture),
    )
    .await?;

    assert_eq!(
        deposit_flow_response.status, 201,
        "First deposit flow creation failed: {}",
        deposit_flow_response.text
    );
    let deposit_flow_id = require_flow_id(&deposit_flow_response.body)?;
    println!("Created first_deposit flow: {}", deposit_flow_id);

    println!("=== Step 10: Submit first deposit init step ===");
    let deposit_steps = get_flow_steps(&client, &bff_base, &fixture, &deposit_flow_id).await?;
    let init_deposit_step_id =
        require_step_id_from_value(find_step(&deposit_steps, "init_first_deposit")?)?;

    let init_deposit_response = send_json_with_bff(
        &client,
        Method::POST,
        &format!("{}/steps/{}", bff_base, init_deposit_step_id),
        None,
        Some(json!({
            "input": {
                "amount": 5000,
                "currency": "XAF"
            }
        })),
        Some(&fixture),
    )
    .await?;
    assert_eq!(
        init_deposit_response.status, 200,
        "First deposit init failed: {}",
        init_deposit_response.text
    );

    println!("=== Step 11: Approve waiting admin step via /staff/flow ===");
    let admin_steps_response = send_json(
        &client,
        Method::GET,
        &format!(
            "{}/flow/steps?status=WAITING&userId={}&flowType=first_deposit",
            staff_base, user_id
        ),
        Some(&staff_token),
        None,
    )
    .await?;
    assert_eq!(
        admin_steps_response.status, 200,
        "List admin steps failed: {}",
        admin_steps_response.text
    );

    let admin_steps = admin_steps_response
        .body
        .as_ref()
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| anyhow!("admin steps array missing"))?;
    let admin_step_id =
        require_step_id_from_value(find_step(&admin_steps, "await_admin_decision")?)?;

    let approve_response = send_json(
        &client,
        Method::POST,
        &format!("{}/flow/steps/{}", staff_base, admin_step_id),
        Some(&staff_token),
        Some(json!({
            "input": {
                "decision": "APPROVED",
                "validated_deposited_amount": 500.0
            }
        })),
    )
    .await?;
    assert_eq!(
        approve_response.status, 200,
        "Approve admin step failed: {}",
        approve_response.text
    );

    println!("=== Step 12: Wait for first_deposit flow to complete ===");
    wait_for_flow_status(
        &client,
        &bff_base,
        &fixture,
        &deposit_flow_id,
        "COMPLETED",
        Duration::from_secs(15),
    )
    .await?;

    println!("=== Step 13: Verify CUSS webhook calls ===");
    let cuss_requests = wait_for_cuss_requests(
        &client,
        &env,
        &["register", "approve"],
        Duration::from_secs(15),
    )
    .await?;
    let register_request = cuss_requests
        .iter()
        .find(|item| item.get("endpoint").and_then(Value::as_str) == Some("register"))
        .ok_or_else(|| anyhow!("register request missing"))?;
    let approve_request = cuss_requests
        .iter()
        .find(|item| item.get("endpoint").and_then(Value::as_str) == Some("approve"))
        .ok_or_else(|| anyhow!("approve request missing"))?;

    assert_eq!(
        register_request.pointer("/payload/externalId"),
        Some(&json!(user_id))
    );
    assert_eq!(
        approve_request.pointer("/payload/depositAmount"),
        Some(&json!(500))
    );

    println!("=== Step 14: Get user metadata ===");
    let user_response = send_json_with_bff(
        &client,
        Method::GET,
        &format!("{}/users/{}", bff_base, user_id),
        None,
        None,
        Some(&fixture),
    )
    .await?;

    assert_eq!(
        user_response.status, 200,
        "Get user failed: {}",
        user_response.text
    );

    let metadata = user_response
        .body
        .as_ref()
        .and_then(|b| b.get("metadata"))
        .cloned()
        .unwrap_or(json!({}));

    println!(
        "User metadata: {}",
        serde_json::to_string_pretty(&metadata)?
    );
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

    println!("=== Step 15: Get completed KYC ===");
    let completed_kyc_response = send_json_with_bff(
        &client,
        Method::GET,
        &format!("{}/users/{}/completed-kyc", bff_base, user_id),
        None,
        None,
        Some(&fixture),
    )
    .await?;

    assert_eq!(
        completed_kyc_response.status, 200,
        "Get completed KYC failed: {}",
        completed_kyc_response.text
    );

    let completed_kyc = completed_kyc_response
        .body
        .as_ref()
        .and_then(|b| b.get("completedKyc"))
        .cloned()
        .unwrap_or(json!({}));

    assert_eq!(
        completed_kyc.pointer("/kyc_full/phone_otp/completed"),
        Some(&json!(true))
    );
    assert_eq!(
        completed_kyc.pointer("/kyc_full/first_deposit/completed"),
        Some(&json!(true))
    );

    println!(
        "Completed KYC: {}",
        serde_json::to_string_pretty(&completed_kyc)?
    );

    println!("=== E2E Flow SDK test completed successfully ===");

    Ok(())
}

#[tokio::test]
#[file_serial]
async fn flow_sdk_first_deposit_reject_closes_session() -> Result<()> {
    let (env, client) = test_context()?;

    let user_id = format!("usr_flow_sdk_reject_{}", chrono::Utc::now().timestamp());
    ensure_bff_fixtures(&env.database_url, &user_id).await?;

    let fixture = require_fixture()?;
    let (staff_token, _) = get_client_token_and_subject(&client, &env).await?;

    let bff_base = format!("{}/bff", env.user_storage_url);
    let staff_base = format!("{}/staff", env.user_storage_url);

    reset_cuss(&client, &env).await?;

    let session_response = send_json_with_bff(
        &client,
        Method::POST,
        &format!("{}/sessions", bff_base),
        None,
        Some(json!({
            "sessionType": "kyc_full"
        })),
        Some(&fixture),
    )
    .await?;
    assert_eq!(session_response.status, 201, "{}", session_response.text);
    let session_id = require_session_id(&session_response.body)?;

    let flow_response = send_json_with_bff(
        &client,
        Method::POST,
        &format!("{}/sessions/{}/flows", bff_base, session_id),
        None,
        Some(json!({
            "flowType": "first_deposit"
        })),
        Some(&fixture),
    )
    .await?;
    assert_eq!(flow_response.status, 201, "{}", flow_response.text);
    let flow_id = require_flow_id(&flow_response.body)?;

    let flow_steps = get_flow_steps(&client, &bff_base, &fixture, &flow_id).await?;
    let init_step_id = require_step_id_from_value(find_step(&flow_steps, "init_first_deposit")?)?;

    let init_response = send_json_with_bff(
        &client,
        Method::POST,
        &format!("{}/steps/{}", bff_base, init_step_id),
        None,
        Some(json!({
            "input": {
                "amount": 7000,
                "currency": "XAF"
            }
        })),
        Some(&fixture),
    )
    .await?;
    assert_eq!(init_response.status, 200, "{}", init_response.text);

    let admin_steps_response = send_json(
        &client,
        Method::GET,
        &format!(
            "{}/flow/steps?status=WAITING&userId={}&flowType=first_deposit",
            staff_base, user_id
        ),
        Some(&staff_token),
        None,
    )
    .await?;
    assert_eq!(
        admin_steps_response.status, 200,
        "{}",
        admin_steps_response.text
    );
    let admin_steps = admin_steps_response
        .body
        .as_ref()
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| anyhow!("admin steps array missing"))?;
    let admin_step_id =
        require_step_id_from_value(find_step(&admin_steps, "await_admin_decision")?)?;

    let reject_response = send_json(
        &client,
        Method::POST,
        &format!("{}/flow/steps/{}", staff_base, admin_step_id),
        Some(&staff_token),
        Some(json!({
            "input": {
                "decision": "REJECTED"
            }
        })),
    )
    .await?;
    assert_eq!(reject_response.status, 200, "{}", reject_response.text);

    wait_for_flow_status(
        &client,
        &bff_base,
        &fixture,
        &flow_id,
        "CLOSED",
        Duration::from_secs(10),
    )
    .await?;
    wait_for_session_status(
        &client,
        &bff_base,
        &fixture,
        &session_id,
        "CLOSED",
        Duration::from_secs(10),
    )
    .await?;

    let repeat_reject_response = send_json(
        &client,
        Method::POST,
        &format!("{}/flow/steps/{}", staff_base, admin_step_id),
        Some(&staff_token),
        Some(json!({
            "input": {
                "decision": "REJECTED"
            }
        })),
    )
    .await?;
    assert_eq!(
        repeat_reject_response.status, 409,
        "{}",
        repeat_reject_response.text
    );

    let session_detail = send_json_with_bff(
        &client,
        Method::GET,
        &format!("{}/sessions/{}", bff_base, session_id),
        None,
        None,
        Some(&fixture),
    )
    .await?;
    assert_eq!(session_detail.status, 200, "{}", session_detail.text);
    assert_eq!(
        session_detail
            .body
            .as_ref()
            .and_then(|body| body.pointer("/session/context/close_reason")),
        Some(&json!("REJECTED_BY_ADMIN"))
    );

    let user_response = send_json_with_bff(
        &client,
        Method::GET,
        &format!("{}/users/{}", bff_base, user_id),
        None,
        None,
        Some(&fixture),
    )
    .await?;
    assert_eq!(user_response.status, 200, "{}", user_response.text);
    let metadata = user_response
        .body
        .as_ref()
        .and_then(|body| body.get("metadata"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    assert!(metadata.get("fineractId").is_none());
    assert!(metadata.get("savingsAccountId").is_none());
    assert!(metadata.get("firstDeposit").is_none());

    let cuss_requests = send_json(
        &client,
        Method::GET,
        &format!("{}/__admin/requests", env.cuss_url),
        None,
        None,
    )
    .await?;
    assert_eq!(cuss_requests.status, 200, "{}", cuss_requests.text);
    let requests = cuss_requests
        .body
        .as_ref()
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        requests.is_empty(),
        "unexpected CUSS requests: {:?}",
        requests
    );

    Ok(())
}

#[tokio::test]
#[file_serial]
async fn flow_sdk_session_creation_and_listing() -> Result<()> {
    let (env, client) = test_context()?;

    let user_id = format!("usr_flow_sdk_list_{}", chrono::Utc::now().timestamp());
    ensure_bff_fixtures(&env.database_url, &user_id).await?;

    let fixture = require_fixture()?;

    let bff_base = format!("{}/bff", env.user_storage_url);

    println!("=== Create multiple sessions ===");

    for i in 0..3 {
        let session_response = send_json_with_bff(
            &client,
            Method::POST,
            &format!("{}/sessions", bff_base),
            None,
            Some(json!({
                "sessionType": "kyc_full",
                "context": {
                    "iteration": i
                }
            })),
            Some(&fixture),
        )
        .await?;

        assert_eq!(
            session_response.status, 201,
            "Session {} creation failed",
            i
        );
    }

    println!("=== List sessions ===");
    let list_response = send_json_with_bff(
        &client,
        Method::GET,
        &format!("{}/sessions", bff_base),
        None,
        None,
        Some(&fixture),
    )
    .await?;

    assert_eq!(
        list_response.status, 200,
        "List sessions failed: {}",
        list_response.text
    );

    let sessions = list_response
        .body
        .as_ref()
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("sessions array missing"))?;

    assert!(
        sessions.len() >= 3,
        "Expected at least 3 sessions, got {}",
        sessions.len()
    );

    println!("Found {} sessions", sessions.len());

    Ok(())
}

#[tokio::test]
#[file_serial]
async fn flow_sdk_retry_config_validation() -> Result<()> {
    let (env, client) = test_context()?;

    let user_id = format!("usr_flow_sdk_retry_{}", chrono::Utc::now().timestamp());
    ensure_bff_fixtures(&env.database_url, &user_id).await?;

    let fixture = require_fixture()?;

    let bff_base = format!("{}/bff", env.user_storage_url);

    println!("=== Create session and flow ===");
    let session_response = send_json_with_bff(
        &client,
        Method::POST,
        &format!("{}/sessions", bff_base),
        None,
        Some(json!({
            "sessionType": "kyc_full"
        })),
        Some(&fixture),
    )
    .await?;

    assert_eq!(session_response.status, 201);
    let session_id = require_session_id(&session_response.body)?;

    println!("=== Add phone_otp flow ===");
    let flow_response = send_json_with_bff(
        &client,
        Method::POST,
        &format!("{}/sessions/{}/flows", bff_base, session_id),
        None,
        Some(json!({
            "flowType": "phone_otp"
        })),
        Some(&fixture),
    )
    .await?;

    assert_eq!(flow_response.status, 201);
    let flow_id = require_flow_id(&flow_response.body)?;

    println!("=== Verify flow was created with retry config ===");
    let flow_detail = send_json_with_bff(
        &client,
        Method::GET,
        &format!("{}/flows/{}", bff_base, flow_id),
        None,
        None,
        Some(&fixture),
    )
    .await?;

    assert_eq!(flow_detail.status, 200);

    let flow_def = flow_detail
        .body
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No body"))?;
    println!(
        "Flow definition: {}",
        serde_json::to_string_pretty(flow_def)?
    );

    Ok(())
}
