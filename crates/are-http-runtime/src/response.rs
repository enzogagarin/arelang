use crate::contracts::{HttpContractManifest, HttpRouteContract};
use crate::functions::RuntimeFunctions;
use crate::host::RuntimeHost;
use crate::request::RuntimeRequest;
use crate::store::RuntimeState;
use are_ast::{FunctionDecl, Param, TypeExpr};
use are_interpreter::{
    Host, InterpretError, Value as InterpretedValue, interpret_function_with_host_and_args,
    interpret_function_with_host_and_functions,
};
use std::collections::HashMap;

#[derive(Debug)]
pub(crate) struct RuntimeResponse {
    pub(crate) status: u16,
    pub(crate) body: serde_json::Value,
}

struct HandlerRequest<'a> {
    route: &'a HttpRouteContract,
    params: &'a HashMap<String, String>,
    body: &'a str,
    query: &'a str,
    headers: &'a HashMap<String, String>,
}

pub(crate) fn runtime_response(
    state: &RuntimeState,
    contracts: &HttpContractManifest,
    functions: &RuntimeFunctions,
    request: &RuntimeRequest,
) -> RuntimeResponse {
    let Some((route, params)) = contracts.route_for(&request.method, request.path()) else {
        return error_response(404, "not_found");
    };

    if let Some(body_type) = &route.body_type
        && let Err(code) = functions
            .schemas
            .validate_json_body(body_type, &request.body)
    {
        return error_response(400, &code);
    }

    let response = interpreted_response(
        state,
        functions,
        contracts,
        &HandlerRequest {
            route,
            params: &params,
            body: &request.body,
            query: request.query(),
            headers: &request.headers,
        },
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
    contracts: &HttpContractManifest,
    request: &HandlerRequest<'_>,
) -> RuntimeResponse {
    let route = request.route;
    let handler = route.handler.as_str();
    let Some(function) = functions.get(handler) else {
        return error_response(500, "handler_not_found");
    };
    let mut host = RuntimeHost {
        state,
        params: request.params,
        request_body: request.body,
        query: request.query,
        headers: request.headers,
        schemas: &functions.schemas,
    };

    let result = handler_args(function, route, request, &mut host).and_then(|args| {
        if args.is_empty() {
            interpret_function_with_host_and_functions(function, &functions.functions, &mut host)
        } else {
            interpret_function_with_host_and_args(function, &functions.functions, &mut host, args)
        }
    });

    match result {
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
                return mapped_error_response(functions, contracts, &mut host, error.clone());
            }

            eprintln!("Arelang interpreter failed in `{handler}`: {err}");
            error_response(500, "interpreter_error")
        }
    }
}

fn handler_args(
    function: &FunctionDecl,
    route: &HttpRouteContract,
    request: &HandlerRequest<'_>,
    host: &mut RuntimeHost<'_>,
) -> Result<Vec<InterpretedValue>, InterpretError> {
    if function.params.is_empty() {
        return Ok(Vec::new());
    }

    function
        .params
        .iter()
        .enumerate()
        .map(|(index, param)| handler_arg(index, param, route, request, host))
        .collect()
}

fn handler_arg(
    index: usize,
    param: &Param,
    route: &HttpRouteContract,
    request: &HandlerRequest<'_>,
    host: &mut RuntimeHost<'_>,
) -> Result<InterpretedValue, InterpretError> {
    if index == 0 || is_http_request_type(&param.ty) {
        return Ok(InterpretedValue::Unit);
    }

    let param_type = type_expr_name(&param.ty);
    if request.params.contains_key(&param.name) {
        return host
            .read_path_param(Some(&param_type), &param.name)
            .map(InterpretedValue::Json);
    }

    if route.body_type.as_deref() == Some(param_type.as_str()) {
        return host
            .read_json_body(Some(&param_type))
            .map(InterpretedValue::Json);
    }

    if route.query_type.as_deref() == Some(param_type.as_str()) {
        return host
            .read_query_params(Some(&param_type))
            .map(InterpretedValue::Json);
    }

    if route.headers_type.as_deref() == Some(param_type.as_str()) {
        return host
            .read_headers(Some(&param_type))
            .map(InterpretedValue::Json);
    }

    if route.cookies_type.as_deref() == Some(param_type.as_str()) {
        return host
            .read_cookies(Some(&param_type))
            .map(InterpretedValue::Json);
    }

    Err(InterpretError::UnsupportedExpression(format!(
        "handler parameter `{}`",
        param.name
    )))
}

