use crate::contracts::{
    HttpContractManifest, HttpRouteContract, find_single_service, match_route, route_summary_line,
};
use crate::functions::RuntimeFunctions;
use crate::host::RuntimeHost;
use crate::response::runtime_response;
use crate::schemas::RuntimeSchemas;
use crate::store::RuntimeState;
use crate::test_project;
use are_ast::{ModelDecl, ModelField, ModelFieldAttr, Path as AstPath, TypeExpr};
use are_diagnostics::{Position, SourceRange};
use are_interpreter::Host;
use are_project::check_path;
use std::collections::HashMap;
use std::path::Path;
use tiny_http::Method;

#[test]
fn matches_named_route_params() {
    let params = match_route("/users/:id", "/users/42").expect("route matches");
    assert_eq!(params.get("id").expect("id"), "42");

    let params = match_route("/users/{id: UserId}", "/users/42").expect("route matches");
    assert_eq!(params.get("id").expect("id"), "42");
}

#[test]
fn formats_route_summary_lines() {
    let line = route_summary_line(&route(
        "POST",
        "/users",
        Some("CreateUserInput"),
        Some("User"),
        Some(201),
        "create_user",
    ));
    assert_eq!(
        line,
        "  POST   /users body CreateUserInput returns User status 201 -> create_user"
    );
}

#[test]
fn builds_http_contract_manifest_from_service() {
    let check = check_path(Path::new("../../examples/users_api")).expect("project checks");
    assert!(check.ok(), "{:#?}", check.diagnostics);
    let service = find_single_service(&check.modules).expect("single service");
    let contracts = HttpContractManifest::from_service(service).expect("contracts");

    assert_eq!(contracts.service, "UsersApi");
    assert_eq!(contracts.routes.len(), 3);
    assert!(contracts.has("POST", "/users"));
    assert_eq!(contracts.error_mapper.as_deref(), Some("map_error"));

    let create_user = contracts
        .routes
        .iter()
        .find(|route| route.method == "POST" && route.path == "/users")
        .expect("POST /users");
    assert_eq!(create_user.body_type.as_deref(), Some("CreateUserInput"));
    assert_eq!(create_user.response_type.as_deref(), Some("User"));
    assert_eq!(create_user.status, Some(201));

    let get_user = contracts
        .routes
        .iter()
        .find(|route| route.method == "GET" && route.path == "/users/{id: UserId}")
        .expect("GET /users/{id: UserId}");
    assert_eq!(get_user.path_params.len(), 1);
    assert_eq!(get_user.path_params[0].name, "id");
    assert_eq!(get_user.path_params[0].ty.as_deref(), Some("UserId"));
}

#[test]
fn handles_users_api_flow() {
    let state = RuntimeState::default();
    let contracts = HttpContractManifest {
        service: "UsersApi".to_string(),
        routes: vec![
            route(
                "GET",
                "/health",
                None,
                Some("HealthResponse"),
                Some(200),
                "health",
            ),
            route(
                "POST",
                "/users",
                Some("CreateUserInput"),
                Some("User"),
                Some(201),
                "create_user",
            ),
            route(
                "GET",
                "/users/{id: UserId}",
                None,
                Some("User"),
                Some(200),
                "get_user",
            ),
        ],
        error_mapper: Some("map_error".to_string()),
    };
    let functions = users_api_functions();

    let health = runtime_response(&state, &contracts, &functions, &Method::Get, "/health", "");
    assert_eq!(health.status, 200);
    assert_eq!(health.body["status"], "ok");

    let created = runtime_response(
        &state,
        &contracts,
        &functions,
        &Method::Post,
        "/users",
        r#"{"email":"ada@example.com","name":"Ada"}"#,
    );
    assert_eq!(created.status, 201);

    let invalid = runtime_response(
        &state,
        &contracts,
        &functions,
        &Method::Post,
        "/users",
        r#"{"email":"invalid","name":"Ada"}"#,
    );
    assert_eq!(invalid.status, 400);
    assert_eq!(invalid.body["error"], "invalid_email");

    let fetched = runtime_response(&state, &contracts, &functions, &Method::Get, "/users/1", "");
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.body["email"], "ada@example.com");
}

