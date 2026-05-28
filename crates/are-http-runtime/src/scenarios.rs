use crate::request::RuntimeRequest;
use crate::response::{RuntimeResponse, runtime_response};
use crate::store::RuntimeState;
use crate::{PreparedProject, RuntimeError, TestScenario};
use tiny_http::Method;

pub(crate) fn test_ping_scenario(prepared: &PreparedProject) -> Result<TestScenario, RuntimeError> {
    let state = RuntimeState::default();
    let response = runtime_response(
        &state,
        &prepared.contracts,
        &prepared.functions,
        &RuntimeRequest::new(Method::Get, "/ping", ""),
    );

    expect_status(&response, 200, "GET /ping")?;
    expect_json_string(&response, "message", "pong", "GET /ping")?;

    Ok(TestScenario {
        name: "minimal ping HTTP flow".to_string(),
        checks: vec![
            "GET /ping returned 200".to_string(),
            "GET /ping returned message=pong".to_string(),
        ],
    })
}

pub(crate) fn test_users_scenario(
    prepared: &PreparedProject,
) -> Result<TestScenario, RuntimeError> {
    let state = RuntimeState::default();
    let mut checks = Vec::new();

    let health = runtime_response(
        &state,
        &prepared.contracts,
        &prepared.functions,
        &RuntimeRequest::new(Method::Get, "/health", ""),
    );
    expect_status(&health, 200, "GET /health")?;
    expect_json_string(&health, "status", "ok", "GET /health")?;
    checks.push("GET /health returned 200".to_string());

    let invalid = runtime_response(
        &state,
        &prepared.contracts,
        &prepared.functions,
        &RuntimeRequest::new(
            Method::Post,
            "/users",
            r#"{"email":"invalid","name":"Ada"}"#,
        ),
    );
    expect_status(&invalid, 400, "POST /users invalid email")?;
    expect_json_string(
        &invalid,
        "error",
        "invalid_email",
        "POST /users invalid email",
    )?;
    checks.push("POST /users rejects invalid email with 400".to_string());

    let created = runtime_response(
        &state,
        &prepared.contracts,
        &prepared.functions,
        &RuntimeRequest::new(
            Method::Post,
            "/users",
            r#"{"email":"ada@example.com","name":"Ada Lovelace"}"#,
        ),
    );
    expect_status(&created, 201, "POST /users")?;
    expect_json_u64(&created, "id", 1, "POST /users")?;
    expect_json_string(&created, "email", "ada@example.com", "POST /users")?;
    checks.push("POST /users creates a user with 201".to_string());

    let listed = runtime_response(
        &state,
        &prepared.contracts,
        &prepared.functions,
        &RuntimeRequest::new(Method::Get, "/users", ""),
    );
    expect_status(&listed, 200, "GET /users")?;
    expect_json_array_len(&listed, 1, "GET /users")?;
    expect_json_array_string(&listed, 0, "email", "ada@example.com", "GET /users")?;
    checks.push("GET /users lists persisted users".to_string());

    let searched = runtime_response(
        &state,
        &prepared.contracts,
        &prepared.functions,
        &RuntimeRequest::new(Method::Get, "/users/search?email=ada%40example.com", ""),
    );
    expect_status(&searched, 200, "GET /users/search")?;
    expect_json_string(&searched, "email", "ada@example.com", "GET /users/search")?;
    checks.push("GET /users/search decodes typed query params".to_string());

    let auth_check = runtime_response(
        &state,
        &prepared.contracts,
        &prepared.functions,
        &RuntimeRequest::new(Method::Get, "/users/auth-check", "")
            .with_header("authorization", "Bearer dev-token"),
    );
    expect_status(&auth_check, 200, "GET /users/auth-check")?;
    expect_json_bool(&auth_check, "authorized", true, "GET /users/auth-check")?;
    checks.push("GET /users/auth-check decodes typed headers".to_string());

    let session = runtime_response(
        &state,
        &prepared.contracts,
        &prepared.functions,
        &RuntimeRequest::new(Method::Get, "/session", "")
            .with_header("cookie", "session_id=session-dev-123"),
    );
    expect_status(&session, 200, "GET /session")?;
    expect_json_string(&session, "session_id", "session-dev-123", "GET /session")?;
    expect_json_bool(&session, "active", true, "GET /session")?;
    checks.push("GET /session decodes typed cookies".to_string());

    let fetched = runtime_response(
        &state,
        &prepared.contracts,
        &prepared.functions,
        &RuntimeRequest::new(Method::Get, "/users/1", ""),
    );
    expect_status(&fetched, 200, "GET /users/1")?;
    expect_json_string(&fetched, "name", "Ada Lovelace", "GET /users/1")?;
    checks.push("GET /users/{id: UserId} fetches the created user".to_string());

    Ok(TestScenario {
        name: "users API HTTP flow".to_string(),
        checks,
    })
}

fn expect_status(
    response: &RuntimeResponse,
    expected: u16,
    label: &str,
) -> Result<(), RuntimeError> {
    if response.status == expected {
        return Ok(());
    }

    Err(RuntimeError::Test(format!(
        "{label} expected HTTP {expected}, got {} with body {}",
        response.status, response.body
    )))
}

fn expect_json_string(
    response: &RuntimeResponse,
    field: &str,
    expected: &str,
    label: &str,
) -> Result<(), RuntimeError> {
    if response.body.get(field).and_then(serde_json::Value::as_str) == Some(expected) {
        return Ok(());
    }

    Err(RuntimeError::Test(format!(
        "{label} expected JSON field `{field}` to be `{expected}`, got {}",
        response.body
    )))
}

fn expect_json_u64(
    response: &RuntimeResponse,
    field: &str,
    expected: u64,
    label: &str,
) -> Result<(), RuntimeError> {
    if response.body.get(field).and_then(serde_json::Value::as_u64) == Some(expected) {
        return Ok(());
    }

    Err(RuntimeError::Test(format!(
        "{label} expected JSON field `{field}` to be `{expected}`, got {}",
        response.body
    )))
}

fn expect_json_bool(
    response: &RuntimeResponse,
    field: &str,
    expected: bool,
    label: &str,
) -> Result<(), RuntimeError> {
    if response
        .body
        .get(field)
        .and_then(serde_json::Value::as_bool)
        == Some(expected)
    {
        return Ok(());
    }

    Err(RuntimeError::Test(format!(
        "{label} expected JSON field `{field}` to be `{expected}`, got {}",
        response.body
    )))
}

fn expect_json_array_len(
    response: &RuntimeResponse,
    expected: usize,
    label: &str,
) -> Result<(), RuntimeError> {
    if response
        .body
        .as_array()
        .is_some_and(|items| items.len() == expected)
    {
        return Ok(());
    }

    Err(RuntimeError::Test(format!(
        "{label} expected JSON array length `{expected}`, got {}",
        response.body
    )))
}

fn expect_json_array_string(
    response: &RuntimeResponse,
    index: usize,
    field: &str,
    expected: &str,
    label: &str,
) -> Result<(), RuntimeError> {
    if response
        .body
        .as_array()
        .and_then(|items| items.get(index))
        .and_then(|item| item.get(field))
        .and_then(serde_json::Value::as_str)
        == Some(expected)
    {
        return Ok(());
    }

    Err(RuntimeError::Test(format!(
        "{label} expected JSON array item {index} field `{field}` to be `{expected}`, got {}",
        response.body
    )))
}
