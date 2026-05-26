use crate::RuntimeError;
use crate::contracts::{HttpContractManifest, route_summary_line};
use crate::functions::RuntimeFunctions;
use crate::request::RuntimeRequest;
use crate::response::runtime_response;
use crate::store::RuntimeState;
use are_project::Manifest;
use std::path::Path;
use tiny_http::{Header, Request, Response, Server, StatusCode};

pub(crate) fn print_run_summary(
    manifest: &Manifest,
    contracts: &HttpContractManifest,
    address: &str,
) {
    println!("{} running at http://{address}", contracts.service);
    println!(
        "package {} v{}",
        manifest.package.name, manifest.package.version
    );
    println!("routes:");
    for route in &contracts.routes {
        println!("{}", route_summary_line(route));
    }
}

pub(crate) fn run_http_server(
    server: &Server,
    contracts: &HttpContractManifest,
    functions: &RuntimeFunctions,
    root: &Path,
    manifest: &Manifest,
) {
    let state = RuntimeState::default();

    for request in server.incoming_requests() {
        let result = handle_tiny_request(request, &state, contracts, functions);
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
    state: &RuntimeState,
    contracts: &HttpContractManifest,
    functions: &RuntimeFunctions,
) -> Result<(), RuntimeError> {
    let method = request.method().clone();
    let url = request.url().to_string();
    let mut body = String::new();
    request
        .as_reader()
        .read_to_string(&mut body)
        .map_err(|err| RuntimeError::Server(format!("failed to read request body: {err}")))?;

    let runtime_request = RuntimeRequest::new(method, url, body);
    let response = runtime_response(state, contracts, functions, &runtime_request);
    request
        .respond(json_response(response.status, &response.body))
        .map_err(|err| RuntimeError::Server(format!("failed to write response: {err}")))
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
