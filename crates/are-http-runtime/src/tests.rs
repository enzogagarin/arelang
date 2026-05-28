use crate::contracts::{
    HttpContractManifest, HttpRouteContract, HttpSchemaManifest, find_single_service, match_route,
    route_summary_line,
};
use crate::functions::RuntimeFunctions;
use crate::host::RuntimeHost;
use crate::request::RuntimeRequest;
use crate::response::runtime_response;
use crate::schemas::RuntimeSchemas;
use crate::store::RuntimeState;
use crate::{openapi_project, test_project};
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
fn runtime_request_exposes_path_without_query() {
    let request = RuntimeRequest::new(Method::Get, "/users/1?expand=posts", "");
    assert_eq!(request.path(), "/users/1");
    assert_eq!(request.query(), "expand=posts");
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

    let line = route_summary_line(&HttpRouteContract {
        method: "GET".to_string(),
        path: "/users/auth-check".to_string(),
        body_type: None,
        query_type: None,
        headers_type: Some("AuthHeaders".to_string()),
        cookies_type: None,
        response_type: Some("AuthCheckResponse".to_string()),
        status: Some(200),
        error_type: None,
        path_params: Vec::new(),
        handler: "auth_check".to_string(),
    });
    assert_eq!(
        line,
        "  GET    /users/auth-check headers AuthHeaders returns AuthCheckResponse status 200 -> auth_check"
    );
}

#[test]
fn builds_http_contract_manifest_from_service() {
    let check = check_path(Path::new("../../examples/users_api")).expect("project checks");
    assert!(check.ok(), "{:#?}", check.diagnostics);
    let service = find_single_service(&check.modules).expect("single service");
    let contracts =
        HttpContractManifest::from_service_and_modules(service, &check.modules).expect("contracts");

    assert_eq!(contracts.service, "UsersApi");
    assert_eq!(contracts.routes.len(), 7);
    assert!(contracts.has("POST", "/users"));
    assert_eq!(contracts.error_mapper.as_deref(), None);
    assert_eq!(contracts.error_contract.as_deref(), Some("ApiError"));
    assert_contract_schemas(&contracts);

    let create_user = contracts
        .routes
        .iter()
        .find(|route| route.method == "POST" && route.path == "/users")
        .expect("POST /users");
    assert_eq!(create_user.body_type.as_deref(), Some("CreateUserInput"));
    assert_eq!(create_user.response_type.as_deref(), Some("User"));
    assert_eq!(create_user.status, Some(201));
    assert_eq!(create_user.error_type.as_deref(), Some("ApiError"));

    let list_users = contracts
        .routes
        .iter()
        .find(|route| route.method == "GET" && route.path == "/users")
        .expect("GET /users");
    assert_eq!(list_users.response_type.as_deref(), Some("List<User>"));
    assert_eq!(list_users.status, Some(200));
    assert_eq!(list_users.error_type.as_deref(), Some("ApiError"));

    let search_users = contracts
        .routes
        .iter()
        .find(|route| route.method == "GET" && route.path == "/users/search")
        .expect("GET /users/search");
    assert_eq!(search_users.query_type.as_deref(), Some("SearchUsersQuery"));
    assert_eq!(
        search_users.response_type.as_deref(),
        Some("SearchUsersResponse")
    );

    let auth_check = contracts
        .routes
        .iter()
        .find(|route| route.method == "GET" && route.path == "/users/auth-check")
        .expect("GET /users/auth-check");
    assert_eq!(auth_check.headers_type.as_deref(), Some("AuthHeaders"));
    assert_eq!(
        auth_check.response_type.as_deref(),
        Some("AuthCheckResponse")
    );

    let current_session = contracts
        .routes
        .iter()
        .find(|route| route.method == "GET" && route.path == "/session")
        .expect("GET /session");
    assert_eq!(
        current_session.cookies_type.as_deref(),
        Some("SessionCookies")
    );
    assert_eq!(
        current_session.response_type.as_deref(),
        Some("SessionResponse")
    );

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
                "/users",
                None,
                Some("List<User>"),
                Some(200),
                "list_users",
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
        schemas: HttpSchemaManifest::default(),
        error_mapper: None,
        error_contract: None,
    };
    let functions = users_api_functions();

    let health = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(Method::Get, "/health", ""),
    );
    assert_eq!(health.status, 200);
    assert_eq!(health.body["status"], "ok");

    let created = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(
            Method::Post,
            "/users",
            r#"{"email":"ada@example.com","name":"Ada"}"#,
        ),
    );
    assert_eq!(created.status, 201);

    let invalid = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(
            Method::Post,
            "/users",
            r#"{"email":"invalid","name":"Ada"}"#,
        ),
    );
    assert_eq!(invalid.status, 400);
    assert_eq!(invalid.body["error"], "invalid_email");

    let invalid_name = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(
            Method::Post,
            "/users",
            r#"{"email":"ada@example.com","name":"A"}"#,
        ),
    );
    assert_eq!(invalid_name.status, 400);
    assert_eq!(invalid_name.body["error"], "invalid_name");

    let fetched = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(Method::Get, "/users/1", ""),
    );
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.body["email"], "ada@example.com");

    assert_users_list_contains_created(&state, &contracts, &functions);
}