fn is_http_request_type(ty: &TypeExpr) -> bool {
    matches!(
        ty,
        TypeExpr::Path { path }
            if path.segments.len() == 2
                && path.segments.get(1).is_some_and(|segment| segment == "Request")
    )
}

fn mapped_error_response(
    functions: &RuntimeFunctions,
    contracts: &HttpContractManifest,
    host: &mut RuntimeHost<'_>,
    error: are_interpreter::EnumValue,
) -> RuntimeResponse {
    if let Some(error_contract) = &contracts.error_contract {
        return declarative_error_response(contracts, error_contract, &error);
    }

    let error_mapper = contracts.error_mapper.as_deref();
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

fn declarative_error_response(
    contracts: &HttpContractManifest,
    error_contract: &str,
    error: &are_interpreter::EnumValue,
) -> RuntimeResponse {
    if error.enum_name != error_contract {
        eprintln!(
            "Arelang application error {}.{} does not match service error contract {error_contract}",
            error.enum_name, error.variant
        );
        return error_response(500, "error_contract_mismatch");
    }

    let Some(enum_schema) = contracts
        .schemas
        .enums
        .iter()
        .find(|schema| schema.name == error_contract)
    else {
        eprintln!("Arelang error contract `{error_contract}` was not found at runtime");
        return error_response(500, "error_contract_missing");
    };

    let Some(variant) = enum_schema
        .variants
        .iter()
        .find(|variant| variant.name == error.variant)
    else {
        eprintln!(
            "Arelang error contract `{error_contract}` has no variant `{}`",
            error.variant
        );
        return error_response(500, "error_contract_missing");
    };

    let Some(status) = variant.status else {
        eprintln!(
            "Arelang error contract `{error_contract}.{}` has no status",
            variant.name
        );
        return error_response(500, "error_contract_missing");
    };

    RuntimeResponse {
        status,
        body: declarative_error_body(&variant.name, &variant.payload, &error.payload),
    }
}

fn declarative_error_body(
    variant_name: &str,
    fields: &[crate::contracts::HttpFieldSchema],
    payload: &[InterpretedValue],
) -> serde_json::Value {
    if fields.is_empty() {
        return serde_json::json!({ "error": error_code(variant_name) });
    }

    if fields.len() == 1
        && matches!(fields[0].name.as_str(), "message" | "error")
        && let Some(value) = payload.first()
    {
        return serde_json::json!({ "error": interpreted_value_to_json(value) });
    }

    let mut body = serde_json::Map::new();
    body.insert(
        "error".to_string(),
        serde_json::Value::String(error_code(variant_name)),
    );
    for (field, value) in fields.iter().zip(payload) {
        body.insert(field.name.clone(), interpreted_value_to_json(value));
    }

    serde_json::Value::Object(body)
}

fn interpreted_value_to_json(value: &InterpretedValue) -> serde_json::Value {
    match value {
        InterpretedValue::Json(value) => value.clone(),
        InterpretedValue::Bool(value) => serde_json::Value::Bool(*value),
        InterpretedValue::Enum(error) => serde_json::json!({
            "variant": error.variant,
        }),
        InterpretedValue::HttpResponse(response) => serde_json::json!({
            "status": response.status,
            "body": response.body,
        }),
        InterpretedValue::Unit => serde_json::Value::Null,
    }
}

fn error_code(variant_name: &str) -> String {
    let mut output = String::new();
    for (index, ch) in variant_name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                output.push('_');
            }
            output.push(ch.to_ascii_lowercase());
        } else {
            output.push(ch);
        }
    }
    output
}

fn type_expr_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Path { path } => path.segments.join("."),
        TypeExpr::Generic { base, args, .. } => {
            let args = args.iter().map(type_expr_name).collect::<Vec<_>>();
            format!("{}<{}>", base.segments.join("."), args.join(", "))
        }
        TypeExpr::Option { inner, .. } => format!("{}?", type_expr_name(inner)),
    }
}

pub(crate) fn error_response(status: u16, error: &str) -> RuntimeResponse {
    RuntimeResponse {
        status,
        body: serde_json::json!({ "error": error }),
    }
}
