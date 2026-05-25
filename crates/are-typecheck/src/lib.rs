use are_ast::{
    EnumDecl, Field, FunctionDecl, Item, Module, Param, Path, RouteDecl, ServiceDecl, StructDecl,
    TypeExpr, UseDecl,
};
use are_diagnostics::{Diagnostic, SourceRange};
use std::collections::{HashMap, HashSet};
use std::path::{Path as FsPath, PathBuf};

#[must_use]
pub fn typecheck_module(file: &FsPath, module: &Module) -> Vec<Diagnostic> {
    TypeChecker::new(file, module).check()
}

struct TypeChecker<'a> {
    file: PathBuf,
    module: &'a Module,
    http_aliases: HashSet<String>,
    functions: HashMap<String, &'a FunctionDecl>,
    structs: HashMap<String, &'a StructDecl>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> TypeChecker<'a> {
    fn new(file: &FsPath, module: &'a Module) -> Self {
        let http_aliases = collect_http_aliases(module);
        let functions = collect_functions(module);
        let structs = collect_structs(module);

        Self {
            file: file.to_path_buf(),
            module,
            http_aliases,
            functions,
            structs,
            diagnostics: Vec::new(),
        }
    }

    fn check(mut self) -> Vec<Diagnostic> {
        self.check_items();
        self.diagnostics
    }

    fn check_items(&mut self) {
        for item in &self.module.items {
            match item {
                Item::Use(_) | Item::Type(_) => {}
                Item::Struct(decl) => self.check_struct(decl),
                Item::Enum(decl) => self.check_enum(decl),
                Item::Function(decl) => self.check_function(decl),
                Item::Service(decl) => self.check_service(decl),
            }
        }

        self.check_generic_arities();
    }

    fn check_struct(&mut self, decl: &StructDecl) {
        self.check_duplicate_fields(&decl.fields, DuplicateScope::StructField);
    }

    fn check_enum(&mut self, decl: &EnumDecl) {
        let mut variants = HashMap::new();

        for variant in &decl.variants {
            if let Some(previous) = variants.insert(variant.name.as_str(), variant.range) {
                self.duplicate(
                    "E_TYPE_0005",
                    variant.range,
                    format!("duplicate enum variant `{}`", variant.name),
                    previous,
                );
            }

            self.check_duplicate_fields(&variant.payload, DuplicateScope::EnumPayload);
        }
    }

    fn check_function(&mut self, decl: &FunctionDecl) {
        self.check_duplicate_params(&decl.params);
    }

    fn check_service(&mut self, decl: &ServiceDecl) {
        if self.http_aliases.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0001",
                &self.file,
                decl.range,
                "HTTP service requires std.http import",
                "add `use std.http as Http` so service handler types can be checked",
            ));
            return;
        }

        let Some(state_param) = &decl.state_param else {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0002",
                &self.file,
                decl.range,
                "service state is required for the HTTP MVP",
                "declare the service as `service Name(state: AppState)`",
            ));
            return;
        };

        if !is_local_struct_type(&state_param.ty, &self.structs) {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0003",
                &self.file,
                state_param.ty.range(),
                "service state must be a local struct",
                "the HTTP MVP uses a concrete AppState-style struct as service state",
            ));
        }

        self.check_routes(decl, &state_param.ty);
    }

    fn check_routes(&mut self, decl: &ServiceDecl, state_type: &TypeExpr) {
        let mut result_error_types = Vec::new();

        for route in &decl.routes {
            self.check_route_shape(route);

            let Some(handler) = route
                .handler
                .segments
                .first()
                .and_then(|name| self.functions.get(name))
                .copied()
            else {
                continue;
            };

            if let Some(error_type) = self.check_route_handler(handler, state_type) {
                result_error_types.push(error_type);
            }
        }

        self.check_error_mapping(decl, &result_error_types);
    }

    fn check_route_shape(&mut self, route: &RouteDecl) {
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

        self.check_route_params(route);
    }

    fn check_route_params(&mut self, route: &RouteDecl) {
        let mut params = HashMap::new();

        for segment in route.path.split('/') {
            let Some(name) = segment.strip_prefix(':') else {
                continue;
            };

            if name.is_empty() || !is_identifier(name) {
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0103",
                    &self.file,
                    route.range,
                    format!("invalid route parameter `:{name}`"),
                    "route parameters must use identifier syntax, such as `:id`",
                ));
                continue;
            }

            if params.insert(name, route.range).is_some() {
                self.diagnostics.push(Diagnostic::error(
                    "E_HTTP_0104",
                    &self.file,
                    route.range,
                    format!("duplicate route parameter `:{name}`"),
                    "each route parameter name must be unique within a path",
                ));
            }
        }
    }

    fn check_route_handler(
        &mut self,
        handler: &FunctionDecl,
        state_type: &TypeExpr,
    ) -> Option<TypeExpr> {
        if handler.params.len() != 2 {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0201",
                &self.file,
                handler.range,
                format!(
                    "route handler `{}` must accept exactly 2 parameters",
                    handler.name
                ),
                "HTTP handlers use `(ctx: Http.Context<AppState>, req: Http.Request)`",
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

        let req = &handler.params[1];
        if !self.is_http_path(&req.ty, "Request") {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0203",
                &self.file,
                req.ty.range(),
                format!(
                    "second parameter of `{}` must be Http.Request",
                    handler.name
                ),
                "route handlers receive the incoming request as the second parameter",
            ));
        }

        let Some(return_type) = &handler.return_type else {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0204",
                &self.file,
                handler.range,
                format!("route handler `{}` must return Http.Response", handler.name),
                "handlers may return Http.Response or Result<Http.Response, ApiError>",
            ));
            return None;
        };

        if self.is_http_path(return_type, "Response") {
            return None;
        }

        if let Some(error_type) = self.result_response_error(return_type) {
            return Some(error_type.clone());
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
            "handlers may return Http.Response or Result<Http.Response, ApiError>",
        ));

        None
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
                "routes returning Result<Http.Response, E> require `use Http.error_map(map_error)`",
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
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0305",
                &self.file,
                mapper_path.range,
                format!("unknown HTTP error mapper `{mapper_name}`"),
                "declare a mapper function before using it in the service",
            ));
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

    fn check_duplicate_fields(&mut self, fields: &[Field], scope: DuplicateScope) {
        let mut names = HashMap::new();

        for field in fields {
            if let Some(previous) = names.insert(field.name.as_str(), field.range) {
                let (code, label) = match scope {
                    DuplicateScope::StructField => ("E_TYPE_0004", "field"),
                    DuplicateScope::EnumPayload => ("E_TYPE_0006", "payload field"),
                };
                self.duplicate(
                    code,
                    field.range,
                    format!("duplicate {label} `{}`", field.name),
                    previous,
                );
            }
        }
    }

    fn check_duplicate_params(&mut self, params: &[Param]) {
        let mut names = HashMap::new();

        for param in params {
            if let Some(previous) = names.insert(param.name.as_str(), param.range) {
                self.duplicate(
                    "E_TYPE_0003",
                    param.range,
                    format!("duplicate parameter `{}`", param.name),
                    previous,
                );
            }
        }
    }

    fn check_generic_arities(&mut self) {
        for item in &self.module.items {
            match item {
                Item::Use(_) | Item::Service(_) => {}
                Item::Type(decl) => self.check_type_expr_arity(&decl.aliased),
                Item::Struct(decl) => {
                    for field in &decl.fields {
                        self.check_type_expr_arity(&field.ty);
                    }
                }
                Item::Enum(decl) => {
                    for variant in &decl.variants {
                        for field in &variant.payload {
                            self.check_type_expr_arity(&field.ty);
                        }
                    }
                }
                Item::Function(decl) => {
                    for param in &decl.params {
                        self.check_type_expr_arity(&param.ty);
                    }
                    if let Some(return_type) = &decl.return_type {
                        self.check_type_expr_arity(return_type);
                    }
                }
            }
        }
    }

    fn check_type_expr_arity(&mut self, ty: &TypeExpr) {
        match ty {
            TypeExpr::Path { .. } => {}
            TypeExpr::Option { inner, .. } => self.check_type_expr_arity(inner),
            TypeExpr::Generic { base, args, range } => {
                let expected = if path_is(base, &["Result"]) {
                    Some(2)
                } else if path_is(base, &["Option"]) || self.path_is_http(base, "Context") {
                    Some(1)
                } else {
                    None
                };

                if let Some(expected) = expected
                    && args.len() != expected
                {
                    self.diagnostics.push(Diagnostic::error(
                        "E_TYPE_0002",
                        &self.file,
                        *range,
                        format!(
                            "`{}` expects {expected} type argument(s), got {}",
                            path_name(base),
                            args.len()
                        ),
                        "generic type arguments must match the type constructor arity",
                    ));
                }

                for arg in args {
                    self.check_type_expr_arity(arg);
                }
            }
        }
    }

    fn error_map_uses(&self, decl: &'a ServiceDecl) -> Vec<&'a are_ast::ServiceUse> {
        decl.uses
            .iter()
            .filter(|service_use| self.path_is_http(&service_use.target, "error_map"))
            .collect()
    }

    fn result_response_error<'b>(&self, ty: &'b TypeExpr) -> Option<&'b TypeExpr> {
        let TypeExpr::Generic { base, args, .. } = ty else {
            return None;
        };

        if !path_is(base, &["Result"])
            || args.len() != 2
            || !self.is_http_path(&args[0], "Response")
        {
            return None;
        }

        Some(&args[1])
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

    fn path_is_http(&self, path: &Path, name: &str) -> bool {
        path.segments.len() == 2
            && self.http_aliases.contains(&path.segments[0])
            && path.segments[1] == name
    }

    fn duplicate(
        &mut self,
        code: &'static str,
        range: SourceRange,
        problem: String,
        previous: SourceRange,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code,
                &self.file,
                range,
                problem,
                format!(
                    "the previous declaration is at {}:{}",
                    previous.start.line, previous.start.column
                ),
            )
            .with_fix("rename this item or remove the duplicate", None),
        );
    }
}

