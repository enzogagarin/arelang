use are_ast::{
    Field, FunctionDecl, Item, ModelDecl, ModelField, Module, RouteDecl, ServiceDecl, StructDecl,
    TypeDecl, TypeExpr,
};
use are_interpreter::{
    Host, InterpretError, Value as InterpretedValue, interpret_function_with_host_and_args,
    interpret_function_with_host_and_functions,
};
use are_project::{CheckResult, Manifest, ProjectError, check_path, load_manifest, project_root};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

#[derive(Debug)]
pub enum RuntimeError {
    Project(ProjectError),
    StaticChecks(CheckResult),
    UnsupportedProject(String),
    Server(String),
    Test(String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project(err) => write!(f, "{err}"),
            Self::StaticChecks(check) => {
                writeln!(f, "static checks failed before runtime start")?;
                for diagnostic in &check.diagnostics {
                    let source = fs::read_to_string(&diagnostic.file).ok();
                    writeln!(f, "{}", diagnostic.render(source.as_deref()))?;
                }
                Ok(())
            }
            Self::UnsupportedProject(message) | Self::Server(message) | Self::Test(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<ProjectError> for RuntimeError {
    fn from(value: ProjectError) -> Self {
        Self::Project(value)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TestReport {
    pub package: String,
    pub version: String,
    pub service: String,
    pub routes: Vec<TestRoute>,
    pub scenarios: Vec<TestScenario>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestRoute {
    pub method: String,
    pub path: String,
    pub body_type: Option<String>,
    pub response_type: Option<String>,
    pub status: Option<u16>,
    pub path_params: Vec<TestPathParam>,
    pub handler: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestPathParam {
    pub name: String,
    pub ty: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestScenario {
    pub name: String,
    pub checks: Vec<String>,
}

/// Run an Arelang HTTP project.
///
/// # Errors
///
/// Returns an error if static checks fail, the manifest cannot be loaded, the
/// project is not supported by the current HTTP MVP runtime, or the TCP server
/// cannot be started.
pub fn run_project(path: &Path) -> Result<(), RuntimeError> {
    let prepared = prepare_project(path)?;
    let server_config = prepared.manifest.server.clone().ok_or_else(|| {
        RuntimeError::UnsupportedProject("server target requires [server] config".into())
    })?;
    let address = format!("{}:{}", server_config.host, server_config.port);
    let server = Server::http(&address).map_err(|err| {
        RuntimeError::Server(format!(
            "failed to start HTTP server at http://{address}\nreason: {err}\nhint: another process may already be using this address; stop it or change [server].port in are.toml"
        ))
    })?;

    print_run_summary(
        &prepared.manifest,
        &prepared.service,
        &prepared.routes,
        &address,
    );

    run_http_server(
        &server,
        &prepared.routes,
        &prepared.functions,
        &prepared.root,
        &prepared.manifest,
    );
    Ok(())
}

/// Run the MVP project test loop without opening a TCP listener.
///
/// # Errors
///
/// Returns an error if static checks fail, the project cannot be prepared for
/// the HTTP runtime, or a built-in MVP HTTP scenario fails.
pub fn test_project(path: &Path) -> Result<TestReport, RuntimeError> {
    let prepared = prepare_project(path)?;
    let mut scenarios = Vec::new();

    if prepared.routes.has("GET", "/ping") {
        scenarios.push(test_ping_scenario(&prepared)?);
    }

    if prepared.routes.has("GET", "/health")
        && prepared.routes.has("POST", "/users")
        && prepared.routes.has("GET", "/users/{id: UserId}")
    {
        scenarios.push(test_users_scenario(&prepared)?);
    }

    Ok(TestReport {
        package: prepared.manifest.package.name,
        version: prepared.manifest.package.version,
        service: prepared.service,
        routes: prepared.routes.test_routes(),
        scenarios,
    })
}

#[derive(Debug)]
struct PreparedProject {
    root: PathBuf,
    manifest: Manifest,
    service: String,
    routes: RuntimeRoutes,
    functions: RuntimeFunctions,
}

fn prepare_project(path: &Path) -> Result<PreparedProject, RuntimeError> {
    let root = project_root(path)?;
    let manifest = load_manifest(&root)?;
    if manifest.package.target != "server" {
        return Err(RuntimeError::UnsupportedProject(format!(
            "HTTP runtime requires package target `server`, got `{}`",
            manifest.package.target
        )));
    }

    let entry_path = root.join(&manifest.package.entry);
    if !entry_path.exists() {
        return Err(RuntimeError::UnsupportedProject(format!(
            "package entry `{}` does not exist",
            entry_path.display()
        )));
    }

    let check = check_path(&root)?;

    if !check.ok() {
        return Err(RuntimeError::StaticChecks(check));
    }

    let service = find_single_service(&check.modules)?;
    let routes = RuntimeRoutes::from_service(service)?;
    let functions = RuntimeFunctions::from_modules(&check.modules);

    Ok(PreparedProject {
        root,
        manifest,
        service: service.name.clone(),
        routes,
        functions,
    })
}

fn print_run_summary(manifest: &Manifest, service: &str, routes: &RuntimeRoutes, address: &str) {
    println!("{service} running at http://{address}");
    println!(
        "package {} v{}",
        manifest.package.name, manifest.package.version
    );
    println!("routes:");
    for route in &routes.routes {
        println!("{}", route_summary_line(route));
    }
}

fn route_summary_line(route: &RuntimeRoute) -> String {
    let mut contract = match &route.body_type {
        Some(body_type) => format!("{} body {body_type}", route.path),
        None => route.path.clone(),
    };
    if let Some(response_type) = &route.response_type {
        contract.push_str(" returns ");
        contract.push_str(response_type);
    }
    if let Some(status) = route.status {
        contract.push_str(" status ");
        contract.push_str(&status.to_string());
    }

    format!(
        "  {:<6} {:<36} -> {}",
        route.method, contract, route.handler
    )
}

fn run_http_server(
    server: &Server,
    routes: &RuntimeRoutes,
    functions: &RuntimeFunctions,
    root: &Path,
    manifest: &Manifest,
) {
    let state = UsersApiState::default();

    for request in server.incoming_requests() {
        let result = handle_tiny_request(request, &state, routes, functions);
        if let Err(err) = result {
            eprintln!(
                "request handling failed for {} at {}: {err}",
                manifest.package.name,
                root.display()
            );
        }
    }
}

fn handle_tiny_request(
    mut request: Request,
    state: &UsersApiState,
    routes: &RuntimeRoutes,
    functions: &RuntimeFunctions,
) -> Result<(), RuntimeError> {
    let method = request.method().clone();
    let url = request.url().to_string();
    let mut body = String::new();
    request
        .as_reader()
        .read_to_string(&mut body)
        .map_err(|err| RuntimeError::Server(format!("failed to read request body: {err}")))?;

    let response = runtime_response(state, routes, functions, &method, &url, &body);
    request
        .respond(json_response(response.status, &response.body))
        .map_err(|err| RuntimeError::Server(format!("failed to write response: {err}")))
}

fn test_ping_scenario(prepared: &PreparedProject) -> Result<TestScenario, RuntimeError> {
    let state = UsersApiState::default();
    let response = runtime_response(
        &state,
        &prepared.routes,
        &prepared.functions,
        &Method::Get,
        "/ping",
        "",
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

fn test_users_scenario(prepared: &PreparedProject) -> Result<TestScenario, RuntimeError> {
    let state = UsersApiState::default();
    let mut checks = Vec::new();

    let health = runtime_response(
        &state,
        &prepared.routes,
        &prepared.functions,
        &Method::Get,
        "/health",
        "",
    );
    expect_status(&health, 200, "GET /health")?;
    expect_json_string(&health, "status", "ok", "GET /health")?;
    checks.push("GET /health returned 200".to_string());

    let invalid = runtime_response(
        &state,
        &prepared.routes,
        &prepared.functions,
        &Method::Post,
        "/users",
        r#"{"email":"invalid","name":"Ada"}"#,
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
        &prepared.routes,
        &prepared.functions,
        &Method::Post,
        "/users",
        r#"{"email":"ada@example.com","name":"Ada Lovelace"}"#,
    );
    expect_status(&created, 201, "POST /users")?;
    expect_json_u64(&created, "id", 1, "POST /users")?;
    expect_json_string(&created, "email", "ada@example.com", "POST /users")?;
    checks.push("POST /users creates a user with 201".to_string());

    let fetched = runtime_response(
        &state,
        &prepared.routes,
        &prepared.functions,
        &Method::Get,
        "/users/1",
        "",
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

#[derive(Debug, Clone)]
struct RuntimeRoutes {
    routes: Vec<RuntimeRoute>,
    error_mapper: Option<String>,
}

#[derive(Debug, Clone)]
struct RuntimeRoute {
    method: String,
    path: String,
    body_type: Option<String>,
    response_type: Option<String>,
    status: Option<u16>,
    handler: String,
}

#[derive(Debug, Clone, Default)]
struct RuntimeFunctions {
    functions: HashMap<String, FunctionDecl>,
    schemas: RuntimeSchemas,
}

impl RuntimeRoutes {
    fn from_service(service: &ServiceDecl) -> Result<Self, RuntimeError> {
        let routes = service.routes.iter().map(runtime_route).collect::<Vec<_>>();
        let error_mapper = runtime_error_mapper(service);

        if routes.is_empty() {
            return Err(RuntimeError::UnsupportedProject(format!(
                "service `{}` must declare at least one route",
                service.name
            )));
        }

        Ok(Self {
            routes,
            error_mapper,
        })
    }

    fn route_for(
        &self,
        method: &Method,
        path: &str,
    ) -> Option<(&RuntimeRoute, HashMap<String, String>)> {
        self.routes.iter().find_map(|route| {
            if !method_matches(method, &route.method) {
                return None;
            }

            match_route(&route.path, path).map(|params| (route, params))
        })
    }

    fn has(&self, method: &str, path: &str) -> bool {
        self.routes
            .iter()
            .any(|route| route.method == method && route.path == path)
    }

    fn test_routes(&self) -> Vec<TestRoute> {
        self.routes
            .iter()
            .map(|route| TestRoute {
                method: route.method.clone(),
                path: route.path.clone(),
                body_type: route.body_type.clone(),
                response_type: route.response_type.clone(),
                status: route.status,
                path_params: route
                    .path_params()
                    .into_iter()
                    .map(|param| TestPathParam {
                        name: param.name,
                        ty: param.ty,
                    })
                    .collect(),
                handler: route.handler.clone(),
            })
            .collect()
    }
}

impl RuntimeFunctions {
    fn from_modules(modules: &[are_project::CheckedFile]) -> Self {
        let functions = modules
            .iter()
            .flat_map(|file| file.module.items.iter())
            .filter_map(|item| {
                if let Item::Function(function) = item {
                    Some((function.name.clone(), function.clone()))
                } else {
                    None
                }
            })
            .collect();

        Self {
            functions,
            schemas: RuntimeSchemas::from_modules(modules),
        }
    }

    fn get(&self, name: &str) -> Option<&FunctionDecl> {
        self.functions.get(name)
    }
}

fn runtime_route(route: &RouteDecl) -> RuntimeRoute {
    RuntimeRoute {
        method: route.method.clone(),
        path: route.path.clone(),
        body_type: route.body_type.as_ref().map(type_expr_name),
        response_type: route.response_type.as_ref().map(type_expr_name),
        status: route.status.map(|status| status.value),
        handler: route.handler.segments.join("."),
    }
}

impl RuntimeRoute {
    fn path_params(&self) -> Vec<RoutePathParam> {
        self.path
            .split('/')
            .filter_map(route_param_from_segment)
            .collect()
    }
}

fn runtime_error_mapper(service: &ServiceDecl) -> Option<String> {
    service.uses.iter().find_map(|service_use| {
        let is_error_map = service_use
            .target
            .segments
            .last()
            .is_some_and(|segment| segment == "error_map");
        if !is_error_map {
            return None;
        }

        service_use.args.first().map(|path| path.segments.join("."))
    })
}

fn find_single_service(modules: &[are_project::CheckedFile]) -> Result<&ServiceDecl, RuntimeError> {
    let services = modules
        .iter()
        .flat_map(|module| services_in_module(&module.module))
        .collect::<Vec<_>>();

    match services.as_slice() {
        [service] => Ok(*service),
        [] => Err(RuntimeError::UnsupportedProject(
            "HTTP MVP runtime needs exactly one service declaration".into(),
        )),
        _ => Err(RuntimeError::UnsupportedProject(
            "HTTP MVP runtime currently supports one service per project".into(),
        )),
    }
}

fn services_in_module(module: &Module) -> impl Iterator<Item = &ServiceDecl> {
    module.items.iter().filter_map(|item| {
        if let Item::Service(service) = item {
            Some(service)
        } else {
            None
        }
    })
}

#[derive(Debug, Default)]
struct UsersApiState {
    inner: Arc<Mutex<UsersApiInner>>,
}

#[derive(Debug, Default)]
struct UsersApiInner {
    next_id: u64,
    users: HashMap<u64, User>,
}

#[derive(Debug, Clone, Serialize)]
struct User {
    id: u64,
    email: String,
    name: String,
}

#[derive(Debug, Clone, Default)]
struct RuntimeSchemas {
    structs: HashMap<String, StructDecl>,
    models: HashMap<String, ModelDecl>,
    aliases: HashMap<String, TypeDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoutePathParam {
    name: String,
    ty: Option<String>,
}

impl RuntimeSchemas {
    fn from_modules(modules: &[are_project::CheckedFile]) -> Self {
        let mut schemas = Self::default();

        for item in modules.iter().flat_map(|file| file.module.items.iter()) {
            match item {
                Item::Struct(decl) => {
                    schemas.structs.insert(decl.name.clone(), decl.clone());
                }
                Item::Model(decl) => {
                    schemas.models.insert(decl.name.clone(), decl.clone());
                }
                Item::Type(decl) => {
                    schemas.aliases.insert(decl.name.clone(), decl.clone());
                }
                Item::Use(_) | Item::Enum(_) | Item::Function(_) | Item::Service(_) => {}
            }
        }

        schemas
    }

    fn validate_json_body(&self, type_name: &str, body: &str) -> bool {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
            return false;
        };

        self.validate_value(type_name, &value)
    }

    fn validate_value(&self, type_name: &str, value: &serde_json::Value) -> bool {
        if let Some(alias) = self.aliases.get(type_name) {
            return self.validate_type_expr(&alias.aliased, value);
        }

        if let Some(decl) = self.structs.get(type_name) {
            return self.validate_struct_fields(&decl.fields, value);
        }

        if let Some(decl) = self.models.get(type_name) {
            return self.validate_model_fields(&decl.fields, value);
        }

        validate_primitive(type_name, value)
    }

    fn validate_type_expr(&self, ty: &TypeExpr, value: &serde_json::Value) -> bool {
        match ty {
            TypeExpr::Path { path } => {
                let Some(type_name) = path.segments.first() else {
                    return false;
                };

                if path.segments.len() != 1 {
                    return true;
                }

                self.validate_value(type_name, value)
            }
            TypeExpr::Generic { base, args, .. } => {
                if path_is(base, &["Option"]) && args.len() == 1 {
                    return value.is_null() || self.validate_type_expr(&args[0], value);
                }

                true
            }
            TypeExpr::Option { inner, .. } => {
                value.is_null() || self.validate_type_expr(inner, value)
            }
        }
    }

    fn validate_struct_fields(&self, fields: &[Field], value: &serde_json::Value) -> bool {
        let Some(object) = value.as_object() else {
            return false;
        };

        fields.iter().all(|field| match object.get(&field.name) {
            Some(value) => self.validate_type_expr(&field.ty, value),
            None => type_expr_is_optional(&field.ty),
        })
    }

    fn validate_model_fields(&self, fields: &[ModelField], value: &serde_json::Value) -> bool {
        let Some(object) = value.as_object() else {
            return false;
        };

        fields.iter().all(|field| match object.get(&field.name) {
            Some(value) => self.validate_type_expr(&field.ty, value),
            None => type_expr_is_optional(&field.ty),
        })
    }

    fn decode_path_param(
        &self,
        type_name: Option<&str>,
        name: &str,
        value: &str,
    ) -> Result<serde_json::Value, InterpretError> {
        let Some(type_name) = type_name else {
            return Ok(serde_json::Value::String(value.to_string()));
        };

        match self.primitive_root(type_name).as_deref() {
            Some("U64") => value
                .parse::<u64>()
                .map(serde_json::Value::from)
                .map_err(|_| api_invalid_input(&format!("invalid_{name}"))),
            Some("I64" | "Int") => value
                .parse::<i64>()
                .map(serde_json::Value::from)
                .map_err(|_| api_invalid_input(&format!("invalid_{name}"))),
            Some("Bool") => value
                .parse::<bool>()
                .map(serde_json::Value::from)
                .map_err(|_| api_invalid_input(&format!("invalid_{name}"))),
            Some("F64") => value
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number)
                .ok_or_else(|| api_invalid_input(&format!("invalid_{name}"))),
            _ => Ok(serde_json::Value::String(value.to_string())),
        }
    }

    fn primitive_root(&self, type_name: &str) -> Option<String> {
        if is_primitive_type(type_name) {
            return Some(type_name.to_string());
        }

        let alias = self.aliases.get(type_name)?;
        let TypeExpr::Path { path } = &alias.aliased else {
            return None;
        };

        if path.segments.len() != 1 {
            return None;
        }

        self.primitive_root(&path.segments[0])
    }
}

#[derive(Debug)]
struct RuntimeResponse {
    status: u16,
    body: serde_json::Value,
}

fn runtime_response(
    state: &UsersApiState,
    routes: &RuntimeRoutes,
    functions: &RuntimeFunctions,
    method: &Method,
    url: &str,
    body: &str,
) -> RuntimeResponse {
    let path = strip_query(url);
    let Some((route, params)) = routes.route_for(method, path) else {
        return error_response(404, "not_found");
    };

    if let Some(body_type) = &route.body_type
        && !functions.schemas.validate_json_body(body_type, body)
    {
        return error_response(400, "invalid_json");
    }

    let response = interpreted_response(
        state,
        functions,
        routes.error_mapper.as_deref(),
        &route.handler,
        &params,
        body,
    );
    apply_route_response_contract(route, functions, response)
}

fn apply_route_response_contract(
    route: &RuntimeRoute,
    functions: &RuntimeFunctions,
    response: RuntimeResponse,
) -> RuntimeResponse {
    if response.status >= 400 {
        return response;
    }

    if let Some(expected_status) = route.status
        && response.status != expected_status
    {
        eprintln!(
            "Arelang response contract failed for {} {}: expected status {}, got {}",
            route.method, route.path, expected_status, response.status
        );
        return error_response(500, "invalid_response_status");
    }

    if let Some(response_type) = &route.response_type
        && !functions
            .schemas
            .validate_value(response_type, &response.body)
    {
        eprintln!(
            "Arelang response contract failed for {} {}: response body is not {}",
            route.method, route.path, response_type
        );
        return error_response(500, "invalid_response");
    }

    response
}

fn interpreted_response(
    state: &UsersApiState,
    functions: &RuntimeFunctions,
    error_mapper: Option<&str>,
    handler: &str,
    params: &HashMap<String, String>,
    body: &str,
) -> RuntimeResponse {
    let Some(function) = functions.get(handler) else {
        return error_response(500, "handler_not_found");
    };
    let mut host = UsersApiHost {
        state,
        params,
        request_body: body,
        schemas: &functions.schemas,
    };

    match interpret_function_with_host_and_functions(function, &functions.functions, &mut host) {
        Ok(InterpretedValue::HttpResponse(response)) => RuntimeResponse {
            status: response.status,
            body: response.body,
        },
        Ok(InterpretedValue::Json(_)) => error_response(500, "handler_returned_json"),
        Ok(InterpretedValue::Bool(_)) => error_response(500, "handler_returned_bool"),
        Ok(InterpretedValue::Enum(_)) => error_response(500, "handler_returned_enum"),
        Ok(InterpretedValue::Unit) => error_response(500, "handler_returned_unit"),
        Err(err) => {
            if let Some(response) = err.as_http_response() {
                return RuntimeResponse {
                    status: response.status,
                    body: response.body.clone(),
                };
            }

            if let Some(error) = err.as_raised_error() {
                return mapped_error_response(functions, error_mapper, &mut host, error.clone());
            }

            eprintln!("Arelang interpreter failed in `{handler}`: {err}");
            error_response(500, "interpreter_error")
        }
    }
}

fn mapped_error_response(
    functions: &RuntimeFunctions,
    error_mapper: Option<&str>,
    host: &mut UsersApiHost<'_>,
    error: are_interpreter::EnumValue,
) -> RuntimeResponse {
    let Some(mapper_name) = error_mapper else {
        eprintln!(
            "Arelang application error {}.{} has no mapper",
            error.enum_name, error.variant
        );
        return error_response(500, "error_mapper_missing");
    };

    let Some(mapper) = functions.get(mapper_name) else {
        eprintln!("Arelang error mapper `{mapper_name}` was not found at runtime");
        return error_response(500, "error_mapper_missing");
    };

    match interpret_function_with_host_and_args(
        mapper,
        &functions.functions,
        host,
        vec![InterpretedValue::Enum(error)],
    ) {
        Ok(InterpretedValue::HttpResponse(response)) => RuntimeResponse {
            status: response.status,
            body: response.body,
        },
        Ok(InterpretedValue::Json(_)) => error_response(500, "mapper_returned_json"),
        Ok(InterpretedValue::Bool(_)) => error_response(500, "mapper_returned_bool"),
        Ok(InterpretedValue::Enum(_)) => error_response(500, "mapper_returned_enum"),
        Ok(InterpretedValue::Unit) => error_response(500, "mapper_returned_unit"),
        Err(err) => {
            eprintln!("Arelang error mapper `{mapper_name}` failed: {err}");
            error_response(500, "error_mapper_failed")
        }
    }
}

struct UsersApiHost<'a> {
    state: &'a UsersApiState,
    params: &'a HashMap<String, String>,
    request_body: &'a str,
    schemas: &'a RuntimeSchemas,
}

impl Host for UsersApiHost<'_> {
    fn read_json_body(
        &mut self,
        type_name: Option<&str>,
    ) -> Result<serde_json::Value, InterpretError> {
        let value = serde_json::from_str::<serde_json::Value>(self.request_body)
            .map_err(|_| api_invalid_input("invalid_json"))?;

        if let Some(type_name) = type_name
            && !self.schemas.validate_value(type_name, &value)
        {
            return Err(api_invalid_input("invalid_json"));
        }

        Ok(value)
    }

    fn validate_email(&mut self, value: &serde_json::Value) -> Result<bool, InterpretError> {
        let Some(email) = value.as_str() else {
            return Ok(false);
        };

        Ok(email.contains('@'))
    }

    fn validate_length(
        &mut self,
        value: &serde_json::Value,
        min: i64,
        max: i64,
    ) -> Result<bool, InterpretError> {
        let Some(text) = value.as_str() else {
            return Ok(false);
        };

        let len = i64::try_from(text.chars().count()).map_err(|_| {
            InterpretError::UnsupportedExpression("validate.length input is too large".into())
        })?;
        Ok((min..=max).contains(&len))
    }

    fn insert_user(
        &mut self,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, InterpretError> {
        let email = input
            .get("email")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| api_invalid_input("invalid_json"))?;
        let name = input
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| api_invalid_input("invalid_json"))?;

        let mut inner = self
            .state
            .inner
            .lock()
            .expect("users api state lock poisoned");
        inner.next_id += 1;
        let user = User {
            id: inner.next_id,
            email: email.to_string(),
            name: name.to_string(),
        };
        inner.users.insert(user.id, user.clone());

        Ok(serde_json::to_value(user).expect("user serializes"))
    }

    fn read_path_param(
        &mut self,
        type_name: Option<&str>,
        name: &str,
    ) -> Result<serde_json::Value, InterpretError> {
        let Some(value) = self.params.get(name) else {
            return Err(api_invalid_input(&format!("missing_{name}")));
        };

        self.schemas.decode_path_param(type_name, name, value)
    }

    fn get_user(&mut self, id: serde_json::Value) -> Result<serde_json::Value, InterpretError> {
        let Some(id) = id.as_u64() else {
            return Err(api_invalid_input("invalid_id"));
        };

        let inner = self
            .state
            .inner
            .lock()
            .expect("users api state lock poisoned");
        let Some(user) = inner.users.get(&id) else {
            return Err(InterpretError::raised_api_error("NotFound", Vec::new()));
        };

        Ok(serde_json::to_value(user).expect("user serializes"))
    }
}

fn error_response(status: u16, error: &str) -> RuntimeResponse {
    RuntimeResponse {
        status,
        body: serde_json::json!({ "error": error }),
    }
}

fn api_invalid_input(message: &str) -> InterpretError {
    InterpretError::raised_api_error(
        "InvalidInput",
        vec![serde_json::Value::String(message.to_string())],
    )
}

fn json_response(status: u16, body: &serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    let encoded = serde_json::to_vec(body).expect("json response encodes");
    let mut response = Response::from_data(encoded).with_status_code(StatusCode(status));
    response.add_header(json_header());
    response
}

fn json_header() -> Header {
    Header::from_bytes("Content-Type", "application/json").expect("valid content-type header")
}

fn strip_query(url: &str) -> &str {
    url.split_once('?').map_or(url, |(path, _query)| path)
}

fn method_matches(actual: &Method, expected: &str) -> bool {
    matches!(
        (actual, expected),
        (Method::Get, "GET")
            | (Method::Post, "POST")
            | (Method::Put, "PUT")
            | (Method::Patch, "PATCH")
            | (Method::Delete, "DELETE")
            | (Method::Head, "HEAD")
            | (Method::Options, "OPTIONS")
    )
}

fn match_route(pattern: &str, path: &str) -> Option<HashMap<String, String>> {
    let pattern_parts = pattern
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let path_parts = path
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    if pattern_parts.len() != path_parts.len() {
        return None;
    }

    let mut params = HashMap::new();
    for (pattern_part, path_part) in pattern_parts.iter().zip(path_parts) {
        if let Some(param) = route_param_from_segment(pattern_part) {
            params.insert(param.name, path_part.to_string());
        } else if pattern_part != &path_part {
            return None;
        }
    }

    Some(params)
}

fn route_param_from_segment(segment: &str) -> Option<RoutePathParam> {
    if let Some(name) = segment.strip_prefix(':') {
        return Some(RoutePathParam {
            name: name.to_string(),
            ty: None,
        });
    }

    let inner = segment
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))?
        .trim();
    let (name, ty) = inner
        .split_once(':')
        .map_or((inner, None), |(name, ty)| (name.trim(), Some(ty.trim())));

    Some(RoutePathParam {
        name: name.to_string(),
        ty: ty.map(str::to_string),
    })
}

fn type_expr_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Path { path } => path.segments.join("."),
        TypeExpr::Generic { base, args, .. } => {
            let args = args
                .iter()
                .map(type_expr_name)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}<{args}>", base.segments.join("."))
        }
        TypeExpr::Option { inner, .. } => format!("{}?", type_expr_name(inner)),
    }
}

fn path_is(path: &are_ast::Path, expected: &[&str]) -> bool {
    path.segments.len() == expected.len()
        && path
            .segments
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
}

fn type_expr_is_optional(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::Option { .. } => true,
        TypeExpr::Generic { base, .. } => path_is(base, &["Option"]),
        TypeExpr::Path { .. } => false,
    }
}

fn validate_primitive(type_name: &str, value: &serde_json::Value) -> bool {
    match type_name {
        "String" | "Text" => value.is_string(),
        "Bool" => value.is_boolean(),
        "Int" | "I64" => value.as_i64().is_some(),
        "U64" => value.as_u64().is_some(),
        "F64" => value.as_f64().is_some(),
        _ => false,
    }
}

fn is_primitive_type(type_name: &str) -> bool {
    matches!(
        type_name,
        "String" | "Text" | "Bool" | "Int" | "I64" | "U64" | "F64"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        RuntimeFunctions, RuntimeRoutes, UsersApiState, find_single_service, match_route,
        route_summary_line, runtime_response, test_project,
    };
    use are_project::check_path;
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
    fn handles_users_api_flow() {
        let state = UsersApiState::default();
        let routes = RuntimeRoutes {
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

        let health = runtime_response(&state, &routes, &functions, &Method::Get, "/health", "");
        assert_eq!(health.status, 200);
        assert_eq!(health.body["status"], "ok");

        let created = runtime_response(
            &state,
            &routes,
            &functions,
            &Method::Post,
            "/users",
            r#"{"email":"ada@example.com","name":"Ada"}"#,
        );
        assert_eq!(created.status, 201);

        let invalid = runtime_response(
            &state,
            &routes,
            &functions,
            &Method::Post,
            "/users",
            r#"{"email":"invalid","name":"Ada"}"#,
        );
        assert_eq!(invalid.status, 400);
        assert_eq!(invalid.body["error"], "invalid_email");

        let fetched = runtime_response(&state, &routes, &functions, &Method::Get, "/users/1", "");
        assert_eq!(fetched.status, 200);
        assert_eq!(fetched.body["email"], "ada@example.com");
    }

    #[test]
    fn handles_minimal_hello_api_flow() {
        let state = UsersApiState::default();
        let check = check_path(Path::new("../../examples/hello_api")).expect("project checks");
        assert!(check.ok(), "{:#?}", check.diagnostics);
        let service = find_single_service(&check.modules).expect("single service");
        let routes = RuntimeRoutes::from_service(service).expect("routes");
        let functions = RuntimeFunctions::from_modules(&check.modules);

        let ping = runtime_response(&state, &routes, &functions, &Method::Get, "/ping", "");
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
    ) -> super::RuntimeRoute {
        super::RuntimeRoute {
            method: method.to_string(),
            path: path.to_string(),
            body_type: body_type.map(str::to_string),
            response_type: response_type.map(str::to_string),
            status,
            handler: handler.to_string(),
        }
    }
}
