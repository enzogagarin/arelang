use crate::RuntimeError;
use are_ast::{Item, Module, RouteDecl, ServiceDecl, TypeExpr};
use serde::Serialize;
use std::collections::HashMap;
use tiny_http::Method;

#[derive(Debug, Clone, Serialize)]
pub struct HttpContractManifest {
    pub service: String,
    pub routes: Vec<HttpRouteContract>,
    pub error_mapper: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HttpRouteContract {
    pub method: String,
    pub path: String,
    pub body_type: Option<String>,
    pub response_type: Option<String>,
    pub status: Option<u16>,
    pub path_params: Vec<HttpPathParam>,
    pub handler: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HttpPathParam {
    pub name: String,
    pub ty: Option<String>,
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

impl HttpContractManifest {
    pub(crate) fn from_service(service: &ServiceDecl) -> Result<Self, RuntimeError> {
        let routes = service.routes.iter().map(runtime_route).collect::<Vec<_>>();
        let error_mapper = runtime_error_mapper(service);

        if routes.is_empty() {
            return Err(RuntimeError::UnsupportedProject(format!(
                "service `{}` must declare at least one route",
                service.name
            )));
        }

        Ok(Self {
            service: service.name.clone(),
            routes,
            error_mapper,
        })
    }

    pub(crate) fn route_for(
        &self,
        method: &Method,
        path: &str,
    ) -> Option<(&HttpRouteContract, HashMap<String, String>)> {
        self.routes.iter().find_map(|route| {
            if !method_matches(method, &route.method) {
                return None;
            }

            match_route(&route.path, path).map(|params| (route, params))
        })
    }

    pub(crate) fn has(&self, method: &str, path: &str) -> bool {
        self.routes
            .iter()
            .any(|route| route.method == method && route.path == path)
    }

    pub(crate) fn test_routes(&self) -> Vec<TestRoute> {
        self.routes
            .iter()
            .map(|route| TestRoute {
                method: route.method.clone(),
                path: route.path.clone(),
                body_type: route.body_type.clone(),
                response_type: route.response_type.clone(),
                status: route.status,
                path_params: route
                    .path_params
                    .iter()
                    .map(|param| TestPathParam {
                        name: param.name.clone(),
                        ty: param.ty.clone(),
                    })
                    .collect(),
                handler: route.handler.clone(),
            })
            .collect()
    }
}

pub(crate) fn find_single_service(
    modules: &[are_project::CheckedFile],
) -> Result<&ServiceDecl, RuntimeError> {
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

pub(crate) fn route_summary_line(route: &HttpRouteContract) -> String {
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

pub(crate) fn match_route(pattern: &str, path: &str) -> Option<HashMap<String, String>> {
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

fn services_in_module(module: &Module) -> impl Iterator<Item = &ServiceDecl> {
    module.items.iter().filter_map(|item| {
        if let Item::Service(service) = item {
            Some(service)
        } else {
            None
        }
    })
}

fn runtime_route(route: &RouteDecl) -> HttpRouteContract {
    HttpRouteContract {
        method: route.method.clone(),
        path: route.path.clone(),
        body_type: route.body_type.as_ref().map(type_expr_name),
        response_type: route.response_type.as_ref().map(type_expr_name),
        status: route.status.map(|status| status.value),
        path_params: path_params_from_path(&route.path),
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

fn path_params_from_path(path: &str) -> Vec<HttpPathParam> {
    path.split('/')
        .filter_map(route_param_from_segment)
        .collect()
}

fn route_param_from_segment(segment: &str) -> Option<HttpPathParam> {
    if let Some(name) = segment.strip_prefix(':') {
        return Some(HttpPathParam {
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

    Some(HttpPathParam {
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