#[derive(Debug, Clone, Copy)]
enum DuplicateScope {
    StructField,
    EnumPayload,
}

fn collect_http_aliases(module: &Module) -> HashSet<String> {
    module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Use(UseDecl { path, alias, .. }) = item else {
                return None;
            };

            if !path_is(path, &["std", "http"]) {
                return None;
            }

            alias.clone().or_else(|| path.segments.last().cloned())
        })
        .collect()
}

fn collect_functions(module: &Module) -> HashMap<String, &FunctionDecl> {
    module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Function(decl) = item else {
                return None;
            };
            Some((decl.name.clone(), decl))
        })
        .collect()
}

fn collect_structs(module: &Module) -> HashMap<String, &StructDecl> {
    module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Struct(decl) = item else {
                return None;
            };
            Some((decl.name.clone(), decl))
        })
        .collect()
}

fn is_local_struct_type(ty: &TypeExpr, structs: &HashMap<String, &StructDecl>) -> bool {
    let TypeExpr::Path { path } = ty else {
        return false;
    };

    path.segments.len() == 1 && structs.contains_key(&path.segments[0])
}

fn path_is(path: &Path, expected: &[&str]) -> bool {
    path.segments.len() == expected.len()
        && path
            .segments
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
}

fn path_name(path: &Path) -> String {
    path.segments.join(".")
}