fn assert_users_list_contains_created(
    state: &RuntimeState,
    contracts: &HttpContractManifest,
    functions: &RuntimeFunctions,
) {
    let listed = runtime_response(
        state,
        contracts,
        functions,
        &request(Method::Get, "/users", ""),
    );
    assert_eq!(listed.status, 200);
    let users = listed.body.as_array().expect("users list body");
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["email"], "ada@example.com");
}

#[test]
fn maps_declarative_users_api_errors() {
    let state = RuntimeState::default();
    let check = check_path(Path::new("../../examples/users_api")).expect("project checks");
    assert!(check.ok(), "{:#?}", check.diagnostics);
    let service = find_single_service(&check.modules).expect("single service");
    let contracts =
        HttpContractManifest::from_service_and_modules(service, &check.modules).expect("contracts");
    let functions = RuntimeFunctions::from_modules(&check.modules);

    let missing = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(Method::Get, "/users/42", ""),
    );

    assert_eq!(missing.status, 404);
    assert_eq!(missing.body["error"], "not_found");
}

#[test]
fn rejects_missing_users_api_query_params() {
    let state = RuntimeState::default();
    let check = check_path(Path::new("../../examples/users_api")).expect("project checks");
    assert!(check.ok(), "{:#?}", check.diagnostics);
    let service = find_single_service(&check.modules).expect("single service");
    let contracts =
        HttpContractManifest::from_service_and_modules(service, &check.modules).expect("contracts");
    let functions = RuntimeFunctions::from_modules(&check.modules);

    let missing = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(Method::Get, "/users/search", ""),
    );

    assert_eq!(missing.status, 400);
    assert_eq!(missing.body["error"], "missing_email");

    let invalid = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(Method::Get, "/users/search?email=invalid", ""),
    );
    assert_eq!(invalid.status, 400);
    assert_eq!(invalid.body["error"], "invalid_email");
}

#[test]
fn handles_users_api_header_contracts() {
    let state = RuntimeState::default();
    let check = check_path(Path::new("../../examples/users_api")).expect("project checks");
    assert!(check.ok(), "{:#?}", check.diagnostics);
    let service = find_single_service(&check.modules).expect("single service");
    let contracts =
        HttpContractManifest::from_service_and_modules(service, &check.modules).expect("contracts");
    let functions = RuntimeFunctions::from_modules(&check.modules);

    let authorized = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(Method::Get, "/users/auth-check", "")
            .with_header("authorization", "Bearer dev-token"),
    );
    assert_eq!(authorized.status, 200);
    assert_eq!(authorized.body["authorized"], true);

    let invalid = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(Method::Get, "/users/auth-check", "").with_header("authorization", "Bearer"),
    );
    assert_eq!(invalid.status, 400);
    assert_eq!(invalid.body["error"], "invalid_authorization");

    let missing = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(Method::Get, "/users/auth-check", ""),
    );
    assert_eq!(missing.status, 400);
    assert_eq!(missing.body["error"], "missing_authorization");
}

