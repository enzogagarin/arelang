use are_ast::{FunctionDecl, Item, Module, RouteDecl, ServiceDecl};
use are_interpreter::{
    Host, InterpretError, Value as InterpretedValue, interpret_function_with_host_and_args,
    interpret_function_with_host_and_functions,
};
use are_project::{CheckResult, Manifest, ProjectError, check_path, load_manifest, project_root};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

#[derive(Debug)]
pub enum RuntimeError {
    Project(ProjectError),
    StaticChecks(CheckResult),
    UnsupportedProject(String),
    Server(String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project(err) => write!(f, "{err}"),
            Self::StaticChecks(check) => {
                writeln!(f, "static checks failed before runtime start")?;
                for diagnostic in &check.diagnostics {
                    writeln!(f, "{diagnostic}")?;
                }
                Ok(())
            }
            Self::UnsupportedProject(message) | Self::Server(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<ProjectError> for RuntimeError {
    fn from(value: ProjectError) -> Self {
        Self::Project(value)
    }
}

/// Run an Arelang HTTP project.
///
/// # Errors
///
/// Returns an error if static checks fail, the manifest cannot be loaded, the
/// project is not supported by the current HTTP MVP runtime, or the TCP server
/// cannot be started.
pub fn run_project(path: &Path) -> Result<(), RuntimeError> {
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
    let server_config = manifest.server.clone().ok_or_else(|| {
        RuntimeError::UnsupportedProject("server target requires [server] config".into())
    })?;
    let address = format!("{}:{}", server_config.host, server_config.port);
    let server = Server::http(&address)
        .map_err(|err| RuntimeError::Server(format!("failed to bind {address}: {err}")))?;

    println!(
        "Arelang HTTP MVP running {} v{} at http://{}",
        manifest.package.name, manifest.package.version, address
    );
    println!("Routes:");
    for route in &routes.routes {
        println!("  {} {}", route.method, route.path);
    }

    run_http_server(&server, &routes, &functions, &root, &manifest);
    Ok(())
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

#[derive(Debug, Clone)]
struct RuntimeRoutes {
    routes: Vec<RuntimeRoute>,
    error_mapper: Option<String>,
}

#[derive(Debug, Clone)]
struct RuntimeRoute {
    method: String,
    path: String,
    handler: String,
}

#[derive(Debug, Clone, Default)]
struct RuntimeFunctions {
    functions: HashMap<String, FunctionDecl>,
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

    fn handler_for(&self, method: &Method, path: &str) -> Option<(&str, HashMap<String, String>)> {
        self.routes.iter().find_map(|route| {
            if !method_matches(method, &route.method) {
                return None;
            }

            match_route(&route.path, path).map(|params| (route.handler.as_str(), params))
        })
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

        Self { functions }
    }

    fn get(&self, name: &str) -> Option<&FunctionDecl> {
        self.functions.get(name)
    }
}

fn runtime_route(route: &RouteDecl) -> RuntimeRoute {
    RuntimeRoute {
        method: route.method.clone(),
        path: route.path.clone(),
        handler: route.handler.segments.join("."),
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
    let Some((handler, params)) = routes.handler_for(method, path) else {
        return error_response(404, "not_found");
    };

    interpreted_response(
        state,
        functions,
        routes.error_mapper.as_deref(),
        handler,
        &params,
        body,
    )
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
    };

    match interpret_function_with_host_and_functions(function, &functions.functions, &mut host) {
        Ok(InterpretedValue::HttpResponse(response)) => RuntimeResponse {
            status: response.status,
            body: response.body,
        },
        Ok(InterpretedValue::Json(_)) => error_response(500, "handler_returned_json"),
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
}

impl Host for UsersApiHost<'_> {
    fn read_json_body(
        &mut self,
        type_name: Option<&str>,
    ) -> Result<serde_json::Value, InterpretError> {
        let value = serde_json::from_str::<serde_json::Value>(self.request_body)
            .map_err(|_| api_invalid_input("invalid_json"))?;

        if type_name == Some("CreateUserInput") && !is_create_user_input(&value) {
            return Err(api_invalid_input("invalid_json"));
        }

        Ok(value)
    }

    fn validate_email(&mut self, value: &serde_json::Value) -> Result<(), InterpretError> {
        let Some(email) = value.as_str() else {
            return Err(api_invalid_input("invalid_email"));
        };

        if email.contains('@') {
            return Ok(());
        }

        Err(api_invalid_input("invalid_email"))
    }

    fn validate_length(
        &mut self,
        value: &serde_json::Value,
        min: i64,
        max: i64,
    ) -> Result<(), InterpretError> {
        let Some(text) = value.as_str() else {
            return Err(api_invalid_input("invalid_name"));
        };

        let len = i64::try_from(text.chars().count()).map_err(|_| {
            InterpretError::UnsupportedExpression("validate.length input is too large".into())
        })?;
        if (min..=max).contains(&len) {
            return Ok(());
        }

        Err(api_invalid_input("invalid_name"))
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
            return Err(api_invalid_input("missing_id"));
        };

        if type_name == Some("UserId") {
            let id = value
                .parse::<u64>()
                .map_err(|_| api_invalid_input("invalid_id"))?;
            return Ok(serde_json::json!(id));
        }

        Ok(serde_json::Value::String(value.clone()))
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

fn is_create_user_input(value: &serde_json::Value) -> bool {
    value.get("email").is_some_and(serde_json::Value::is_string)
        && value.get("name").is_some_and(serde_json::Value::is_string)
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
        if let Some(name) = pattern_part.strip_prefix(':') {
            params.insert(name.to_string(), path_part.to_string());
        } else if pattern_part != &path_part {
            return None;
        }
    }

    Some(params)
}

#[cfg(test)]
mod tests {
    use super::{
        RuntimeFunctions, RuntimeRoutes, UsersApiState, find_single_service, match_route,
        runtime_response,
    };
    use are_project::check_path;
    use std::path::Path;
    use tiny_http::Method;

    #[test]
    fn matches_named_route_params() {
        let params = match_route("/users/:id", "/users/42").expect("route matches");
        assert_eq!(params.get("id").expect("id"), "42");
    }

    #[test]
    fn handles_users_api_flow() {
        let state = UsersApiState::default();
        let routes = RuntimeRoutes {
            routes: vec![
                route("GET", "/health", "health"),
                route("POST", "/users", "create_user"),
                route("GET", "/users/:id", "get_user"),
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

    fn users_api_functions() -> RuntimeFunctions {
        let check = check_path(Path::new("../../examples/users_api")).expect("project checks");
        assert!(check.ok(), "{:#?}", check.diagnostics);
        RuntimeFunctions::from_modules(&check.modules)
    }

    fn route(method: &str, path: &str, handler: &str) -> super::RuntimeRoute {
        super::RuntimeRoute {
            method: method.to_string(),
            path: path.to_string(),
            handler: handler.to_string(),
        }
    }
}