fn type_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Path { path } => path_name(path),
        TypeExpr::Generic { base, args, .. } => {
            let args = args.iter().map(type_name).collect::<Vec<_>>().join(", ");
            format!("{}<{args}>", path_name(base))
        }
        TypeExpr::Option { inner, .. } => format!("{}?", type_name(inner)),
    }
}

fn same_type(left: &TypeExpr, right: &TypeExpr) -> bool {
    match (left, right) {
        (TypeExpr::Path { path: left }, TypeExpr::Path { path: right }) => {
            left.segments == right.segments
        }
        (
            TypeExpr::Generic {
                base: left_base,
                args: left_args,
                ..
            },
            TypeExpr::Generic {
                base: right_base,
                args: right_args,
                ..
            },
        ) => {
            left_base.segments == right_base.segments
                && left_args.len() == right_args.len()
                && left_args
                    .iter()
                    .zip(right_args)
                    .all(|(left, right)| same_type(left, right))
        }
        (TypeExpr::Option { inner: left, .. }, TypeExpr::Option { inner: right, .. }) => {
            same_type(left, right)
        }
        _ => false,
    }
}

fn is_http_method(method: &str) -> bool {
    matches!(
        method,
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    )
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    matches!(first, 'A'..='Z' | 'a'..='z' | '_')
        && chars.all(|ch| matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_'))
}

#[cfg(test)]
mod tests {
    use super::typecheck_module;
    use are_lexer::lex_source;
    use are_parser::parse_tokens;
    use are_resolver::resolve_module;
    use std::path::Path;

    #[test]
    fn typechecks_users_api_shape() {
        let source = include_str!("../../../examples/users_api/main.are");
        let diagnostics = diagnostics_for("examples/users_api/main.are", source);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    }

    #[test]
    fn rejects_bad_route_handler_context() {
        let source = r#"
            use std.http as Http

            struct AppState {}
            struct OtherState {}

            fn bad(ctx: Http.Context<OtherState>, req: Http.Request) -> Http.Response {}

            service Api(state: AppState) {
                route GET "/bad" -> bad
            }
        "#;

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "E_HTTP_0202");
    }

    #[test]
    fn requires_error_map_for_result_handlers() {
        let source = r#"
            use std.http as Http

            struct AppState {}
            enum ApiError { Failed }

            fn create(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {}

            service Api(state: AppState) {
                route POST "/users" -> create
            }
        "#;

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "E_HTTP_0302");
    }

    #[test]
    fn validates_error_map_signature() {
        let source = r#"
            use std.http as Http

            struct AppState {}
            enum ApiError { Failed }
            enum OtherError { Failed }

            fn create(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {}
            fn map_error(err: OtherError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                route POST "/users" -> create
            }
        "#;

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "E_HTTP_0307");
    }

    #[test]
    fn rejects_duplicate_struct_fields_and_params() {
        let source = r"
            struct User {
                id: U64
                id: U64
            }

            fn f(id: U64, id: U64) {}
        ";

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].code, "E_TYPE_0004");
        assert_eq!(diagnostics[1].code, "E_TYPE_0003");
    }

    #[test]
    fn rejects_duplicate_route_params() {
        let source = r#"
            use std.http as Http

            struct AppState {}

            fn get(ctx: Http.Context<AppState>, req: Http.Request) -> Http.Response {}

            service Api(state: AppState) {
                route GET "/users/:id/orders/:id" -> get
            }
        "#;

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "E_HTTP_0104");
    }

    #[test]
    fn rejects_invalid_result_arity() {
        let source = r#"
            use std.http as Http

            struct AppState {}

            fn create(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response> {}

            service Api(state: AppState) {
                route POST "/users" -> create
            }
        "#;

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].code, "E_HTTP_0204");
        assert_eq!(diagnostics[1].code, "E_TYPE_0002");
    }

    fn diagnostics_for(file_name: &str, source: &str) -> Vec<are_diagnostics::Diagnostic> {
        let file = Path::new(file_name);
        let (tokens, lex_diagnostics) = lex_source(file, source);
        assert!(lex_diagnostics.is_empty(), "{lex_diagnostics:#?}");

        let (module, parse_diagnostics) = parse_tokens(file, &tokens);
        assert!(parse_diagnostics.is_empty(), "{parse_diagnostics:#?}");
        let module = module.expect("module parses");

        let resolve_diagnostics = resolve_module(file, &module);
        let mut diagnostics = resolve_diagnostics;
        diagnostics.extend(typecheck_module(file, &module));
        diagnostics
    }
}
