use super::{TypeChecker, path_is, path_name, type_name};
use are_ast::{
    CallArg, EnumDecl, Expr, FunctionBody, FunctionDecl, Item, ModelDecl, Path, Pattern, Stmt,
    StructDecl, TypeDecl, TypeExpr,
};
use are_diagnostics::{Diagnostic, SourceRange, best_name_suggestion};
use are_semantics::{
    Builtin, DbCall, DbOperation, builtin_by_callee, collection_name_for_model, db_call_by_callee,
};
use std::collections::{HashMap, HashSet};
use std::path::Path as FsPath;

impl TypeChecker<'_> {
    pub(super) fn check_function_body(&mut self, decl: &FunctionDecl) {
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
            http_responses: Vec::new(),
            diagnostics: Vec::new(),
        };

        body.check_statements(&block.statements, &decl.name);
        self.http_responses
            .insert(decl.name.clone(), body.http_responses.clone());
        self.diagnostics.extend(body.diagnostics);
    }

    pub(super) fn check_generic_arities(&mut self) {
        for item in &self.module.items {
            match item {
                Item::Use(_) => {}
                Item::Service(decl) => {
                    for route in &decl.routes {
                        if let Some(body_type) = &route.body_type {
                            self.check_type_expr_arity(body_type);
                        }
                        if let Some(query_type) = &route.query_type {
                            self.check_type_expr_arity(query_type);
                        }
                        if let Some(response_type) = &route.response_type {
                            self.check_type_expr_arity(response_type);
                        }
                    }
                }
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

    fn body_type_from_type_expr(&self, ty: &TypeExpr) -> BodyType {
        body_type_from_type_expr(ty, &self.http_aliases)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HttpResponseUse {
    pub status: Option<u16>,
    pub success: bool,
    pub body_type: BodyType,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BodyType {
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

    pub(super) fn display(&self) -> String {
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
    http_responses: Vec<HttpResponseUse>,
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

        if let Some(db_call) = db_call_by_callee(&callee_name) {
            return self.check_db_call(db_call, &callee_name, type_args, args, range);
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
        if !type_args.is_empty()
            && !matches!(
                builtin,
                Builtin::RequestJson | Builtin::RequestQuery | Builtin::ContextParam
            )
        {
            self.error(
                "E_BODY_0014",
                range,
                format!(
                    "`{callee_name}` does not accept type argument(s), got {}",
                    type_args.len()
                ),
                "only generic std calls such as `req.json<T>()` and `req.query<T>()` accept type arguments",
            );
        }

        match builtin {
            Builtin::HttpResponseOk | Builtin::HttpResponseCreated => {
                self.expect_positional_arity(callee_name, args, 1, range);
                let body_type = self.check_positional_arg(args, 0);
                self.http_responses.push(HttpResponseUse {
                    status: Some(match builtin {
                        Builtin::HttpResponseOk => 200,
                        Builtin::HttpResponseCreated => 201,
                        _ => unreachable!("covered by outer match arm"),
                    }),
                    success: true,
                    body_type,
                    range,
                });
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
                let body_type = self.check_positional_arg(args, 1);
                self.http_responses.push(HttpResponseUse {
                    status: integer_literal_arg(args, 0)
                        .and_then(|status| u16::try_from(status).ok()),
                    success: false,
                    body_type,
                    range,
                });
                BodyType::HttpResponse
            }
            Builtin::RequestJson | Builtin::RequestQuery => {
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
        }
    }

    fn check_db_call(
        &mut self,
        db_call: DbCall<'_>,
        callee_name: &str,
        type_args: &[TypeExpr],
        args: &[CallArg],
        range: SourceRange,
    ) -> BodyType {
        if !type_args.is_empty() {
            self.error(
                "E_BODY_0014",
                range,
                format!(
                    "`{callee_name}` does not accept type argument(s), got {}",
                    type_args.len()
                ),
                "model database calls infer their payload type from the collection name",
            );
        }

        let Some(model_name) = self.model_name_for_collection(db_call.collection) else {
            self.error(
                "E_BODY_0019",
                range,
                format!("unknown model collection `{}`", db_call.collection),
                "declare a model whose collection matches this ctx.db path, such as `model User` for `ctx.db.users`",
            );
            return BodyType::Unknown;
        };

        self.expect_positional_arity(callee_name, args, 1, range);
        self.check_positional_arg(args, 0);
        match db_call.operation {
            DbOperation::Insert | DbOperation::Get => self.result_type(BodyType::Named(model_name)),
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

        if self.return_accepts(return_type, value_type) {
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

    fn return_accepts(&self, expected: &BodyType, actual: &BodyType) -> bool {
        if same_body_type(expected, actual) {
            return true;
        }

        if self.named_payload_accepts_object(expected, actual) {
            return true;
        }

        match expected {
            BodyType::Result { ok, .. } => {
                same_body_type(ok, actual) || self.named_payload_accepts_object(ok, actual)
            }
            _ => false,
        }
    }

    fn named_payload_accepts_object(&self, expected: &BodyType, actual: &BodyType) -> bool {
        let (BodyType::Named(name), BodyType::Object) = (expected, actual) else {
            return false;
        };

        self.structs.contains_key(name) || self.models.contains_key(name)
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

    fn model_name_for_collection(&self, collection: &str) -> Option<String> {
        self.models
            .keys()
            .find(|model_name| collection_name_for_model(model_name) == collection)
            .cloned()
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

fn integer_literal_arg(args: &[CallArg], index: usize) -> Option<i64> {
    let arg = args.iter().filter(|arg| arg.label.is_none()).nth(index)?;
    let Expr::Integer { value, .. } = &arg.value else {
        return None;
    };
    Some(*value)
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
