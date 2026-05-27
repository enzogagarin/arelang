use super::{TypeChecker, is_identifier, path_is, path_name, same_type, type_name};
use crate::body::{BodyType, HttpResponseUse};
use are_ast::{
    CallArg, Expr, FunctionBody, FunctionDecl, Param, Path, RouteDecl, ServiceDecl, Stmt, TypeExpr,
};
use are_diagnostics::{Diagnostic, SourceRange};
use std::collections::HashMap;

impl TypeChecker<'_> {
    pub(super) fn check_routes(&mut self, decl: &ServiceDecl, state_type: &TypeExpr) {
        let mut result_error_types = Vec::new();

        for route in &decl.routes {
            let path_params = self.check_route_shape(route);
            self.check_route_body_contract(route);
            self.check_route_query_contract(route);
            self.check_route_headers_contract(route);
            self.check_route_cookies_contract(route);
            self.check_route_response_contract(route);

            let Some(handler) = route
                .handler
                .segments
                .first()
                .and_then(|name| self.functions.get(name))
                .copied()
            else {
                continue;
            };

            self.check_route_io_contract(route, handler, &path_params);
            self.check_route_handler_response_contract(route, handler);
            if let Some(error_type) =
                self.check_route_handler(route, handler, state_type, &path_params)
            {
                result_error_types.push(error_type);
            }
        }

        self.check_error_mapping(decl, &result_error_types);
    }

    fn check_route_shape(&mut self, route: &RouteDecl) -> Vec<RoutePathParam> {
        if !is_http_method(&route.method) {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0101",
                &self.file,
                route.range,
                format!("unsupported HTTP method `{}`", route.method),
                "supported methods are GET, POST, PUT, PATCH, DELETE, HEAD, and OPTIONS",
            ));
        }

        if !route.path.starts_with('/') {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0102",
                &self.file,
                route.range,
                format!("route path `{}` must start with `/`", route.path),
                "route paths are absolute within the service",
            ));
        }

        self.check_route_params(route)
    }

    fn check_route_params(&mut self, route: &RouteDecl) -> Vec<RoutePathParam> {
        let mut params = HashMap::new();
        let mut parsed = Vec::new();

        for segment in route.path.split('/') {
            let param = match route_param_from_segment(segment) {
                RouteSegmentParam::None => continue,
                RouteSegmentParam::Malformed(reason) => {
                    self.diagnostics.push(Diagnostic::error(
                        "E_HTTP_0105",
                        &self.file,
                        route.range,
                        format!("invalid route parameter segment `{segment}`"),
                        reason,
                    ));
                    continue;
                }
                RouteSegmentParam::Param(param) => param,
            };

            if param.name.is_empty() || !is_identifier(&param.name) {
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0103",
                    &self.file,
                    route.range,
                    format!("invalid route parameter `{}`", param.name),
                    "route parameters must use identifier syntax, such as `id`",
                ));
                continue;
            }

            if let Some(type_name) = &param.ty
                && !self.is_known_contract_type(type_name)
            {
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0106",
                    &self.file,
                    route.range,
                    format!("unknown route parameter type `{type_name}`"),
                    "typed route parameters must use a builtin or local type",
                ));
            }

            if params.insert(param.name.clone(), route.range).is_some() {
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0104",
                    &self.file,
                    route.range,
                    format!("duplicate route parameter `{}`", param.name),
                    "each route parameter name must be unique within a path",
                ));
            }

            parsed.push(param);
        }

        parsed
    }

    fn check_route_body_contract(&mut self, route: &RouteDecl) {
        let Some(body_type) = &route.body_type else {
            return;
        };

        if !route_method_allows_body(&route.method) {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0410",
                &self.file,
                body_type.range(),
                format!("{} routes cannot declare a request body", route.method),
                "body contracts are currently accepted for POST, PUT, and PATCH routes",
            ));
        }

        if !self.is_local_payload_type(body_type) {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0411",
                &self.file,
                body_type.range(),
                format!(
                    "route body `{}` is not a local payload type",
                    type_name(body_type)
                ),
                "body contracts should name a local struct or model such as `CreateUserInput`",
            ));
        }
    }

    fn check_route_query_contract(&mut self, route: &RouteDecl) {
        let Some(query_type) = &route.query_type else {
            return;
        };

        if !self.is_local_payload_type(query_type) {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0430",
                &self.file,
                query_type.range(),
                format!(
                    "route query `{}` is not a local payload type",
                    type_name(query_type)
                ),
                "query contracts should name a local struct such as `SearchUsersQuery`",
            ));
        }
    }

    fn check_route_headers_contract(&mut self, route: &RouteDecl) {
        let Some(headers_type) = &route.headers_type else {
            return;
        };

        if !self.is_local_payload_type(headers_type) {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0440",
                &self.file,
                headers_type.range(),
                format!(
                    "route headers `{}` is not a local payload type",
                    type_name(headers_type)
                ),
                "headers contracts should name a local struct such as `AuthHeaders`",
            ));
        }
    }

    fn check_route_cookies_contract(&mut self, route: &RouteDecl) {
        let Some(cookies_type) = &route.cookies_type else {
            return;
        };

        if !self.is_local_payload_type(cookies_type) {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0450",
                &self.file,
                cookies_type.range(),
                format!(
                    "route cookies `{}` is not a local payload type",
                    type_name(cookies_type)
                ),
                "cookies contracts should name a local struct such as `SessionCookies`",
            ));
        }
    }

    fn check_route_response_contract(&mut self, route: &RouteDecl) {
        if let Some(response_type) = &route.response_type
            && !self.is_response_payload_type(response_type)
        {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0420",
                &self.file,
                response_type.range(),
                format!(
                    "route response `{}` is not a response payload type",
                    type_name(response_type)
                ),
                "response contracts should name a local struct, model, type alias, or primitive payload",
            ));
        }

        if let Some(status) = route.status
            && !(100..=599).contains(&status.value)
        {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0421",
                &self.file,
                status.range,
                format!("invalid HTTP status code `{}`", status.value),
                "status contracts must use an integer status code from 100 to 599",
            ));
        }
    }

    fn check_route_io_contract(
        &mut self,
        route: &RouteDecl,
        handler: &FunctionDecl,
        path_params: &[RoutePathParam],
    ) {
        let mut uses = collect_handler_io_uses(handler);
        self.collect_handler_param_bindings(route, handler, path_params, &mut uses);
        self.check_route_path_param_contract(route, handler, path_params, &uses);
        self.check_route_body_decode_contract(route, handler, &uses);
        self.check_route_query_decode_contract(route, handler, &uses);
        self.check_route_headers_decode_contract(route, handler, &uses);
        self.check_route_cookies_decode_contract(route, handler, &uses);
    }

    fn collect_handler_param_bindings(
        &self,
        route: &RouteDecl,
        handler: &FunctionDecl,
        path_params: &[RoutePathParam],
        uses: &mut HandlerIoUses,
    ) {
        for param in handler.params.iter().skip(1) {
            match self.handler_param_source(route, path_params, param) {
                HandlerParamSource::Path(name) => uses.path_params.push(PathParamUse {
                    name: Some(name),
                    ty: Some(type_name(&param.ty)),
                    range: param.range,
                }),
                HandlerParamSource::Body => uses.request_bodies.push(RequestBodyUse {
                    ty: Some(type_name(&param.ty)),
                    range: param.range,
                }),
                HandlerParamSource::Query => uses.request_queries.push(RequestQueryUse {
                    ty: Some(type_name(&param.ty)),
                    range: param.range,
                }),
                HandlerParamSource::Headers => uses.request_headers.push(RequestHeadersUse {
                    ty: Some(type_name(&param.ty)),
                    range: param.range,
                }),
                HandlerParamSource::Cookies => uses.request_cookies.push(RequestCookiesUse {
                    ty: Some(type_name(&param.ty)),
                    range: param.range,
                }),
                HandlerParamSource::Request
                | HandlerParamSource::Unknown
                | HandlerParamSource::Ambiguous(_) => {}
            }
        }
    }

    fn check_route_path_param_contract(
        &mut self,
        route: &RouteDecl,
        handler: &FunctionDecl,
        path_params: &[RoutePathParam],
        uses: &HandlerIoUses,
    ) {
        for param_use in &uses.path_params {
            let Some(name) = &param_use.name else {
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0404",
                    &self.file,
                    param_use.range,
                    "ctx.param requires a literal route parameter name",
                    "write route parameter reads as `ctx.param<UserId>(\"id\")` so the compiler can check the route contract",
                ));
                continue;
            };

            let Some(route_param) = path_params.iter().find(|param| param.name == *name) else {
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0403",
                    &self.file,
                    param_use.range,
                    format!(
                        "handler `{}` reads unknown route parameter `{name}`",
                        handler.name
                    ),
                    "the route path must declare the parameter before the handler can read it",
                ));
                continue;
            };

            if let Some(expected) = &route_param.ty {
                match &param_use.ty {
                    Some(actual) if actual == expected => {}
                    Some(actual) => self.diagnostics.push(Diagnostic::error(
                        "E_HTTP_0402",
                        &self.file,
                        param_use.range,
                        format!("route parameter `{name}` is `{expected}` but handler reads `{actual}`"),
                        "the route path type and ctx.param<T> type must match",
                    )),
                    None => self.diagnostics.push(Diagnostic::error(
                        "E_HTTP_0402",
                        &self.file,
                        param_use.range,
                        format!("route parameter `{name}` is `{expected}` but handler does not declare a read type"),
                        "read typed route parameters with `ctx.param<T>(\"name\")`",
                    )),
                }
            }
        }

        for route_param in path_params.iter().filter(|param| param.ty.is_some()) {
            if uses
                .path_params
                .iter()
                .any(|param_use| param_use.name.as_deref() == Some(route_param.name.as_str()))
            {
                continue;
            }

            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0401",
                &self.file,
                route.range,
                format!(
                    "typed route parameter `{}` is not read by handler `{}`",
                    route_param.name, handler.name
                ),
                "typed path parameters are part of the handler contract and should be consumed with ctx.param<T>",
            ));
        }
    }

    fn check_route_body_decode_contract(
        &mut self,
        route: &RouteDecl,
        handler: &FunctionDecl,
        uses: &HandlerIoUses,
    ) {
        if let Some(body_type) = &route.body_type {
            let expected = type_name(body_type);
            if uses.request_bodies.is_empty() {
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0412",
                    &self.file,
                    route.range,
                    format!(
                        "route body `{expected}` is not decoded by handler `{}`",
                        handler.name
                    ),
                    "decode the declared body with `req.json<T>()` inside the route handler",
                ));
                return;
            }

            if !uses
                .request_bodies
                .iter()
                .any(|body_use| body_use.ty.as_deref() == Some(expected.as_str()))
            {
                let actual = uses
                    .request_bodies
                    .iter()
                    .filter_map(|body_use| body_use.ty.as_deref())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0413",
                    &self.file,
                    route.range,
                    format!("route body `{expected}` does not match handler decode `{actual}`"),
                    "the service route body contract and req.json<T>() type must match",
                ));
            }
        } else if let Some(body_use) = uses.request_bodies.first() {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0414",
                &self.file,
                body_use.range,
                format!(
                    "handler `{}` decodes JSON without a route body contract",
                    handler.name
                ),
                "declare the route body as `post \"/path\" body Payload -> handler`",
            ));
        }
    }

    fn check_route_query_decode_contract(
        &mut self,
        route: &RouteDecl,
        handler: &FunctionDecl,
        uses: &HandlerIoUses,
    ) {
        if let Some(query_type) = &route.query_type {
            let expected = type_name(query_type);
            if uses.request_queries.is_empty() {
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0431",
                    &self.file,
                    route.range,
                    format!(
                        "route query `{expected}` is not decoded by handler `{}`",
                        handler.name
                    ),
                    "decode the declared query with `req.query<T>()` inside the route handler",
                ));
                return;
            }

            if !uses
                .request_queries
                .iter()
                .any(|query_use| query_use.ty.as_deref() == Some(expected.as_str()))
            {
                let actual = uses
                    .request_queries
                    .iter()
                    .filter_map(|query_use| query_use.ty.as_deref())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0432",
                    &self.file,
                    route.range,
                    format!("route query `{expected}` does not match handler decode `{actual}`"),
                    "the service route query contract and req.query<T>() type must match",
                ));
            }
        } else if let Some(query_use) = uses.request_queries.first() {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0433",
                &self.file,
                query_use.range,
                format!(
                    "handler `{}` decodes query params without a route query contract",
                    handler.name
                ),
                "declare the route query as `get \"/path\" query QueryPayload -> handler`",
            ));
        }
    }

    fn check_route_headers_decode_contract(
        &mut self,
        route: &RouteDecl,
        handler: &FunctionDecl,
        uses: &HandlerIoUses,
    ) {
        if let Some(headers_type) = &route.headers_type {
            let expected = type_name(headers_type);
            if uses.request_headers.is_empty() {
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0441",
                    &self.file,
                    route.range,
                    format!(
                        "route headers `{expected}` is not decoded by handler `{}`",
                        handler.name
                    ),
                    "decode the declared headers with `req.headers<T>()` inside the route handler",
                ));
                return;
            }

            if !uses
                .request_headers
                .iter()
                .any(|headers_use| headers_use.ty.as_deref() == Some(expected.as_str()))
            {
                let actual = uses
                    .request_headers
                    .iter()
                    .filter_map(|headers_use| headers_use.ty.as_deref())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0442",
                    &self.file,
                    route.range,
                    format!("route headers `{expected}` does not match handler decode `{actual}`"),
                    "the service route headers contract and req.headers<T>() type must match",
                ));
            }
        } else if let Some(headers_use) = uses.request_headers.first() {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0443",
                &self.file,
                headers_use.range,
                format!(
                    "handler `{}` decodes headers without a route headers contract",
                    handler.name
                ),
                "declare the route headers as `get \"/path\" headers HeaderPayload -> handler`",
            ));
        }
    }

    fn check_route_cookies_decode_contract(
        &mut self,
        route: &RouteDecl,
        handler: &FunctionDecl,
        uses: &HandlerIoUses,
    ) {
        if let Some(cookies_type) = &route.cookies_type {
            let expected = type_name(cookies_type);
            if uses.request_cookies.is_empty() {
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0451",
                    &self.file,
                    route.range,
                    format!(
                        "route cookies `{expected}` is not decoded by handler `{}`",
                        handler.name
                    ),
                    "decode the declared cookies with `req.cookies<T>()` inside the route handler",
                ));
                return;
            }

            if !uses
                .request_cookies
                .iter()
                .any(|cookies_use| cookies_use.ty.as_deref() == Some(expected.as_str()))
            {
                let actual = uses
                    .request_cookies
                    .iter()
                    .filter_map(|cookies_use| cookies_use.ty.as_deref())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0452",
                    &self.file,
                    route.range,
                    format!("route cookies `{expected}` does not match handler decode `{actual}`"),
                    "the service route cookies contract and req.cookies<T>() type must match",
                ));
            }
        } else if let Some(cookies_use) = uses.request_cookies.first() {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0453",
                &self.file,
                cookies_use.range,
                format!(
                    "handler `{}` decodes cookies without a route cookies contract",
                    handler.name
                ),
                "declare the route cookies as `get \"/path\" cookies CookiePayload -> handler`",
            ));
        }
    }

    fn check_route_handler_response_contract(&mut self, route: &RouteDecl, handler: &FunctionDecl) {
        if route.response_type.is_none() && route.status.is_none() {
            return;
        }

        let Some(response_uses) = self.http_responses.get(&handler.name).cloned() else {
            return;
        };
        for response_use in response_uses
            .iter()
            .filter(|response_use| response_use.success)
        {
            self.check_route_handler_response_type(route, handler, response_use);
            self.check_route_handler_status(route, handler, response_use);
        }
    }

    fn check_route_handler_response_type(
        &mut self,
        route: &RouteDecl,
        handler: &FunctionDecl,
        response_use: &HttpResponseUse,
    ) {
        let Some(response_type) = &route.response_type else {
            return;
        };

        if self.response_body_accepts(response_type, &response_use.body_type) {
            return;
        }

        self.diagnostics.push(Diagnostic::error(
            "E_HTTP_0422",
            &self.file,
            response_use.range,
            format!(
                "route response `{}` does not match handler `{}` response body `{}`",
                type_name(response_type),
                handler.name,
                response_use.body_type.display()
            ),
            "the route `returns` contract should match the value passed to Http.Response.ok/created",
        ));
    }

    fn check_route_handler_status(
        &mut self,
        route: &RouteDecl,
        handler: &FunctionDecl,
        response_use: &HttpResponseUse,
    ) {
        let Some(expected_status) = route.status.map(|status| status.value) else {
            return;
        };
        let Some(actual_status) = response_use.status else {
            return;
        };
        if actual_status == expected_status {
            return;
        }

        self.diagnostics.push(Diagnostic::error(
            "E_HTTP_0423",
            &self.file,
            response_use.range,
            format!(
                "route declares status {expected_status} but handler `{}` returns {actual_status}",
                handler.name
            ),
            "route status contracts should match the success response constructor used by the handler",
        ));
    }

    fn is_known_contract_type(&self, name: &str) -> bool {
        is_builtin_type_name(name)
            || self.types.contains_key(name)
            || self.structs.contains_key(name)
            || self.models.contains_key(name)
            || self.enums.contains_key(name)
    }

    fn is_local_payload_type(&self, ty: &TypeExpr) -> bool {
        let TypeExpr::Path { path } = ty else {
            return false;
        };

        path.segments.len() == 1
            && (self.structs.contains_key(&path.segments[0])
                || self.models.contains_key(&path.segments[0]))
    }

    fn is_response_payload_type(&self, ty: &TypeExpr) -> bool {
        let TypeExpr::Path { path } = ty else {
            return false;
        };

        path.segments.len() == 1 && self.is_known_response_payload_name(&path.segments[0])
    }

    fn is_known_response_payload_name(&self, name: &str) -> bool {
        is_builtin_type_name(name)
            || self.types.contains_key(name)
            || self.structs.contains_key(name)
            || self.models.contains_key(name)
    }

    fn response_body_accepts(&self, expected: &TypeExpr, actual: &BodyType) -> bool {
        if matches!(actual, BodyType::Unknown) {
            return true;
        }

        let expected_name = type_name(expected);
        match actual {
            BodyType::Named(actual_name) => actual_name == &expected_name,
            BodyType::Object => self.structs.contains_key(&expected_name),
            BodyType::String => expected_name == "String",
            BodyType::Bool => expected_name == "Bool",
            BodyType::Integer => {
                matches!(
                    expected_name.as_str(),
                    "Integer" | "Int" | "I64" | "U64" | "F64"
                )
            }
            BodyType::Unit
            | BodyType::HttpRequest
            | BodyType::HttpContext(_)
            | BodyType::HttpResponse
            | BodyType::Result { .. } => false,
            BodyType::Unknown => true,
        }
    }

    fn check_route_handler(
        &mut self,
        route: &RouteDecl,
        handler: &FunctionDecl,
        state_type: &TypeExpr,
        path_params: &[RoutePathParam],
    ) -> Option<TypeExpr> {
        if handler.params.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0201",
                &self.file,
                handler.range,
                format!(
                    "route handler `{}` must accept at least one parameter",
                    handler.name
                ),
                "HTTP handlers start with `ctx: Http.Context<AppState>` and may then bind route contracts such as `input: CreateUserInput`",
            ));
            return None;
        }

        let ctx = &handler.params[0];
        if !self.is_http_context_of(&ctx.ty, state_type) {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0202",
                &self.file,
                ctx.ty.range(),
                format!(
                    "first parameter of `{}` must be Http.Context<{}>",
                    handler.name,
                    type_name(state_type)
                ),
                "route handlers receive service state through the typed HTTP context",
            ));
        }

        self.check_route_handler_params(route, handler, path_params);

        let Some(return_type) = &handler.return_type else {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0204",
                &self.file,
                handler.range,
                format!("route handler `{}` must declare a return type", handler.name),
                "handlers may return Http.Response, Result<Http.Response, ApiError>, or the route `returns` type",
            ));
            return None;
        };

        if self.is_http_path(return_type, "Response") {
            return None;
        }

        if let Some(error_type) = self.result_response_error(return_type) {
            return Some(error_type.clone());
        }

        if let Some(response_type) = &route.response_type {
            if same_type(return_type, response_type) {
                return None;
            }

            if let Some((ok_type, error_type)) = result_ok_error(return_type)
                && same_type(ok_type, response_type)
            {
                return Some(error_type.clone());
            }
        }

        self.diagnostics.push(Diagnostic::error(
            "E_HTTP_0204",
            &self.file,
            return_type.range(),
            format!(
                "route handler `{}` has invalid return type `{}`",
                handler.name,
                type_name(return_type)
            ),
            "handlers may return Http.Response, Result<Http.Response, ApiError>, or the declared route response type",
        ));

        None
    }

    fn check_route_handler_params(
        &mut self,
        route: &RouteDecl,
        handler: &FunctionDecl,
        path_params: &[RoutePathParam],
    ) {
        let mut bound_inputs = HashMap::new();
        let mut raw_request_count = 0usize;

        for param in handler.params.iter().skip(1) {
            match self.handler_param_source(route, path_params, param) {
                HandlerParamSource::Request => {
                    raw_request_count += 1;
                    if raw_request_count > 1 {
                        self.diagnostics.push(Diagnostic::error(
                            "E_HTTP_0206",
                            &self.file,
                            param.range,
                            format!(
                                "handler `{}` binds Http.Request more than once",
                                handler.name
                            ),
                            "keep a single raw request parameter, or prefer typed route contract parameters",
                        ));
                    }
                }
                HandlerParamSource::Path(name) => {
                    let source = format!("path `{name}`");
                    self.check_duplicate_handler_binding(
                        &mut bound_inputs,
                        param,
                        handler,
                        &source,
                    );
                }
                HandlerParamSource::Body => {
                    self.check_duplicate_handler_binding(&mut bound_inputs, param, handler, "body");
                }
                HandlerParamSource::Query => {
                    self.check_duplicate_handler_binding(
                        &mut bound_inputs,
                        param,
                        handler,
                        "query",
                    );
                }
                HandlerParamSource::Headers => self.check_duplicate_handler_binding(
                    &mut bound_inputs,
                    param,
                    handler,
                    "headers",
                ),
                HandlerParamSource::Cookies => self.check_duplicate_handler_binding(
                    &mut bound_inputs,
                    param,
                    handler,
                    "cookies",
                ),
                HandlerParamSource::Ambiguous(matches) => {
                    self.diagnostics.push(Diagnostic::error(
                        "E_HTTP_0205",
                        &self.file,
                        param.ty.range(),
                        format!(
                            "handler parameter `{}` matches multiple route contracts: {}",
                            param.name,
                            matches.join(", ")
                        ),
                        "use distinct payload types for route input contracts before binding them as handler parameters",
                    ));
                }
                HandlerParamSource::Unknown => {
                    self.diagnostics.push(Diagnostic::error(
                        "E_HTTP_0205",
                        &self.file,
                        param.ty.range(),
                        format!(
                            "handler parameter `{}` is not part of route `{}`",
                            param.name, route.path
                        ),
                        "bind a typed path/body/query/headers/cookies contract, or use `req: Http.Request` for raw request access",
                    ));
                }
            }
        }
    }

    fn check_duplicate_handler_binding(
        &mut self,
        bound_inputs: &mut HashMap<String, SourceRange>,
        param: &Param,
        handler: &FunctionDecl,
        source: &str,
    ) {
        if let Some(previous) = bound_inputs.insert(source.to_string(), param.range) {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0206",
                &self.file,
                param.range,
                format!(
                    "handler `{}` binds route input `{source}` more than once",
                    handler.name
                ),
                format!(
                    "the first binding for `{source}` starts at line {}",
                    previous.start.line
                ),
            ));
        }
    }

    fn handler_param_source(
        &self,
        route: &RouteDecl,
        path_params: &[RoutePathParam],
        param: &Param,
    ) -> HandlerParamSource {
        if self.is_http_path(&param.ty, "Request") {
            return HandlerParamSource::Request;
        }

        if path_params
            .iter()
            .any(|path_param| path_param.name == param.name)
        {
            return HandlerParamSource::Path(param.name.clone());
        }

        let mut matches = Vec::new();
        if let Some(body_type) = &route.body_type
            && same_type(&param.ty, body_type)
        {
            matches.push(HandlerParamSource::Body);
        }
        if let Some(query_type) = &route.query_type
            && same_type(&param.ty, query_type)
        {
            matches.push(HandlerParamSource::Query);
        }
        if let Some(headers_type) = &route.headers_type
            && same_type(&param.ty, headers_type)
        {
            matches.push(HandlerParamSource::Headers);
        }
        if let Some(cookies_type) = &route.cookies_type
            && same_type(&param.ty, cookies_type)
        {
            matches.push(HandlerParamSource::Cookies);
        }

        match matches.as_slice() {
            [] => HandlerParamSource::Unknown,
            [source] => source.clone(),
            _ => HandlerParamSource::Ambiguous(
                matches
                    .iter()
                    .map(HandlerParamSource::label)
                    .map(str::to_string)
                    .collect(),
            ),
        }
    }

    fn check_error_mapping(&mut self, decl: &ServiceDecl, result_error_types: &[TypeExpr]) {
        let Some(first_error) = result_error_types.first() else {
            return;
        };

        for error_type in result_error_types.iter().skip(1) {
            if !same_type(first_error, error_type) {
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0301",
                    &self.file,
                    error_type.range(),
                    "service routes use multiple error types",
                    "the HTTP MVP supports one service error family so a single mapper can convert it to Http.Response",
                ));
            }
        }

        let error_map_uses = self.error_map_uses(decl);
        if error_map_uses.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0302",
                &self.file,
                decl.range,
                format!("service `{}` needs an HTTP error mapper", decl.name),
                "routes returning Result<Payload, E> require `use Http.error_map(map_error)`",
            ));
            return;
        }

        if error_map_uses.len() > 1 {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0303",
                &self.file,
                decl.range,
                format!("service `{}` has multiple HTTP error mappers", decl.name),
                "the HTTP MVP allows one `Http.error_map` per service",
            ));
        }

        for service_use in error_map_uses {
            if service_use.args.len() != 1 {
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0304",
                    &self.file,
                    service_use.range,
                    "Http.error_map expects exactly one mapper function",
                    "use `Http.error_map(map_error)`",
                ));
                continue;
            }

            self.check_error_mapper(&service_use.args[0], first_error);
        }
    }

    fn check_error_mapper(&mut self, mapper_path: &Path, expected_error: &TypeExpr) {
        if mapper_path.segments.len() != 1 {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0305",
                &self.file,
                mapper_path.range,
                "HTTP error mapper must be a local function",
                "use a local function name such as `map_error`",
            ));
            return;
        }

        let mapper_name = &mapper_path.segments[0];
        let Some(mapper) = self.functions.get(mapper_name).copied() else {
            let mut diagnostic = Diagnostic::error(
                "E_HTTP_0305",
                &self.file,
                mapper_path.range,
                format!("unknown HTTP error mapper `{mapper_name}`"),
                "declare a mapper function before using it in the service",
            );
            if let Some(suggestion) = self.function_suggestion(mapper_name) {
                diagnostic =
                    diagnostic.with_fix(format!("did you mean `{suggestion}`?"), Some(suggestion));
            }
            self.diagnostics.push(diagnostic);
            return;
        };

        if mapper.params.len() != 1 {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0306",
                &self.file,
                mapper.range,
                format!("HTTP error mapper `{mapper_name}` must accept exactly one parameter"),
                "mapper signature should be `fn map_error(err: ApiError) -> Http.Response`",
            ));
            return;
        }

        let mapper_param = &mapper.params[0].ty;
        if !same_type(mapper_param, expected_error) {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0307",
                &self.file,
                mapper_param.range(),
                format!(
                    "HTTP error mapper `{mapper_name}` accepts `{}` but route errors use `{}`",
                    type_name(mapper_param),
                    type_name(expected_error)
                ),
                "mapper parameter type must match the service route Result error type",
            ));
        }

        if !mapper
            .return_type
            .as_ref()
            .is_some_and(|return_type| self.is_http_path(return_type, "Response"))
        {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0308",
                &self.file,
                mapper.range,
                format!("HTTP error mapper `{mapper_name}` must return Http.Response"),
                "mapper signature should be `fn map_error(err: ApiError) -> Http.Response`",
            ));
        }
    }

    fn error_map_uses<'a>(&self, decl: &'a ServiceDecl) -> Vec<&'a are_ast::ServiceUse> {
        decl.uses
            .iter()
            .filter(|service_use| self.path_is_http(&service_use.target, "error_map"))
            .collect()
    }

    fn result_response_error<'b>(&self, ty: &'b TypeExpr) -> Option<&'b TypeExpr> {
        let (ok_type, error_type) = result_ok_error(ty)?;
        self.is_http_path(ok_type, "Response").then_some(error_type)
    }

    fn is_http_context_of(&self, ty: &TypeExpr, state_type: &TypeExpr) -> bool {
        let TypeExpr::Generic { base, args, .. } = ty else {
            return false;
        };

        self.path_is_http(base, "Context") && args.len() == 1 && same_type(&args[0], state_type)
    }

    fn is_http_path(&self, ty: &TypeExpr, name: &str) -> bool {
        let TypeExpr::Path { path } = ty else {
            return false;
        };

        self.path_is_http(path, name)
    }

    pub(super) fn path_is_http(&self, path: &Path, name: &str) -> bool {
        path.segments.len() == 2
            && self.http_aliases.contains(&path.segments[0])
            && path.segments[1] == name
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoutePathParam {
    name: String,
    ty: Option<String>,
}

enum RouteSegmentParam {
    None,
    Param(RoutePathParam),
    Malformed(&'static str),
}

#[derive(Debug, Clone)]
enum HandlerParamSource {
    Request,
    Path(String),
    Body,
    Query,
    Headers,
    Cookies,
    Ambiguous(Vec<String>),
    Unknown,
}

impl HandlerParamSource {
    fn label(&self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Path(_) => "path",
            Self::Body => "body",
            Self::Query => "query",
            Self::Headers => "headers",
            Self::Cookies => "cookies",
            Self::Ambiguous(_) | Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
struct PathParamUse {
    name: Option<String>,
    ty: Option<String>,
    range: SourceRange,
}

#[derive(Debug, Clone)]
struct RequestBodyUse {
    ty: Option<String>,
    range: SourceRange,
}

#[derive(Debug, Clone)]
struct RequestQueryUse {
    ty: Option<String>,
    range: SourceRange,
}

#[derive(Debug, Clone)]
struct RequestHeadersUse {
    ty: Option<String>,
    range: SourceRange,
}

#[derive(Debug, Clone)]
struct RequestCookiesUse {
    ty: Option<String>,
    range: SourceRange,
}

#[derive(Debug, Default)]
struct HandlerIoUses {
    path_params: Vec<PathParamUse>,
    request_bodies: Vec<RequestBodyUse>,
    request_queries: Vec<RequestQueryUse>,
    request_headers: Vec<RequestHeadersUse>,
    request_cookies: Vec<RequestCookiesUse>,
}

fn collect_handler_io_uses(function: &FunctionDecl) -> HandlerIoUses {
    let mut uses = HandlerIoUses::default();
    let FunctionBody::Parsed { block } = &function.body else {
        return uses;
    };

    for statement in &block.statements {
        collect_statement_io_uses(statement, &mut uses);
    }

    uses
}

fn collect_statement_io_uses(statement: &Stmt, uses: &mut HandlerIoUses) {
    match statement {
        Stmt::Let { value, .. } | Stmt::Expr { value, .. } | Stmt::Return { value, .. } => {
            collect_expr_io_uses(value, uses);
        }
        Stmt::Ensure {
            condition, error, ..
        } => {
            collect_expr_io_uses(condition, uses);
            collect_expr_io_uses(error, uses);
        }
        Stmt::Match { value, arms, .. } => {
            collect_expr_io_uses(value, uses);
            for arm in arms {
                collect_statement_io_uses(&arm.body, uses);
            }
        }
    }
}

fn collect_expr_io_uses(expr: &Expr, uses: &mut HandlerIoUses) {
    match expr {
        Expr::String { .. } | Expr::Integer { .. } | Expr::Bool { .. } | Expr::Path { .. } => {}
        Expr::Object { fields, .. } => {
            for field in fields {
                collect_expr_io_uses(&field.value, uses);
            }
        }
        Expr::Call {
            callee,
            type_args,
            args,
            range,
        } => {
            let callee_name = path_name(callee);
            if callee_name == "ctx.param" {
                uses.path_params.push(PathParamUse {
                    name: first_string_arg(args),
                    ty: single_type_arg_name(type_args),
                    range: *range,
                });
            } else if callee_name == "req.json" {
                uses.request_bodies.push(RequestBodyUse {
                    ty: single_type_arg_name(type_args),
                    range: *range,
                });
            } else if callee_name == "req.query" {
                uses.request_queries.push(RequestQueryUse {
                    ty: single_type_arg_name(type_args),
                    range: *range,
                });
            } else if callee_name == "req.headers" {
                uses.request_headers.push(RequestHeadersUse {
                    ty: single_type_arg_name(type_args),
                    range: *range,
                });
            } else if callee_name == "req.cookies" {
                uses.request_cookies.push(RequestCookiesUse {
                    ty: single_type_arg_name(type_args),
                    range: *range,
                });
            }

            for arg in args {
                collect_expr_io_uses(&arg.value, uses);
            }
        }
        Expr::Try { value, .. } => collect_expr_io_uses(value, uses),
    }
}

fn first_string_arg(args: &[CallArg]) -> Option<String> {
    args.iter().find_map(|arg| {
        if arg.label.is_some() {
            return None;
        }

        let Expr::String { value, .. } = &arg.value else {
            return None;
        };
        Some(value.clone())
    })
}

fn single_type_arg_name(type_args: &[TypeExpr]) -> Option<String> {
    match type_args {
        [ty] => Some(type_name(ty)),
        _ => None,
    }
}

fn route_param_from_segment(segment: &str) -> RouteSegmentParam {
    if let Some(name) = segment.strip_prefix(':') {
        return RouteSegmentParam::Param(RoutePathParam {
            name: name.to_string(),
            ty: None,
        });
    }

    if !(segment.starts_with('{') || segment.ends_with('}')) {
        return RouteSegmentParam::None;
    }

    let Some(inner) = segment
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
    else {
        return RouteSegmentParam::Malformed(
            "typed route parameters must use `{name: Type}` inside one path segment",
        );
    };

    let inner = inner.trim();
    if inner.is_empty() {
        return RouteSegmentParam::Malformed("route parameter braces cannot be empty");
    }

    let (name, ty) = inner
        .split_once(':')
        .map_or((inner, None), |(name, ty)| (name.trim(), Some(ty.trim())));

    if name.is_empty() {
        return RouteSegmentParam::Malformed("route parameter name cannot be empty");
    }

    if let Some(ty) = ty {
        if ty.is_empty() {
            return RouteSegmentParam::Malformed("typed route parameter type cannot be empty");
        }

        if !is_simple_type_name(ty) {
            return RouteSegmentParam::Malformed(
                "route parameter types currently use a single builtin or local type name",
            );
        }
    }

    RouteSegmentParam::Param(RoutePathParam {
        name: name.to_string(),
        ty: ty.map(str::to_string),
    })
}

fn is_http_method(method: &str) -> bool {
    matches!(
        method,
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    )
}

fn result_ok_error(ty: &TypeExpr) -> Option<(&TypeExpr, &TypeExpr)> {
    let TypeExpr::Generic { base, args, .. } = ty else {
        return None;
    };

    if !path_is(base, &["Result"]) || args.len() != 2 {
        return None;
    }

    Some((&args[0], &args[1]))
}

fn route_method_allows_body(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH")
}

fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "String" | "Text" | "Bool" | "Int" | "I64" | "U64" | "F64"
    )
}

fn is_simple_type_name(value: &str) -> bool {
    is_identifier(value)
}
