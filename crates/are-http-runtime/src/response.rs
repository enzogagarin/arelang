use crate::contracts::{HttpContractManifest, HttpRouteContract};
use crate::functions::RuntimeFunctions;
use crate::host::RuntimeHost;
use crate::store::RuntimeState;
use are_interpreter::{
    Value as InterpretedValue, interpret_function_with_host_and_args,
    interpret_function_with_host_and_functions,
};
use std::collections::HashMap;
use tiny_http::Method;

#[derive(Debug)]
pub(crate) struct RuntimeResponse {
    pub(crate) status: u16,
    pub(crate) body: serde_json::Value,
}

pub(crate) fn runtime_response(
    state: &RuntimeState,
    contracts: &HttpContractManifest,
    functions: &RuntimeFunctions,
    method: &Method,
    url: &str,
    body: &str,
) -> RuntimeResponse {
    let path = strip_query(url);
    let Some((route, params)) = contracts.route_for(method, path) else {
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
        contracts.error_mapper.as_deref(),
        route,
        &route.handler,
        &params,
        body,
    );
    apply_route_response_contract(route, functions, response)
}

fn apply_route_response_contract(
    route: &HttpRouteContract,
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
    state: &RuntimeState,
    functions: &RuntimeFunctions,
    error_mapper: Option<&str>,
    route: &HttpRouteContract,
    handler: &str,
    params: &HashMap<String, String>,
    body: &str,
) -> RuntimeResponse {
    let Some(function) = functions.get(handler) else {
        return error_response(500, "handler_not_found");
    };
    let mut host = RuntimeHost {
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
        Ok(InterpretedValue::Json(body)) => RuntimeResponse {
            status: route.status.unwrap_or(200),
            body,
        },
        Ok(InterpretedValue::Bool(value)) => RuntimeResponse {
            status: route.status.unwrap_or(200),
            body: serde_json::Value::Bool(value),
        },
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
    host: &mut RuntimeHost<'_>,
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

pub(crate) fn error_response(status: u16, error: &str) -> RuntimeResponse {
    RuntimeResponse {
        status,
        body: serde_json::json!({ "error": error }),
    }
}

fn strip_query(url: &str) -> &str {
    url.split_once('?').map_or(url, |(path, _query)| path)
}
