use are_ast::{FunctionDecl, Item, Module, RouteDecl, ServiceDecl};
use are_interpreter::{Value as InterpretedValue, interpret_function};
use are_project::{CheckResult, Manifest, ProjectError, check_path, load_manifest, project_root};
use serde::{Deserialize, Serialize};
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

    run_users_api_server(&server, &routes, &functions, &root, &manifest);
    Ok(())
}

fn run_users_api_server(
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

    let response = users_api_response(state, routes, functions, &method, &url, &body);
    request
        .respond(json_response(response.status, &response.body))
        .map_err(|err| RuntimeError::Server(format!("failed to write response: {err}")))
}

#[derive(Debug, Clone)]
struct RuntimeRoutes {
    routes: Vec<RuntimeRoute>,
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

        for required in [
            ("GET", "/health", "health"),
            ("POST", "/users", "create_user"),
            ("GET", "/users/:id", "get_user"),
        ] {
            if !routes.iter().any(|route| route.matches(required)) {
                return Err(RuntimeError::UnsupportedProject(format!(
                    "HTTP MVP runtime currently requires route `{} {} -> {}`",
                    required.0, required.1, required.2
                )));
            }
        }

        Ok(Self { routes })
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

impl RuntimeRoute {
    fn matches(&self, expected: (&str, &str, &str)) -> bool {
        self.method == expected.0 && self.path == expected.1 && self.handler == expected.2
    }
}

fn runtime_route(route: &RouteDecl) -> RuntimeRoute {
    RuntimeRoute {
        method: route.method.clone(),
        path: route.path.clone(),
        handler: route.handler.segments.join("."),
    }
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

#[derive(Debug, Deserialize)]
struct CreateUserInput {
    email: String,
    name: String,
}

#[derive(Debug)]
struct RuntimeResponse {
    status: u16,
    body: serde_json::Value,
}

fn users_api_response(
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

    match handler {
        "health" => interpreted_response(functions, handler),
        "create_user" => create_user(state, body),
        "get_user" => get_user(state, &params),
        _ => error_response(500, "unsupported_handler"),
    }
}

fn interpreted_response(functions: &RuntimeFunctions, handler: &str) -> RuntimeResponse {
    let Some(function) = functions.get(handler) else {
        return error_response(500, "handler_not_found");
    };

    match interpret_function(function) {
        Ok(InterpretedValue::HttpResponse(response)) => RuntimeResponse {
            status: response.status,
            body: response.body,
        },
        Ok(InterpretedValue::Json(_)) => error_response(500, "handler_returned_json"),
        Err(err) => {
            eprintln!("Arelang interpreter failed in `{handler}`: {err}");
            error_response(500, "interpreter_error")
        }
    }
}

fn create_user(state: &UsersApiState, body: &str) -> RuntimeResponse {
    let Ok(input) = serde_json::from_str::<CreateUserInput>(body) else {
        return error_response(400, "invalid_json");
    };

    if !input.email.contains('@') {
        return error_response(400, "invalid_email");
    }

    let name_len = input.name.chars().count();
    if !(2..=80).contains(&name_len) {
        return error_response(400, "invalid_name");
    }

    let mut inner = state.inner.lock().expect("users api state lock poisoned");
    inner.next_id += 1;
    let user = User {
        id: inner.next_id,
        email: input.email,
        name: input.name,
    };
    inner.users.insert(user.id, user.clone());

    RuntimeResponse {
        status: 201,
        body: serde_json::to_value(user).expect("user serializes"),
    }
}

fn get_user(state: &UsersApiState, params: &HashMap<String, String>) -> RuntimeResponse {
    let Some(raw_id) = params.get("id") else {
        return error_response(400, "missing_id");
    };

    let Ok(id) = raw_id.parse::<u64>() else {
        return error_response(400, "invalid_id");
    };

    let inner = state.inner.lock().expect("users api state lock poisoned");
    let Some(user) = inner.users.get(&id) else {
        return error_response(404, "not_found");
    };

    RuntimeResponse {
        status: 200,
        body: serde_json::to_value(user).expect("user serializes"),
    }
}

fn error_response(status: u16, error: &str) -> RuntimeResponse {
    RuntimeResponse {
        status,
        body: serde_json::json!({ "error": error }),
    }
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
    use super::{RuntimeFunctions, RuntimeRoutes, UsersApiState, match_route, users_api_response};
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
        };
        let functions = users_api_functions();

        let health = users_api_response(&state, &routes, &functions, &Method::Get, "/health", "");
        assert_eq!(health.status, 200);
        assert_eq!(health.body["status"], "ok");

        let created = users_api_response(
            &state,
            &routes,
            &functions,
            &Method::Post,
            "/users",
            r#"{"email":"ada@example.com","name":"Ada"}"#,
        );
        assert_eq!(created.status, 201);

        let fetched = users_api_response(&state, &routes, &functions, &Method::Get, "/users/1", "");
        assert_eq!(fetched.status, 200);
        assert_eq!(fetched.body["email"], "ada@example.com");
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
