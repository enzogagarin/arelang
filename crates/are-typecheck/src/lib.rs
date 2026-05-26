use are_ast::{
    CallArg, EnumDecl, Expr, Field, FunctionBody, FunctionDecl, Item, ModelDecl, ModelField,
    ModelFieldAttr, Module, Param, Path, Pattern, RouteDecl, ServiceDecl, Stmt, StructDecl,
    TypeDecl, TypeExpr, UseDecl,
};
use are_diagnostics::{Diagnostic, SourceRange, best_name_suggestion};
use are_semantics::{Builtin, builtin_by_callee};
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
    models: HashMap<String, &'a ModelDecl>,
    enums: HashMap<String, &'a EnumDecl>,
    types: HashMap<String, &'a TypeDecl>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> TypeChecker<'a> {
    fn new(file: &FsPath, module: &'a Module) -> Self {
        let http_aliases = collect_http_aliases(module);
        let functions = collect_functions(module);
        let structs = collect_structs(module);
        let models = collect_models(module);
        let enums = collect_enums(module);
        let types = collect_types(module);

        Self {
            file: file.to_path_buf(),
            module,
            http_aliases,
            functions,
            structs,
            models,
            enums,
            types,
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
                Item::Model(decl) => self.check_model(decl),
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

    fn check_model(&mut self, decl: &ModelDecl) {
        self.check_duplicate_model_fields(&decl.fields);
        self.check_model_attrs(decl);
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
        self.check_function_body(decl);
    }

    fn check_function_body(&mut self, decl: &FunctionDecl) {
        let FunctionBody::Parsed { block } = &decl.body else {
            return;
        };

        let return_type = decl
            .return_type
            .as_ref()
            .map(|ty| self.body_type_from_type_expr(ty));
        let result_error = return_type.as_ref().and_then(BodyType::result_error);
        let mut body = BodyChecker {
            file: &self.file,
            http_aliases: &self.http_aliases,
            functions: &self.functions,
            structs: &self.structs,
            models: &self.models,
            enums: &self.enums,
            types: &self.types,
            env: decl
                .params
                .iter()
                .map(|param| (param.name.clone(), self.body_type_from_type_expr(&param.ty)))
                .collect(),
            return_type,
            result_error,
            diagnostics: Vec::new(),
        };

        body.check_statements(&block.statements, &decl.name);
        self.diagnostics.extend(body.diagnostics);
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

    fn check_duplicate_model_fields(&mut self, fields: &[ModelField]) {
        let mut names = HashMap::new();

        for field in fields {
            if let Some(previous) = names.insert(field.name.as_str(), field.range) {
                self.duplicate(
                    "E_TYPE_0007",
                    field.range,
                    format!("duplicate model field `{}`", field.name),
                    previous,
                );
            }
        }
    }

    fn check_model_attrs(&mut self, decl: &ModelDecl) {
        let mut primary = None;

        for field in &decl.fields {
            let mut attrs = HashMap::new();
            for attr in &field.attrs {
                if let Some(previous) = attrs.insert(*attr, field.range) {
                    self.duplicate(
                        "E_TYPE_0008",
                        field.range,
                        format!(
                            "duplicate model field attribute `{}`",
                            model_attr_name(*attr)
                        ),
                        previous,
                    );
                }

                if *attr == ModelFieldAttr::Primary
                    && let Some(previous) = primary.replace(field.range)
                {
                    self.duplicate(
                        "E_TYPE_0009",
                        field.range,
                        format!("model `{}` has multiple primary fields", decl.name),
                        previous,
                    );
                }
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
                Item::Model(decl) => {
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

    fn body_type_from_type_expr(&self, ty: &TypeExpr) -> BodyType {
        body_type_from_type_expr(ty, &self.http_aliases)
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

    fn function_suggestion(&self, name: &str) -> Option<String> {
        let mut candidates = self
            .functions
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        candidates.sort_unstable();

        best_name_suggestion(name, candidates).map(str::to_string)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BodyType {
    Unit,
    Bool,
    String,
    Integer,
    Object,
    Named(String),
    HttpRequest,
    HttpContext(String),
    HttpResponse,
    Result {
        ok: Box<BodyType>,
        error: Box<BodyType>,
    },
    Unknown,
}

impl BodyType {
    fn result_error(&self) -> Option<BodyType> {
        match self {
            Self::Result { error, .. } => Some((**error).clone()),
            _ => None,
        }
    }

    fn result_ok(&self) -> Option<BodyType> {
        match self {
            Self::Result { ok, .. } => Some((**ok).clone()),
            _ => None,
        }
    }

    fn display(&self) -> String {
        match self {
            Self::Unit => "Unit".to_string(),
            Self::Bool => "Bool".to_string(),
            Self::String => "String".to_string(),
            Self::Integer => "Integer".to_string(),
            Self::Object => "Object".to_string(),
            Self::Named(name) => name.clone(),
            Self::HttpRequest => "Http.Request".to_string(),
            Self::HttpContext(state) => format!("Http.Context<{state}>"),
            Self::HttpResponse => "Http.Response".to_string(),
            Self::Result { ok, error } => {
                format!("Result<{}, {}>", ok.display(), error.display())
            }
            Self::Unknown => "Unknown".to_string(),
        }
    }
}

struct BodyChecker<'a> {
    file: &'a FsPath,
    http_aliases: &'a HashSet<String>,
    functions: &'a HashMap<String, &'a FunctionDecl>,
    structs: &'a HashMap<String, &'a StructDecl>,
    models: &'a HashMap<String, &'a ModelDecl>,
    enums: &'a HashMap<String, &'a EnumDecl>,
    types: &'a HashMap<String, &'a TypeDecl>,
    env: HashMap<String, BodyType>,
    return_type: Option<BodyType>,
    result_error: Option<BodyType>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
struct EnumVariantShape {
    name: String,
    payload: Vec<(String, BodyType)>,
}

impl BodyChecker<'_> {
    fn check_statements(&mut self, statements: &[Stmt], function_name: &str) {
        for statement in statements {
            self.check_statement(statement, function_name);
        }
    }

    fn check_statement(&mut self, statement: &Stmt, function_name: &str) {
        match statement {
            Stmt::Let { name, value, .. } => {
                let value_type = self.check_expr(value);
                self.env.insert(name.clone(), value_type);
            }
            Stmt::Expr { value, .. } => {
                self.check_expr(value);
            }
            Stmt::Return { value, range } => {
                let value_type = self.check_expr(value);
                self.check_return_type(&value_type, *range, function_name);
            }
            Stmt::Ensure {
                condition,
                error,
                range,
            } => {
                self.check_ensure_statement(condition, error, *range);
            }
            Stmt::Match { value, arms, range } => {
                self.check_match_statement(value, arms, *range, function_name);
            }
        }
    }

    fn check_ensure_statement(&mut self, condition: &Expr, error: &Expr, range: SourceRange) {
        let condition_type = self.check_expr(condition);
        self.expect_exact_type(
            &condition_type,
            &BodyType::Bool,
            condition.range(),
            "`ensure` expects a boolean condition",
        );

        let error_type = self.check_expr(error);
        let Some(expected_error) = self.result_error.clone() else {
            self.error(
                "E_BODY_0017",
                range,
                "`ensure` requires the function to return Result<_, E>",
                "ensure raises an error value when its condition is false",
            );
            return;
        };

        self.expect_exact_type(
            &error_type,
            &expected_error,
            error.range(),
            format!(
                "`ensure` must raise `{}` in this function",
                expected_error.display()
            ),
        );
    }

    fn check_match_statement(
        &mut self,
        value: &Expr,
        arms: &[are_ast::MatchArm],
        range: SourceRange,
        function_name: &str,
    ) {
        let matched_type = self.check_expr(value);
        let BodyType::Named(enum_name) = matched_type else {
            self.error(
                "E_BODY_0010",
                range,
                format!("`match` cannot inspect `{}`", matched_type.display()),
                "`match` currently works on local enum values",
            );
            for arm in arms {
                self.check_statement(&arm.body, function_name);
            }
            return;
        };

        let Some(variants) = self.enum_variants(&enum_name) else {
            self.error(
                "E_BODY_0010",
                range,
                format!("`{enum_name}` is not a local enum"),
                "`match` currently works on local enum values",
            );
            for arm in arms {
                self.check_statement(&arm.body, function_name);
            }
            return;
        };

        let mut seen = HashSet::new();
        for arm in arms {
            self.check_match_arm(arm, &variants, &mut seen, function_name);
        }

        let missing = variants
            .iter()
            .filter(|variant| !seen.contains(&variant.name))
            .map(|variant| variant.name.as_str())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            self.error(
                "E_BODY_0011",
                range,
                format!("`match` does not cover variant(s): {}", missing.join(", ")),
                "error mappers should handle every variant explicitly",
            );
        }
    }

    fn check_match_arm(
        &mut self,
        arm: &are_ast::MatchArm,
        variants: &[EnumVariantShape],
        seen: &mut HashSet<String>,
        function_name: &str,
    ) {
        let Pattern::Variant {
            name,
            bindings,
            range,
        } = &arm.pattern;

        let Some(variant) = variants.iter().find(|variant| variant.name == *name) else {
            let suggestion =
                Self::suggestion(name, variants.iter().map(|variant| variant.name.as_str()));
            self.error_with_suggestion(
                "E_BODY_0012",
                *range,
                format!("unknown enum variant `{name}`"),
                "match arms must use variants from the matched enum",
                suggestion,
            );
            self.check_statement(&arm.body, function_name);
            return;
        };

        seen.insert(name.clone());

        if bindings.len() != variant.payload.len() {
            self.error(
                "E_BODY_0013",
                *range,
                format!(
                    "`{name}` pattern expects {} binding(s), got {}",
                    variant.payload.len(),
                    bindings.len()
                ),
                "pattern bindings must match the enum variant payload",
            );
        }

        let previous = self.bind_pattern_payload(bindings, &variant.payload, *range);
        self.check_statement(&arm.body, function_name);
        self.restore_bindings(previous);
    }

    fn check_expr(&mut self, expr: &Expr) -> BodyType {
        match expr {
            Expr::String { .. } => BodyType::String,
            Expr::Integer { .. } => BodyType::Integer,
            Expr::Bool { .. } => BodyType::Bool,
            Expr::Object { fields, .. } => {
                for field in fields {
                    self.check_expr(&field.value);
                }
                BodyType::Object
            }
            Expr::Path { path } => self.check_path(path),
            Expr::Call {
                callee,
                type_args,
                args,
                range,
            } => self.check_call(callee, type_args, args, *range),
            Expr::Try { value, range } => self.check_try(value, *range),
        }
    }

    fn check_path(&mut self, path: &Path) -> BodyType {
        if let Some(enum_name) = self.check_empty_enum_variant_path(path) {
            return enum_name;
        }

        let Some(first) = path.segments.first() else {
            return BodyType::Unknown;
        };

        let mut current = if let Some(binding) = self.env.get(first) {
            binding.clone()
        } else {
            let suggestion = Self::suggestion(first, self.env.keys().map(String::as_str));
            self.error_with_suggestion(
                "E_BODY_0001",
                path.range,
                format!("unknown binding `{first}`"),
                "bindings must be declared with `let` or as function parameters before use",
                suggestion,
            );
            BodyType::Unknown
        };

        for segment in path.segments.iter().skip(1) {
            current = self.field_type(&current, segment, path.range);
        }

        current
    }

    fn field_type(&mut self, owner: &BodyType, field_name: &str, range: SourceRange) -> BodyType {
        let BodyType::Named(type_name) = owner else {
            self.error(
                "E_BODY_0002",
                range,
                format!("`{}` has no field `{field_name}`", owner.display()),
                "field access is currently supported on local struct and model values",
            );
            return BodyType::Unknown;
        };

        if let Some(struct_decl) = self.structs.get(type_name) {
            let Some(field) = struct_decl
                .fields
                .iter()
                .find(|field| field.name == field_name)
            else {
                let suggestion = Self::suggestion(
                    field_name,
                    struct_decl.fields.iter().map(|field| field.name.as_str()),
                );
                self.error_with_suggestion(
                    "E_BODY_0002",
                    range,
                    format!("struct `{type_name}` has no field `{field_name}`"),
                    "check the field name or update the struct declaration",
                    suggestion,
                );
                return BodyType::Unknown;
            };

            return body_type_from_type_expr(&field.ty, self.http_aliases);
        }

        if let Some(model_decl) = self.models.get(type_name) {
            let Some(field) = model_decl
                .fields
                .iter()
                .find(|field| field.name == field_name)
            else {
                let suggestion = Self::suggestion(
                    field_name,
                    model_decl.fields.iter().map(|field| field.name.as_str()),
                );
                self.error_with_suggestion(
                    "E_BODY_0002",
                    range,
                    format!("model `{type_name}` has no field `{field_name}`"),
                    "check the field name or update the model declaration",
                    suggestion,
                );
                return BodyType::Unknown;
            };

            return body_type_from_type_expr(&field.ty, self.http_aliases);
        }

        self.error(
            "E_BODY_0002",
            range,
            format!("unknown data type `{type_name}` in field access"),
            "only local struct and model fields can be selected in the current body checker",
        );
        BodyType::Unknown
    }

    fn check_call(
        &mut self,
        callee: &Path,
        type_args: &[TypeExpr],
        args: &[CallArg],
        range: SourceRange,
    ) -> BodyType {
        let callee_name = path_name(callee);
        if let Some(builtin) = builtin_by_callee(&callee_name) {
            return self.check_builtin_call(builtin, &callee_name, type_args, args, range);
        }

        if let Some(enum_value) = self.check_enum_constructor(callee, type_args, args, range) {
            return enum_value;
        }

        if callee.segments.len() == 1
            && let Some(function) = self.functions.get(&callee.segments[0]).copied()
        {
            return self.check_user_function_call(function, type_args, args, range);
        }

        let suggestion = if callee.segments.len() == 1 {
            Self::suggestion(
                &callee.segments[0],
                self.functions.keys().map(String::as_str),
            )
        } else {
            None
        };
        self.error_with_suggestion(
            "E_BODY_0003",
            range,
            format!("unsupported call `{callee_name}`"),
            "calls must target a known local function or supported std backend function",
            suggestion,
        );
        BodyType::Unknown
    }

    fn check_builtin_call(
        &mut self,
        builtin: Builtin,
        callee_name: &str,
        type_args: &[TypeExpr],
        args: &[CallArg],
        range: SourceRange,
    ) -> BodyType {
        if !type_args.is_empty() && !matches!(builtin, Builtin::RequestJson | Builtin::ContextParam)
        {
            self.error(
                "E_BODY_0014",
                range,
                format!(
                    "`{callee_name}` does not accept type argument(s), got {}",
                    type_args.len()
                ),
                "only generic std calls such as `req.json<T>()` accept type arguments",
            );
        }

        match builtin {
            Builtin::HttpResponseOk | Builtin::HttpResponseCreated => {
                self.expect_positional_arity(callee_name, args, 1, range);
                self.check_positional_arg(args, 0);
                BodyType::HttpResponse
            }
            Builtin::HttpResponseError => {
                let status_type = self.expect_positional_arg(callee_name, args, 0, 2, range);
                self.expect_exact_type(
                    &status_type,
                    &BodyType::Integer,
                    range,
                    "Http.Response.error expects an integer HTTP status",
                );
                self.check_positional_arg(args, 1);
                BodyType::HttpResponse
            }
            Builtin::RequestJson => {
                self.expect_positional_arity(callee_name, args, 0, range);
                let ok = self.single_type_arg(callee_name, type_args, range);
                self.result_type(ok)
            }
            Builtin::ValidateEmail => {
                let value_type = self.expect_single_arg(callee_name, args, range);
                self.expect_string_like(
                    &value_type,
                    range,
                    "validate.email expects a string value",
                );
                BodyType::Bool
            }
            Builtin::ValidateLength => {
                let value_type = self.expect_positional_arg(callee_name, args, 0, 1, range);
                self.expect_string_like(
                    &value_type,
                    range,
                    "validate.length expects a string value",
                );
                self.expect_named_integer(callee_name, args, "min", range);
                self.expect_named_integer(callee_name, args, "max", range);
                BodyType::Bool
            }
            Builtin::ContextParam => {
                let name_type = self.expect_single_arg(callee_name, args, range);
                self.expect_exact_type(
                    &name_type,
                    &BodyType::String,
                    range,
                    "ctx.param expects the route parameter name as a string",
                );
                let ok = self.single_type_arg(callee_name, type_args, range);
                self.result_type(ok)
            }
            Builtin::StateUsersInsert
            | Builtin::StateUsersGet
            | Builtin::DbUsersInsert
            | Builtin::DbUsersGet => {
                self.expect_positional_arity(callee_name, args, 1, range);
                self.check_positional_arg(args, 0);
                self.result_type(self.named_or_unknown("User"))
            }
        }
    }

    fn check_enum_constructor(
        &mut self,
        callee: &Path,
        type_args: &[TypeExpr],
        args: &[CallArg],
        range: SourceRange,
    ) -> Option<BodyType> {
        let [enum_name, variant_name] = callee.segments.as_slice() else {
            return None;
        };

        let Some(variant) = self.enum_variant(enum_name, variant_name) else {
            if let Some(variants) = self.enum_variants(enum_name) {
                let suggestion = Self::suggestion(
                    variant_name,
                    variants.iter().map(|variant| variant.name.as_str()),
                );
                self.error_with_suggestion(
                    "E_BODY_0012",
                    range,
                    format!("unknown enum variant `{variant_name}`"),
                    "enum constructors must use variants from the target enum",
                    suggestion,
                );
                return Some(BodyType::Named(enum_name.clone()));
            }

            return None;
        };
        if !type_args.is_empty() {
            self.error(
                "E_BODY_0014",
                range,
                format!(
                    "`{}` does not accept type argument(s), got {}",
                    path_name(callee),
                    type_args.len()
                ),
                "enum constructors use their declared payload types",
            );
        }

        for arg in args.iter().filter(|arg| arg.label.is_some()) {
            self.error(
                "E_BODY_0015",
                arg.range,
                format!("`{}` does not accept named arguments", path_name(callee)),
                "enum constructor payloads currently use positional arguments",
            );
        }

        let positional = args
            .iter()
            .filter(|arg| arg.label.is_none())
            .collect::<Vec<_>>();
        if positional.len() != variant.payload.len() {
            self.error(
                "E_BODY_0004",
                range,
                format!(
                    "`{}` expects {} payload value(s), got {}",
                    path_name(callee),
                    variant.payload.len(),
                    positional.len()
                ),
                "enum constructor calls must match the variant payload",
            );
        }

        for (arg, (_field_name, expected)) in positional.iter().zip(&variant.payload) {
            let actual = self.check_expr(&arg.value);
            self.expect_exact_type(
                &actual,
                expected,
                arg.range,
                format!(
                    "`{}` payload expects `{}`",
                    path_name(callee),
                    expected.display()
                ),
            );
        }

        Some(BodyType::Named(enum_name.clone()))
    }

    fn check_empty_enum_variant_path(&mut self, path: &Path) -> Option<BodyType> {
        let [enum_name, variant_name] = path.segments.as_slice() else {
            return None;
        };

        let variant = self.enum_variant(enum_name, variant_name)?;
        if !variant.payload.is_empty() {
            self.error(
                "E_BODY_0018",
                path.range,
                format!("`{}` requires payload values", path_name(path)),
                "call the enum constructor with payload values",
            );
            return Some(BodyType::Named(enum_name.clone()));
        }

        Some(BodyType::Named(enum_name.clone()))
    }

    fn check_user_function_call(
        &mut self,
        function: &FunctionDecl,
        type_args: &[TypeExpr],
        args: &[CallArg],
        range: SourceRange,
    ) -> BodyType {
        if !type_args.is_empty() {
            self.error(
                "E_BODY_0014",
                range,
                format!(
                    "`{}` does not accept type argument(s), got {}",
                    function.name,
                    type_args.len()
                ),
                "local functions are monomorphic in the current language slice",
            );
        }

        for arg in args.iter().filter(|arg| arg.label.is_some()) {
            self.error(
                "E_BODY_0015",
                arg.range,
                format!("`{}` does not accept named arguments", function.name),
                "local function calls currently use positional arguments",
            );
        }

        let positional = args
            .iter()
            .filter(|arg| arg.label.is_none())
            .collect::<Vec<_>>();
        if positional.len() != function.params.len() {
            self.error(
                "E_BODY_0004",
                range,
                format!(
                    "`{}` expects {} argument(s), got {}",
                    function.name,
                    function.params.len(),
                    positional.len()
                ),
                "local function calls must match the function signature",
            );
        }

        for (arg, param) in positional.iter().zip(&function.params) {
            let actual = self.check_expr(&arg.value);
            let expected = body_type_from_type_expr(&param.ty, self.http_aliases);
            self.expect_exact_type(
                &actual,
                &expected,
                arg.range,
                format!(
                    "`{}` parameter `{}` expects `{}`",
                    function.name,
                    param.name,
                    expected.display()
                ),
            );
        }

        function.return_type.as_ref().map_or(BodyType::Unit, |ty| {
            body_type_from_type_expr(ty, self.http_aliases)
        })
    }

    fn enum_variants(&self, enum_name: &str) -> Option<Vec<EnumVariantShape>> {
        let decl = self.enums.get(enum_name)?;
        Some(
            decl.variants
                .iter()
                .map(|variant| EnumVariantShape {
                    name: variant.name.clone(),
                    payload: variant
                        .payload
                        .iter()
                        .map(|field| {
                            (
                                field.name.clone(),
                                body_type_from_type_expr(&field.ty, self.http_aliases),
                            )
                        })
                        .collect(),
                })
                .collect(),
        )
    }

    fn enum_variant(&self, enum_name: &str, variant_name: &str) -> Option<EnumVariantShape> {
        self.enum_variants(enum_name)?
            .into_iter()
            .find(|variant| variant.name == variant_name)
    }

    fn bind_pattern_payload(
        &mut self,
        bindings: &[String],
        payload: &[(String, BodyType)],
        range: SourceRange,
    ) -> Vec<(String, Option<BodyType>)> {
        let mut seen = HashSet::new();
        let mut previous = Vec::new();

        for (binding, (_field_name, ty)) in bindings.iter().zip(payload) {
            if !seen.insert(binding.as_str()) {
                self.error(
                    "E_BODY_0016",
                    range,
                    format!("duplicate pattern binding `{binding}`"),
                    "each payload binding in a match arm must have a unique name",
                );
                continue;
            }

            previous.push((
                binding.clone(),
                self.env.insert(binding.clone(), ty.clone()),
            ));
        }

        previous
    }

    fn restore_bindings(&mut self, previous: Vec<(String, Option<BodyType>)>) {
        for (name, value) in previous {
            if let Some(value) = value {
                self.env.insert(name, value);
            } else {
                self.env.remove(&name);
            }
        }
    }

    fn check_try(&mut self, value: &Expr, range: SourceRange) -> BodyType {
        let value_type = self.check_expr(value);
        let Some(ok_type) = value_type.result_ok() else {
            self.error(
                "E_BODY_0005",
                range,
                format!("`?` cannot be used on `{}`", value_type.display()),
                "`?` can only unwrap Result values",
            );
            return BodyType::Unknown;
        };

        let Some(actual_error) = value_type.result_error() else {
            return ok_type;
        };

        if let Some(expected_error) = &self.result_error {
            if !same_body_type(expected_error, &actual_error) {
                self.error(
                    "E_BODY_0006",
                    range,
                    format!(
                        "`?` propagates `{}` but the function returns `{}`",
                        actual_error.display(),
                        expected_error.display()
                    ),
                    "the propagated error type must match the function Result error type",
                );
            }
        } else {
            self.error(
                "E_BODY_0005",
                range,
                "`?` requires the function to return Result<_, E>",
                "change the function return type to Result or handle the error explicitly",
            );
        }

        ok_type
    }

    fn check_return_type(
        &mut self,
        value_type: &BodyType,
        range: SourceRange,
        function_name: &str,
    ) {
        let Some(return_type) = &self.return_type else {
            self.error(
                "E_BODY_0009",
                range,
                format!("`{function_name}` returns a value but has no return type"),
                "add a return type to the function signature",
            );
            return;
        };

        if Self::return_accepts(return_type, value_type) {
            return;
        }

        self.error(
            "E_BODY_0009",
            range,
            format!(
                "`{function_name}` returns `{}` but its signature expects `{}`",
                value_type.display(),
                return_type.display()
            ),
            "return expressions must match the function return type",
        );
    }

    fn return_accepts(expected: &BodyType, actual: &BodyType) -> bool {
        if same_body_type(expected, actual) {
            return true;
        }

        matches!(
            expected,
            BodyType::Result { ok, .. } if same_body_type(ok, actual)
        )
    }

    fn result_type(&self, ok: BodyType) -> BodyType {
        BodyType::Result {
            ok: Box::new(ok),
            error: Box::new(self.result_error.clone().unwrap_or(BodyType::Unknown)),
        }
    }

    fn single_type_arg(
        &mut self,
        callee: &str,
        type_args: &[TypeExpr],
        range: SourceRange,
    ) -> BodyType {
        if type_args.len() != 1 {
            self.error(
                "E_BODY_0007",
                range,
                format!(
                    "`{callee}` expects 1 type argument, got {}",
                    type_args.len()
                ),
                "write the expected payload type explicitly, such as `req.json<CreateUserInput>()`",
            );
            return BodyType::Unknown;
        }

        body_type_from_type_expr(&type_args[0], self.http_aliases)
    }

    fn expect_single_arg(
        &mut self,
        callee: &str,
        args: &[CallArg],
        range: SourceRange,
    ) -> BodyType {
        self.expect_positional_arg(callee, args, 0, 1, range)
    }

    fn expect_positional_arg(
        &mut self,
        callee: &str,
        args: &[CallArg],
        index: usize,
        expected: usize,
        range: SourceRange,
    ) -> BodyType {
        self.expect_positional_arity(callee, args, expected, range);
        self.check_positional_arg(args, index)
    }

    fn expect_positional_arity(
        &mut self,
        callee: &str,
        args: &[CallArg],
        expected: usize,
        range: SourceRange,
    ) {
        let actual = args.iter().filter(|arg| arg.label.is_none()).count();
        if actual == expected {
            return;
        }

        self.error(
            "E_BODY_0004",
            range,
            format!("`{callee}` expects {expected} positional argument(s), got {actual}"),
            "call arguments must match the builtin function contract",
        );
    }

    fn check_positional_arg(&mut self, args: &[CallArg], index: usize) -> BodyType {
        let Some(arg) = args.iter().filter(|arg| arg.label.is_none()).nth(index) else {
            return BodyType::Unknown;
        };

        self.check_expr(&arg.value)
    }

    fn expect_named_integer(
        &mut self,
        callee: &str,
        args: &[CallArg],
        label: &str,
        range: SourceRange,
    ) {
        let Some(arg) = args.iter().find(|arg| arg.label.as_deref() == Some(label)) else {
            self.error(
                "E_BODY_0008",
                range,
                format!("`{callee}` is missing `{label}:`"),
                "named arguments are part of the builtin function contract",
            );
            return;
        };

        let value_type = self.check_expr(&arg.value);
        self.expect_exact_type(
            &value_type,
            &BodyType::Integer,
            arg.range,
            format!("`{label}` must be an integer literal"),
        );
    }

    fn expect_string_like(
        &mut self,
        actual: &BodyType,
        range: SourceRange,
        reason: impl Into<String>,
    ) {
        if self.is_string_like(actual) {
            return;
        }

        self.error(
            "E_BODY_0006",
            range,
            format!("expected String, got `{}`", actual.display()),
            reason,
        );
    }

    fn expect_exact_type(
        &mut self,
        actual: &BodyType,
        expected: &BodyType,
        range: SourceRange,
        reason: impl Into<String>,
    ) {
        if same_body_type(actual, expected) || matches!(actual, BodyType::Unknown) {
            return;
        }

        self.error(
            "E_BODY_0006",
            range,
            format!(
                "expected `{}`, got `{}`",
                expected.display(),
                actual.display()
            ),
            reason,
        );
    }

    fn is_string_like(&self, actual: &BodyType) -> bool {
        if matches!(actual, BodyType::String | BodyType::Unknown) {
            return true;
        }

        match actual {
            BodyType::Named(name) => self.types.get(name).is_some_and(|decl| {
                matches!(&decl.aliased, TypeExpr::Path { path } if path_is(path, &["String"]))
            }),
            _ => false,
        }
    }

    fn named_or_unknown(&self, name: &str) -> BodyType {
        if self.structs.contains_key(name)
            || self.models.contains_key(name)
            || self.types.contains_key(name)
        {
            BodyType::Named(name.to_string())
        } else {
            BodyType::Unknown
        }
    }

    fn error(
        &mut self,
        code: impl Into<String>,
        range: SourceRange,
        problem: impl Into<String>,
        reason: impl Into<String>,
    ) {
        self.error_with_suggestion(code, range, problem, reason, None);
    }

    fn error_with_suggestion(
        &mut self,
        code: impl Into<String>,
        range: SourceRange,
        problem: impl Into<String>,
        reason: impl Into<String>,
        suggestion: Option<String>,
    ) {
        let mut diagnostic = Diagnostic::error(code, self.file, range, problem, reason);
        if let Some(suggestion) = suggestion {
            diagnostic =
                diagnostic.with_fix(format!("did you mean `{suggestion}`?"), Some(suggestion));
        }
        self.diagnostics.push(diagnostic);
    }

    fn suggestion<'b>(name: &str, candidates: impl IntoIterator<Item = &'b str>) -> Option<String> {
        let mut candidates = candidates.into_iter().collect::<Vec<_>>();
        candidates.sort_unstable();

        best_name_suggestion(name, candidates).map(str::to_string)
    }
}

#[derive(Debug, Clone, Copy)]
enum DuplicateScope {
    StructField,
    EnumPayload,
}

fn model_attr_name(attr: ModelFieldAttr) -> &'static str {
    match attr {
        ModelFieldAttr::Primary => "primary",
        ModelFieldAttr::Unique => "unique",
    }
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

fn collect_models(module: &Module) -> HashMap<String, &ModelDecl> {
    module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Model(decl) = item else {
                return None;
            };
            Some((decl.name.clone(), decl))
        })
        .collect()
}

fn collect_enums(module: &Module) -> HashMap<String, &EnumDecl> {
    module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Enum(decl) = item else {
                return None;
            };
            Some((decl.name.clone(), decl))
        })
        .collect()
}

fn collect_types(module: &Module) -> HashMap<String, &TypeDecl> {
    module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Type(decl) = item else {
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

fn body_type_from_type_expr(ty: &TypeExpr, http_aliases: &HashSet<String>) -> BodyType {
    match ty {
        TypeExpr::Path { path } => {
            if path.segments.len() == 2
                && http_aliases.contains(&path.segments[0])
                && path.segments[1] == "Request"
            {
                return BodyType::HttpRequest;
            }

            if path.segments.len() == 2
                && http_aliases.contains(&path.segments[0])
                && path.segments[1] == "Response"
            {
                return BodyType::HttpResponse;
            }

            if path_is(path, &["String"]) {
                return BodyType::String;
            }

            if path_is(path, &["Bool"]) {
                return BodyType::Bool;
            }

            BodyType::Named(path_name(path))
        }
        TypeExpr::Generic { base, args, .. } => {
            if path_is(base, &["Result"]) && args.len() == 2 {
                return BodyType::Result {
                    ok: Box::new(body_type_from_type_expr(&args[0], http_aliases)),
                    error: Box::new(body_type_from_type_expr(&args[1], http_aliases)),
                };
            }

            if base.segments.len() == 2
                && http_aliases.contains(&base.segments[0])
                && base.segments[1] == "Context"
                && args.len() == 1
            {
                return BodyType::HttpContext(type_name(&args[0]));
            }

            BodyType::Named(type_name(ty))
        }
        TypeExpr::Option { inner, .. } => BodyType::Named(format!(
            "{}?",
            body_type_from_type_expr(inner, http_aliases).display()
        )),
    }
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

fn same_body_type(left: &BodyType, right: &BodyType) -> bool {
    if matches!(left, BodyType::Unknown) || matches!(right, BodyType::Unknown) {
        return true;
    }

    match (left, right) {
        (BodyType::Unit, BodyType::Unit)
        | (BodyType::Bool, BodyType::Bool)
        | (BodyType::String, BodyType::String)
        | (BodyType::Integer, BodyType::Integer)
        | (BodyType::Object, BodyType::Object)
        | (BodyType::HttpRequest, BodyType::HttpRequest)
        | (BodyType::HttpResponse, BodyType::HttpResponse) => true,
        (BodyType::Named(left), BodyType::Named(right))
        | (BodyType::HttpContext(left), BodyType::HttpContext(right)) => left == right,
        (
            BodyType::Result {
                ok: left_ok,
                error: left_error,
            },
            BodyType::Result {
                ok: right_ok,
                error: right_error,
            },
        ) => same_body_type(left_ok, right_ok) && same_body_type(left_error, right_error),
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

    #[test]
    fn rejects_unknown_fields_in_function_bodies() {
        let source = r#"
            use std.http as Http
            use std.validate

            type UserId = opaque U64

            struct AppState { users: UserStore }
            struct UserStore {}
            struct CreateUserInput {
                email: String
                name: String
            }
            struct User {
                id: UserId
                email: String
                name: String
            }
            enum ApiError { Failed }

            fn create(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {
                let input = req.json<CreateUserInput>()?
                ensure validate.email(input.emali), ApiError.Failed
                return Http.Response.created(input)
            }

            fn map_error(err: ApiError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                route POST "/users" -> create
            }
        "#;

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].code, "E_BODY_0002");
        assert_eq!(diagnostics[0].fixes[0].label, "did you mean `email`?");
    }

    #[test]
    fn rejects_question_on_non_result_values() {
        let source = r#"
            use std.http as Http

            struct AppState {}

            fn ping(ctx: Http.Context<AppState>, req: Http.Request) -> Http.Response {
                let response = Http.Response.ok({ "message": "pong" })?
                return response
            }

            service Api(state: AppState) {
                route GET "/ping" -> ping
            }
        "#;

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].code, "E_BODY_0005");
    }

    #[test]
    fn rejects_invalid_builtin_named_argument_types() {
        let source = r#"
            use std.http as Http
            use std.validate

            struct AppState {}
            struct CreateUserInput { name: String }
            enum ApiError { Failed }

            fn create(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {
                let input = req.json<CreateUserInput>()?
                ensure validate.length(input.name, min: "two", max: 80), ApiError.Failed
                return Http.Response.ok(input)
            }

            fn map_error(err: ApiError) -> Http.Response {}

            service Api(state: AppState) {
                use Http.error_map(map_error)
                route POST "/users" -> create
            }
        "#;

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].code, "E_BODY_0006");
    }

    #[test]
    fn rejects_invalid_local_function_argument_types() {
        let source = r#"
            struct CreateUserInput { name: String }
            enum ApiError { Failed }

            fn validate_user(input: CreateUserInput) -> Result<CreateUserInput, ApiError> {
                return input
            }

            fn bad() -> Result<CreateUserInput, ApiError> {
                return validate_user("Ada")?
            }
        "#;

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].code, "E_BODY_0006");
    }

    #[test]
    fn rejects_non_exhaustive_enum_match() {
        let source = r#"
            use std.http as Http

            enum ApiError {
                InvalidInput(message: String)
                NotFound
            }

            fn map_error(err: ApiError) -> Http.Response {
                match err {
                    InvalidInput(message) => return Http.Response.error(400, { "error": message })
                }
            }
        "#;

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].code, "E_BODY_0011");
    }

    #[test]
    fn rejects_ensure_with_non_bool_condition() {
        let source = r#"
            enum ApiError { Failed }

            fn validate() -> Result<String, ApiError> {
                ensure "yes", ApiError.Failed
                return "ok"
            }
        "#;

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].code, "E_BODY_0006");
    }

    #[test]
    fn rejects_invalid_enum_constructor_payloads() {
        let source = r#"
            enum ApiError {
                InvalidInput(message: String)
            }

            fn validate() -> Result<String, ApiError> {
                ensure false, ApiError.InvalidInput(400)
                return "ok"
            }
        "#;

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].code, "E_BODY_0006");
    }

    #[test]
    fn suggests_nearby_enum_variants() {
        let source = r#"
            enum ApiError {
                InvalidInput(message: String)
            }

            fn validate() -> Result<String, ApiError> {
                ensure false, ApiError.InvaldInput("bad")
                return "ok"
            }
        "#;

        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].code, "E_BODY_0012");
        assert_eq!(
            diagnostics[0].fixes[0].label,
            "did you mean `InvalidInput`?"
        );
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