#[test]
fn handles_users_api_cookie_contracts() {
    let state = RuntimeState::default();
    let check = check_path(Path::new("../../examples/users_api")).expect("project checks");
    assert!(check.ok(), "{:#?}", check.diagnostics);
    let service = find_single_service(&check.modules).expect("single service");
    let contracts =
        HttpContractManifest::from_service_and_modules(service, &check.modules).expect("contracts");
    let functions = RuntimeFunctions::from_modules(&check.modules);

    let active = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(Method::Get, "/session", "")
            .with_header("Cookie", "theme=dark; session_id=session+dev-123"),
    );
    assert_eq!(active.status, 200);
    assert_eq!(active.body["session_id"], "session+dev-123");
    assert_eq!(active.body["active"], true);

    let invalid = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(Method::Get, "/session", "").with_header("Cookie", "session_id=no"),
    );
    assert_eq!(invalid.status, 400);
    assert_eq!(invalid.body["error"], "invalid_session_id");

    let missing = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(Method::Get, "/session", ""),
    );
    assert_eq!(missing.status, 400);
    assert_eq!(missing.body["error"], "missing_session_id");
}

#[test]
fn runtime_store_uses_model_collection_contracts() {
    let schemas = RuntimeSchemas {
        models: [("Post".to_string(), model_decl("Post", "title"))].into(),
        ..RuntimeSchemas::default()
    };
    let state = RuntimeState::default();
    let params = HashMap::new();
    let headers = HashMap::new();
    let mut host = RuntimeHost {
        state: &state,
        params: &params,
        request_body: "",
        query: "",
        headers: &headers,
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

    let listed = host.list_model("posts").expect("posts list");
    assert_eq!(listed, serde_json::json!([created]));
}

#[test]
fn handles_minimal_hello_api_flow() {
    let state = RuntimeState::default();
    let check = check_path(Path::new("../../examples/hello_api")).expect("project checks");
    assert!(check.ok(), "{:#?}", check.diagnostics);
    let service = find_single_service(&check.modules).expect("single service");
    let contracts = HttpContractManifest::from_service(service).expect("contracts");
    let functions = RuntimeFunctions::from_modules(&check.modules);

    let ping = runtime_response(
        &state,
        &contracts,
        &functions,
        &request(Method::Get, "/ping", ""),
    );
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
    assert_eq!(report.routes.len(), 7);
    assert_eq!(report.scenarios.len(), 1);
    assert_eq!(report.scenarios[0].name, "users API HTTP flow");
}

#[test]
fn exports_users_api_openapi_document() {
    let document = openapi_project(Path::new("../../examples/users_api")).expect("openapi");

    assert_eq!(document["openapi"], "3.1.0");
    assert_eq!(document["info"]["title"], "users-api");
    assert_eq!(document["info"]["x-are-service"], "UsersApi");
    assert_eq!(document["servers"][0]["url"], "http://127.0.0.1:8080");

    let create_user = &document["paths"]["/users"]["post"];
    assert_eq!(create_user["operationId"], "create_user");
    assert_eq!(
        create_user["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/CreateUserInput"
    );
    assert_eq!(
        create_user["responses"]["201"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/User"
    );
    assert_eq!(
        create_user["responses"]["400"]["content"]["application/json"]["schema"]["properties"]["error"]
            ["type"],
        "string"
    );
    assert_eq!(
        create_user["responses"]["404"]["content"]["application/json"]["schema"]["properties"]["error"]
            ["const"],
        "not_found"
    );
    assert_eq!(
        create_user["responses"]["500"]["content"]["application/json"]["schema"]["properties"]["error"]
            ["type"],
        "string"
    );

    let list_users = &document["paths"]["/users"]["get"];
    assert_eq!(list_users["operationId"], "list_users");
    assert_eq!(
        list_users["responses"]["200"]["content"]["application/json"]["schema"]["type"],
        "array"
    );
    assert_eq!(
        list_users["responses"]["200"]["content"]["application/json"]["schema"]["items"]["$ref"],
        "#/components/schemas/User"
    );

    let get_user_param = &document["paths"]["/users/{id}"]["get"]["parameters"][0];
    assert_eq!(get_user_param["name"], "id");
    assert_eq!(
        get_user_param["schema"]["$ref"],
        "#/components/schemas/UserId"
    );

    let search_query_param = &document["paths"]["/users/search"]["get"]["parameters"][0];
    assert_eq!(search_query_param["name"], "email");
    assert_eq!(search_query_param["in"], "query");
    assert_eq!(search_query_param["required"], true);
    assert_eq!(
        search_query_param["schema"]["$ref"],
        "#/components/schemas/Email"
    );

    let auth_header_param = &document["paths"]["/users/auth-check"]["get"]["parameters"][0];
    assert_eq!(auth_header_param["name"], "authorization");
    assert_eq!(auth_header_param["in"], "header");
    assert_eq!(auth_header_param["required"], true);
    assert_eq!(
        auth_header_param["schema"]["$ref"],
        "#/components/schemas/AuthorizationHeader"
    );

    let session_cookie_param = &document["paths"]["/session"]["get"]["parameters"][0];
    assert_eq!(session_cookie_param["name"], "session_id");
    assert_eq!(session_cookie_param["in"], "cookie");
    assert_eq!(session_cookie_param["required"], true);
    assert_eq!(
        session_cookie_param["schema"]["$ref"],
        "#/components/schemas/SessionId"
    );

    let user = &document["components"]["schemas"]["User"];
    assert_eq!(user["x-are-collection"], "users");
    assert_eq!(user["properties"]["id"]["x-are-primary"], true);
    assert_eq!(user["properties"]["email"]["x-are-unique"], true);

    let email = &document["components"]["schemas"]["Email"];
    assert_eq!(email["type"], "string");
    assert_eq!(email["format"], "email");

    let display_name = &document["components"]["schemas"]["DisplayName"];
    assert_eq!(display_name["minLength"], 2);
    assert_eq!(display_name["maxLength"], 80);

    let authorization_header = &document["components"]["schemas"]["AuthorizationHeader"];
    assert_eq!(authorization_header["minLength"], 7);
    assert_eq!(authorization_header["maxLength"], 200);

    let api_error = &document["components"]["schemas"]["ApiError"];
    assert_eq!(
        api_error["oneOf"][0]["properties"]["variant"]["const"],
        "InvalidInput"
    );
    assert_eq!(api_error["oneOf"][0]["x-are-status"], 400);
    assert_eq!(api_error["oneOf"][1]["x-are-status"], 404);
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
        query_type: None,
        headers_type: None,
        cookies_type: None,
        response_type: response_type.map(str::to_string),
        status,
        error_type: None,
        path_params: Vec::new(),
        handler: handler.to_string(),
    }
}

fn assert_contract_schemas(contracts: &HttpContractManifest) {
    let user_id = contracts
        .schemas
        .aliases
        .iter()
        .find(|schema| schema.name == "UserId")
        .expect("UserId alias schema");
    assert_eq!(user_id.aliased_type, "U64");
    assert!(user_id.opaque);

    let email = contracts
        .schemas
        .aliases
        .iter()
        .find(|schema| schema.name == "Email")
        .expect("Email alias schema");
    assert_eq!(email.aliased_type, "String");
    assert_eq!(email.validations.len(), 1);

    let display_name = contracts
        .schemas
        .aliases
        .iter()
        .find(|schema| schema.name == "DisplayName")
        .expect("DisplayName alias schema");
    assert_eq!(display_name.validations.len(), 1);

    let input = contracts
        .schemas
        .structs
        .iter()
        .find(|schema| schema.name == "CreateUserInput")
        .expect("CreateUserInput struct schema");
    assert_eq!(input.fields.len(), 2);
    assert_eq!(input.fields[0].name, "email");
    assert_eq!(input.fields[0].ty, "Email");
    assert!(input.fields[0].validations.is_empty());
    assert_eq!(input.fields[1].ty, "DisplayName");
    assert!(input.fields[1].validations.is_empty());

    let user = contracts
        .schemas
        .models
        .iter()
        .find(|schema| schema.name == "User")
        .expect("User model schema");
    assert_eq!(user.collection, "users");
    assert_eq!(user.fields.len(), 3);
    assert!(user.fields[0].primary);
    assert!(user.fields[1].unique);
    assert_eq!(user.fields[2].ty, "DisplayName");

    let api_error = contracts
        .schemas
        .enums
        .iter()
        .find(|schema| schema.name == "ApiError")
        .expect("ApiError enum schema");
    assert_eq!(api_error.variants.len(), 3);
    assert_eq!(api_error.variants[0].name, "InvalidInput");
    assert_eq!(api_error.variants[0].payload[0].name, "message");
    assert_eq!(api_error.variants[0].status, Some(400));
    assert_eq!(api_error.variants[1].status, Some(404));
    assert_eq!(api_error.variants[2].status, Some(500));
}

fn request(method: Method, url: &str, body: &str) -> RuntimeRequest {
    RuntimeRequest::new(method, url, body)
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