#[test]
fn runtime_store_uses_model_collection_contracts() {
    let schemas = RuntimeSchemas {
        models: [("Post".to_string(), model_decl("Post", "title"))].into(),
        ..RuntimeSchemas::default()
    };
    let state = RuntimeState::default();
    let params = HashMap::new();
    let mut host = RuntimeHost {
        state: &state,
        params: &params,
        request_body: "",
        schemas: &schemas,
    };

    let created = host
        .insert_model("posts", serde_json::json!({ "title": "Hello" }))
        .expect("post inserts");
    assert_eq!(created["id"], 1);
    assert_eq!(created["title"], "Hello");

    let fetched = host
        .get_model("posts", serde_json::json!(1))
        .expect("post fetches");
    assert_eq!(fetched, created);
}

#[test]
fn handles_minimal_hello_api_flow() {
    let state = RuntimeState::default();
    let check = check_path(Path::new("../../examples/hello_api")).expect("project checks");
    assert!(check.ok(), "{:#?}", check.diagnostics);
    let service = find_single_service(&check.modules).expect("single service");
    let contracts = HttpContractManifest::from_service(service).expect("contracts");
    let functions = RuntimeFunctions::from_modules(&check.modules);

    let ping = runtime_response(&state, &contracts, &functions, &Method::Get, "/ping", "");
    assert_eq!(ping.status, 200);
    assert_eq!(ping.body["message"], "pong");
}

#[test]
fn tests_minimal_hello_api_project() {
    let report = test_project(Path::new("../../examples/hello_api")).expect("project tests");
    assert_eq!(report.package, "hello-api");
    assert_eq!(report.service, "HelloApi");
    assert_eq!(report.routes.len(), 1);
    assert_eq!(report.scenarios.len(), 1);
    assert_eq!(report.scenarios[0].name, "minimal ping HTTP flow");
}

#[test]
fn tests_users_api_project() {
    let report = test_project(Path::new("../../examples/users_api")).expect("project tests");
    assert_eq!(report.package, "users-api");
    assert_eq!(report.service, "UsersApi");
    assert_eq!(report.routes.len(), 3);
    assert_eq!(report.scenarios.len(), 1);
    assert_eq!(report.scenarios[0].name, "users API HTTP flow");
}

fn users_api_functions() -> RuntimeFunctions {
    let check = check_path(Path::new("../../examples/users_api")).expect("project checks");
    assert!(check.ok(), "{:#?}", check.diagnostics);
    RuntimeFunctions::from_modules(&check.modules)
}

fn route(
    method: &str,
    path: &str,
    body_type: Option<&str>,
    response_type: Option<&str>,
    status: Option<u16>,
    handler: &str,
) -> HttpRouteContract {
    HttpRouteContract {
        method: method.to_string(),
        path: path.to_string(),
        body_type: body_type.map(str::to_string),
        response_type: response_type.map(str::to_string),
        status,
        path_params: Vec::new(),
        handler: handler.to_string(),
    }
}

fn model_decl(name: &str, text_field: &str) -> ModelDecl {
    let range = SourceRange::new(Position::new(1, 1), Position::new(1, 1));
    ModelDecl {
        name: name.to_string(),
        fields: vec![
            ModelField {
                name: "id".to_string(),
                ty: type_path("U64", range),
                attrs: vec![ModelFieldAttr::Primary],
                range,
            },
            ModelField {
                name: text_field.to_string(),
                ty: type_path("String", range),
                attrs: Vec::new(),
                range,
            },
        ],
        range,
    }
}

fn type_path(name: &str, range: SourceRange) -> TypeExpr {
    TypeExpr::Path {
        path: AstPath {
            segments: vec![name.to_string()],
            range,
        },
    }
}
