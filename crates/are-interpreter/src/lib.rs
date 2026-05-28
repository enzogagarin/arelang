use are_ast::{CallArg, Expr, FunctionBody, FunctionDecl, Path, Pattern, Stmt, TypeExpr};

mod error;
mod host;
mod value;

use are_semantics::{Builtin, DbCall, DbOperation, builtin_by_callee, db_call_by_callee};
pub use error::InterpretError;
pub use host::Host;
use host::NoopHost;
use serde_json::{Map, Value as JsonValue};
use std::collections::HashMap;
use std::hash::BuildHasher;
pub use value::{EnumValue, HttpResponseValue, Value};

/// Interpret an Arelang function body.
///
/// # Errors
///
/// Returns an error when the function uses syntax or runtime values that this
/// MVP interpreter does not support yet.
pub fn interpret_function(function: &FunctionDecl) -> Result<Value, InterpretError> {
    let mut host = NoopHost;
    interpret_function_with_host(function, &mut host)
}

/// Interpret an Arelang function body with host-provided backend effects.
///
/// # Errors
///
/// Returns an error when the function uses unsupported syntax, raises an
/// application response, or calls a host operation that fails.
pub fn interpret_function_with_host(
    function: &FunctionDecl,
    host: &mut impl Host,
) -> Result<Value, InterpretError> {
    let functions: HashMap<String, FunctionDecl> = HashMap::new();
    Interpreter::new(host, &functions).interpret_function(function)
}

/// Interpret an Arelang function body with access to local functions.
///
/// # Errors
///
/// Returns an error when the function uses unsupported syntax, raises an
/// application error, or calls a host operation that fails.
pub fn interpret_function_with_host_and_functions<S: BuildHasher>(
    function: &FunctionDecl,
    functions: &HashMap<String, FunctionDecl, S>,
    host: &mut impl Host,
) -> Result<Value, InterpretError> {
    Interpreter::new(host, functions).interpret_function(function)
}

/// Interpret an Arelang function body with explicit positional arguments.
///
/// # Errors
///
/// Returns an error when argument binding fails, the function uses unsupported
/// syntax, raises an application error, or calls a host operation that fails.
pub fn interpret_function_with_host_and_args<S: BuildHasher>(
    function: &FunctionDecl,
    functions: &HashMap<String, FunctionDecl, S>,
    host: &mut impl Host,
    args: Vec<Value>,
) -> Result<Value, InterpretError> {
    Interpreter::new(host, functions).interpret_function_with_args(function, args)
}

struct Interpreter<'a, H, S> {
    host: &'a mut H,
    functions: &'a HashMap<String, FunctionDecl, S>,
    env: HashMap<String, Value>,
}

impl<'a, H: Host, S: BuildHasher> Interpreter<'a, H, S> {
    fn new(host: &'a mut H, functions: &'a HashMap<String, FunctionDecl, S>) -> Self {
        Self {
            host,
            functions,
            env: HashMap::new(),
        }
    }

    fn interpret_function(&mut self, function: &FunctionDecl) -> Result<Value, InterpretError> {
        self.interpret_function_with_args(function, Vec::new())
    }

    fn interpret_function_with_args(
        &mut self,
        function: &FunctionDecl,
        args: Vec<Value>,
    ) -> Result<Value, InterpretError> {
        if args.len() != function.params.len() && !args.is_empty() {
            return Err(InterpretError::Arity {
                callee: function.name.clone(),
                expected: function.params.len(),
                actual: args.len(),
            });
        }

        let FunctionBody::Parsed { block } = &function.body else {
            return Err(InterpretError::UnsupportedBody(function.name.clone()));
        };

        let previous_env = std::mem::take(&mut self.env);
        for (param, value) in function.params.iter().zip(args) {
            self.env.insert(param.name.clone(), value);
        }

        let mut result = Err(InterpretError::MissingReturn(function.name.clone()));
        for statement in &block.statements {
            match self.exec_stmt(statement) {
                Ok(Some(value)) => {
                    result = Ok(value);
                    break;
                }
                Ok(None) => {}
                Err(err) => {
                    result = Err(err);
                    break;
                }
            }
        }
        self.env = previous_env;
        result
    }

    fn exec_stmt(&mut self, statement: &Stmt) -> Result<Option<Value>, InterpretError> {
        match statement {
            Stmt::Let { name, value, .. } => {
                let value = self.eval_expr(value)?;
                self.env.insert(name.clone(), value);
                Ok(None)
            }
            Stmt::Expr { value, .. } => {
                self.eval_expr(value)?;
                Ok(None)
            }
            Stmt::Return { value, .. } => self.eval_expr(value).map(Some),
            Stmt::Ensure {
                condition, error, ..
            } => self.exec_ensure(condition, error),
            Stmt::Match { value, arms, .. } => self.exec_match(value, arms),
        }
    }

    fn exec_ensure(
        &mut self,
        condition: &Expr,
        error: &Expr,
    ) -> Result<Option<Value>, InterpretError> {
        let condition = self.eval_expr(condition)?;
        if expect_bool(&condition, "ensure")? {
            return Ok(None);
        }

        let error = expect_enum(self.eval_expr(error)?, "ensure")?;
        Err(InterpretError::RaisedError(error))
    }

    fn exec_match(
        &mut self,
        value: &Expr,
        arms: &[are_ast::MatchArm],
    ) -> Result<Option<Value>, InterpretError> {
        let value = self.eval_expr(value)?;
        let Value::Enum(error) = value else {
            return Err(InterpretError::ExpectedEnum {
                context: "match".to_string(),
            });
        };

        let Some(arm) = arms.iter().find(|arm| match &arm.pattern {
            Pattern::Variant { name, .. } => *name == error.variant,
        }) else {
            return Err(InterpretError::UnmatchedPattern {
                variant: error.variant,
            });
        };

        let Pattern::Variant { bindings, .. } = &arm.pattern;
        let previous = self.bind_match_payload(bindings, &error.payload);
        let result = self.exec_stmt(&arm.body);
        self.restore_bindings(previous);
        result
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, InterpretError> {
        match expr {
            Expr::String { value, .. } => Ok(Value::Json(JsonValue::String(value.clone()))),
            Expr::Integer { value, .. } => Ok(Value::Json((*value).into())),
            Expr::Bool { value, .. } => Ok(Value::Bool(*value)),
            Expr::Object { fields, .. } => {
                let mut object = Map::new();
                for field in fields {
                    object.insert(
                        field.key.clone(),
                        expect_json(self.eval_expr(&field.value)?, "object")?,
                    );
                }
                Ok(Value::Json(JsonValue::Object(object)))
            }
            Expr::Call {
                callee,
                type_args,
                args,
                ..
            } => self.eval_call(callee, type_args, args),
            Expr::Try { value, .. } => self.eval_expr(value),
            Expr::Path { path } => self.eval_path(path),
        }
    }

    fn eval_path(&self, path: &Path) -> Result<Value, InterpretError> {
        if let Some(value) = enum_value_from_path(path) {
            return Ok(value);
        }

        let Some(first) = path.segments.first() else {
            return Err(InterpretError::UnsupportedExpression(String::new()));
        };

        let mut value = self
            .env
            .get(first)
            .cloned()
            .ok_or_else(|| InterpretError::UnknownBinding(first.clone()))?;

        for segment in path.segments.iter().skip(1) {
            value = match value {
                Value::Json(JsonValue::Object(object)) => object
                    .get(segment)
                    .cloned()
                    .map(Value::Json)
                    .ok_or_else(|| InterpretError::UnknownBinding(path.segments.join(".")))?,
                _ => {
                    return Err(InterpretError::ExpectedJson {
                        context: path.segments.join("."),
                    });
                }
            };
        }

        Ok(value)
    }

    fn eval_call(
        &mut self,
        callee: &Path,
        type_args: &[TypeExpr],
        args: &[CallArg],
    ) -> Result<Value, InterpretError> {
        let callee_name = callee.segments.join(".");

        match builtin_by_callee(&callee_name) {
            Some(Builtin::HttpResponseOk) => {
                let body = self.single_json_arg(&callee_name, args)?;
                Ok(Value::HttpResponse(HttpResponseValue { status: 200, body }))
            }
            Some(Builtin::HttpResponseCreated) => {
                let body = self.single_json_arg(&callee_name, args)?;
                Ok(Value::HttpResponse(HttpResponseValue { status: 201, body }))
            }
            Some(Builtin::HttpResponseError) => {
                let status = self.positional_i64_arg(&callee_name, args, 0, 2)?;
                let status =
                    u16::try_from(status).map_err(|_| InterpretError::ExpectedInteger {
                        context: callee_name.clone(),
                    })?;
                let body = self.positional_json_arg(&callee_name, args, 1, 2)?;
                Ok(Value::HttpResponse(HttpResponseValue { status, body }))
            }
            Some(Builtin::RequestJson) => {
                Self::expect_arity(&callee_name, args, 0)?;
                let type_name = type_args.first().and_then(type_expr_name);
                self.host
                    .read_json_body(type_name.as_deref())
                    .map(Value::Json)
            }
            Some(Builtin::RequestQuery) => {
                Self::expect_arity(&callee_name, args, 0)?;
                let type_name = type_args.first().and_then(type_expr_name);
                self.host
                    .read_query_params(type_name.as_deref())
                    .map(Value::Json)
            }
            Some(Builtin::RequestHeaders) => {
                Self::expect_arity(&callee_name, args, 0)?;
                let type_name = type_args.first().and_then(type_expr_name);
                self.host
                    .read_headers(type_name.as_deref())
                    .map(Value::Json)
            }
            Some(Builtin::RequestCookies) => {
                Self::expect_arity(&callee_name, args, 0)?;
                let type_name = type_args.first().and_then(type_expr_name);
                self.host
                    .read_cookies(type_name.as_deref())
                    .map(Value::Json)
            }
            Some(Builtin::ValidateEmail) => {
                let value = self.single_json_arg(&callee_name, args)?;
                self.host.validate_email(&value).map(Value::Bool)
            }
            Some(Builtin::ValidateLength) => {
                let value = self.positional_json_arg(&callee_name, args, 0, 1)?;
                let min = self.named_i64_arg(&callee_name, args, "min")?;
                let max = self.named_i64_arg(&callee_name, args, "max")?;
                self.host.validate_length(&value, min, max).map(Value::Bool)
            }
            Some(Builtin::ContextParam) => {
                let name = self.single_string_arg(&callee_name, args)?;
                let type_name = type_args.first().and_then(type_expr_name);
                self.host
                    .read_path_param(type_name.as_deref(), &name)
                    .map(Value::Json)
            }
            None => {
                if let Some(db_call) = db_call_by_callee(&callee_name) {
                    return self.eval_db_call(db_call, &callee_name, args);
                }

                if let Some(value) = self.eval_enum_constructor(callee, args)? {
                    return Ok(value);
                }

                self.eval_user_function_call(&callee_name, args)
            }
        }
    }

    fn eval_db_call(
        &mut self,
        db_call: DbCall<'_>,
        callee_name: &str,
        args: &[CallArg],
    ) -> Result<Value, InterpretError> {
        let value = self.single_json_arg(callee_name, args)?;
        match db_call.operation {
            DbOperation::Insert => self
                .host
                .insert_model(db_call.collection, value)
                .map(Value::Json),
            DbOperation::Get => self
                .host
                .get_model(db_call.collection, value)
                .map(Value::Json),
        }
    }

    fn eval_enum_constructor(
        &mut self,
        callee: &Path,
        args: &[CallArg],
    ) -> Result<Option<Value>, InterpretError> {
        let [enum_name, variant] = callee.segments.as_slice() else {
            return Ok(None);
        };

        if let Some(arg) = args.iter().find(|arg| arg.label.is_some()) {
            return Err(InterpretError::UnsupportedExpression(format!(
                "{}:{}",
                callee.segments.join("."),
                arg.label.as_deref().unwrap_or_default()
            )));
        }

        let payload = args
            .iter()
            .map(|arg| self.eval_expr(&arg.value))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(Value::Enum(EnumValue {
            enum_name: enum_name.clone(),
            variant: variant.clone(),
            payload,
        })))
    }

    fn eval_user_function_call(
        &mut self,
        callee: &str,
        args: &[CallArg],
    ) -> Result<Value, InterpretError> {
        let Some(function) = self.functions.get(callee).cloned() else {
            return Err(InterpretError::UnsupportedExpression(callee.to_string()));
        };

        let positional = args
            .iter()
            .filter(|arg| arg.label.is_none())
            .collect::<Vec<_>>();
        if positional.len() != function.params.len() {
            return Err(InterpretError::Arity {
                callee: callee.to_string(),
                expected: function.params.len(),
                actual: positional.len(),
            });
        }

        if let Some(arg) = args.iter().find(|arg| arg.label.is_some()) {
            return Err(InterpretError::UnsupportedExpression(format!(
                "{}:{}",
                callee,
                arg.label.as_deref().unwrap_or_default()
            )));
        }

        let values = positional
            .iter()
            .map(|arg| self.eval_expr(&arg.value))
            .collect::<Result<Vec<_>, _>>()?;
        self.interpret_function_with_args(&function, values)
    }

    fn expect_arity(callee: &str, args: &[CallArg], expected: usize) -> Result<(), InterpretError> {
        if args.len() == expected {
            return Ok(());
        }

        Err(InterpretError::Arity {
            callee: callee.to_string(),
            expected,
            actual: args.len(),
        })
    }

    fn single_json_arg(
        &mut self,
        callee: &str,
        args: &[CallArg],
    ) -> Result<JsonValue, InterpretError> {
        self.positional_json_arg(callee, args, 0, 1)
    }

    fn single_string_arg(
        &mut self,
        callee: &str,
        args: &[CallArg],
    ) -> Result<String, InterpretError> {
        let value = self.single_json_arg(callee, args)?;
        value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| InterpretError::ExpectedString {
                context: callee.to_string(),
            })
    }

    fn positional_json_arg(
        &mut self,
        callee: &str,
        args: &[CallArg],
        index: usize,
        expected: usize,
    ) -> Result<JsonValue, InterpretError> {
        let positional = args
            .iter()
            .filter(|arg| arg.label.is_none())
            .collect::<Vec<_>>();
        if positional.len() != expected {
            return Err(InterpretError::Arity {
                callee: callee.to_string(),
                expected,
                actual: positional.len(),
            });
        }

        let value = positional.get(index).ok_or_else(|| InterpretError::Arity {
            callee: callee.to_string(),
            expected,
            actual: positional.len(),
        })?;

        expect_json(self.eval_expr(&value.value)?, callee)
    }

    fn named_i64_arg(
        &mut self,
        callee: &str,
        args: &[CallArg],
        label: &str,
    ) -> Result<i64, InterpretError> {
        let arg = args
            .iter()
            .find(|arg| arg.label.as_deref() == Some(label))
            .ok_or_else(|| InterpretError::MissingArgument {
                callee: callee.to_string(),
                label: label.to_string(),
            })?;

        let value = expect_json(self.eval_expr(&arg.value)?, label)?;
        value
            .as_i64()
            .ok_or_else(|| InterpretError::ExpectedInteger {
                context: label.to_string(),
            })
    }

    fn positional_i64_arg(
        &mut self,
        callee: &str,
        args: &[CallArg],
        index: usize,
        expected: usize,
    ) -> Result<i64, InterpretError> {
        let value = self.positional_json_arg(callee, args, index, expected)?;
        value
            .as_i64()
            .ok_or_else(|| InterpretError::ExpectedInteger {
                context: callee.to_string(),
            })
    }

    fn bind_match_payload(
        &mut self,
        bindings: &[String],
        payload: &[Value],
    ) -> Vec<(String, Option<Value>)> {
        let mut previous = Vec::new();

        for (binding, value) in bindings.iter().zip(payload) {
            previous.push((
                binding.clone(),
                self.env.insert(binding.clone(), value.clone()),
            ));
        }

        previous
    }

    fn restore_bindings(&mut self, previous: Vec<(String, Option<Value>)>) {
        for (name, value) in previous {
            if let Some(value) = value {
                self.env.insert(name, value);
            } else {
                self.env.remove(&name);
            }
        }
    }
}

fn type_expr_name(ty: &TypeExpr) -> Option<String> {
    match ty {
        TypeExpr::Path { path } => Some(path.segments.join(".")),
        TypeExpr::Generic { base, .. } => Some(base.segments.join(".")),
        TypeExpr::Option { inner, .. } => type_expr_name(inner),
    }
}

fn enum_value_from_path(path: &Path) -> Option<Value> {
    let [enum_name, variant] = path.segments.as_slice() else {
        return None;
    };

    if !enum_name
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
    {
        return None;
    }

    Some(Value::Enum(EnumValue {
        enum_name: enum_name.clone(),
        variant: variant.clone(),
        payload: Vec::new(),
    }))
}

fn expect_json(value: Value, context: &str) -> Result<JsonValue, InterpretError> {
    match value {
        Value::Json(value) => Ok(value),
        Value::Bool(value) => Ok(JsonValue::Bool(value)),
        Value::HttpResponse(_) | Value::Enum(_) | Value::Unit => {
            Err(InterpretError::ExpectedJson {
                context: context.to_string(),
            })
        }
    }
}

fn expect_bool(value: &Value, context: &str) -> Result<bool, InterpretError> {
    match value {
        Value::Bool(value) | Value::Json(JsonValue::Bool(value)) => Ok(*value),
        Value::Json(_) | Value::HttpResponse(_) | Value::Enum(_) | Value::Unit => {
            Err(InterpretError::ExpectedBool {
                context: context.to_string(),
            })
        }
    }
}

fn expect_enum(value: Value, context: &str) -> Result<EnumValue, InterpretError> {
    match value {
        Value::Enum(value) => Ok(value),
        Value::Json(_) | Value::Bool(_) | Value::HttpResponse(_) | Value::Unit => {
            Err(InterpretError::ExpectedEnum {
                context: context.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Host, InterpretError, Value, interpret_function, interpret_function_with_host_and_args,
    };
    use are_ast::{FunctionDecl, Item};
    use are_lexer::lex_source;
    use are_parser::parse_tokens;
    use std::collections::HashMap;
    use std::path::Path;

    #[test]
    fn interprets_health_response_from_arelang_body() {
        let health = users_api_function("health");
        let value = interpret_function(&health).expect("health interprets");
        assert_eq!(value, Value::Json(serde_json::json!({ "status": "ok" })));
    }

    #[test]
    fn interprets_create_user_flow_with_host_effects() {
        let mut host = TestHost::new(r#"{"email":"ada@example.com","name":"Ada"}"#);

        let value =
            interpret_users_api_function("create_user", &mut host).expect("create_user runs");

        assert_eq!(
            value,
            Value::Json(serde_json::json!({
                "id": 1,
                "email": "ada@example.com",
                "name": "Ada",
            }))
        );
    }

    #[test]
    fn interprets_manual_validation_builtins() {
        let functions = functions_from_source(
            "validation.are",
            r#"
                use std.validate

                fn valid_email() -> Bool {
                    return validate.email("ada@example.com")
                }

                fn valid_length() -> Bool {
                    return validate.length("Ada", min: 2, max: 80)
                }
            "#,
        );
        let mut host = TestHost::new("");

        let email = interpret_function_with_host_and_args(
            functions.get("valid_email").expect("function exists"),
            &functions,
            &mut host,
            Vec::new(),
        )
        .expect("email validation runs");
        let length = interpret_function_with_host_and_args(
            functions.get("valid_length").expect("function exists"),
            &functions,
            &mut host,
            Vec::new(),
        )
        .expect("length validation runs");

        assert_eq!(email, Value::Bool(true));
        assert_eq!(length, Value::Bool(true));
    }

    #[test]
    fn interprets_get_user_flow_with_host_effects() {
        let mut host = TestHost::new(r#"{"email":"ada@example.com","name":"Ada"}"#);

        interpret_users_api_function("create_user", &mut host).expect("create_user runs");
        host.set_path_param("id", "1");
        let value = interpret_users_api_function("get_user", &mut host).expect("get_user runs");

        assert_eq!(
            value,
            Value::Json(serde_json::json!({
                "id": 1,
                "email": "ada@example.com",
                "name": "Ada",
            }))
        );
    }

    #[test]
    fn propagates_get_user_not_found_errors() {
        let mut host = TestHost::new("");
        host.set_path_param("id", "42");

        let err = interpret_users_api_function("get_user", &mut host).expect_err("user missing");
        let error = err.as_raised_error().expect("not found raises ApiError");

        assert_eq!(error.enum_name, "ApiError");
        assert_eq!(error.variant, "NotFound");
        assert!(error.payload.is_empty());
    }

    fn users_api_function(name: &str) -> FunctionDecl {
        let mut functions = users_api_functions();
        functions.remove(name).expect("function exists")
    }

    fn users_api_functions() -> HashMap<String, FunctionDecl> {
        let source = include_str!("../../../examples/users_api/main.are");
        functions_from_source("examples/users_api/main.are", source)
    }

    fn functions_from_source(file_name: &str, source: &str) -> HashMap<String, FunctionDecl> {
        let file = Path::new(file_name);
        let (tokens, lex_diagnostics) = lex_source(file, source);
        assert!(lex_diagnostics.is_empty(), "{lex_diagnostics:#?}");
        let (module, parse_diagnostics) = parse_tokens(file, &tokens);
        assert!(parse_diagnostics.is_empty(), "{parse_diagnostics:#?}");
        let module = module.expect("module parses");

        module
            .items
            .into_iter()
            .filter_map(|item| {
                if let Item::Function(function) = item {
                    Some((function.name.clone(), function))
                } else {
                    None
                }
            })
            .collect()
    }

    fn interpret_users_api_function(
        name: &str,
        host: &mut TestHost,
    ) -> Result<Value, InterpretError> {
        let functions = users_api_functions();
        let function = functions.get(name).expect("function exists");
        let args = users_api_function_args(name, host);
        interpret_function_with_host_and_args(function, &functions, host, args)
    }

    fn users_api_function_args(name: &str, host: &TestHost) -> Vec<Value> {
        match name {
            "create_user" => vec![
                Value::Unit,
                Value::Json(
                    serde_json::from_str(&host.request_body).expect("request body json decodes"),
                ),
            ],
            "get_user" => vec![
                Value::Unit,
                Value::Json(serde_json::Value::from(
                    host.path_params
                        .get("id")
                        .expect("id path param")
                        .parse::<u64>()
                        .expect("id path param is u64"),
                )),
            ],
            _ => Vec::new(),
        }
    }

    struct TestHost {
        request_body: String,
        next_id: u64,
        path_params: HashMap<String, String>,
        users: HashMap<u64, serde_json::Value>,
    }

    impl TestHost {
        fn new(request_body: &str) -> Self {
            Self {
                request_body: request_body.to_string(),
                next_id: 0,
                path_params: HashMap::new(),
                users: HashMap::new(),
            }
        }

        fn set_path_param(&mut self, name: &str, value: &str) {
            self.path_params.insert(name.to_string(), value.to_string());
        }
    }

    impl Host for TestHost {
        fn read_json_body(
            &mut self,
            type_name: Option<&str>,
        ) -> Result<serde_json::Value, InterpretError> {
            let value = serde_json::from_str::<serde_json::Value>(&self.request_body)
                .map_err(|_| api_invalid_input("invalid_json"))?;
            if type_name == Some("CreateUserInput")
                && !(value.get("email").is_some_and(serde_json::Value::is_string)
                    && value.get("name").is_some_and(serde_json::Value::is_string))
            {
                return Err(api_invalid_input("invalid_json"));
            }

            Ok(value)
        }

        fn read_query_params(
            &mut self,
            _type_name: Option<&str>,
        ) -> Result<serde_json::Value, InterpretError> {
            Err(InterpretError::UnsupportedExpression("req.query".into()))
        }

        fn read_headers(
            &mut self,
            _type_name: Option<&str>,
        ) -> Result<serde_json::Value, InterpretError> {
            Err(InterpretError::UnsupportedExpression("req.headers".into()))
        }

        fn read_cookies(
            &mut self,
            _type_name: Option<&str>,
        ) -> Result<serde_json::Value, InterpretError> {
            Err(InterpretError::UnsupportedExpression("req.cookies".into()))
        }

        fn validate_email(&mut self, value: &serde_json::Value) -> Result<bool, InterpretError> {
            Ok(value.as_str().is_some_and(|email| email.contains('@')))
        }

        fn validate_length(
            &mut self,
            value: &serde_json::Value,
            min: i64,
            max: i64,
        ) -> Result<bool, InterpretError> {
            let Some(text) = value.as_str() else {
                return Ok(false);
            };
            let len = i64::try_from(text.chars().count()).map_err(|_| {
                InterpretError::UnsupportedExpression("test string too long".into())
            })?;
            Ok((min..=max).contains(&len))
        }

        fn insert_model(
            &mut self,
            collection: &str,
            input: serde_json::Value,
        ) -> Result<serde_json::Value, InterpretError> {
            if collection != "users" {
                return Err(InterpretError::UnsupportedExpression(format!(
                    "ctx.db.{collection}.insert"
                )));
            }

            self.next_id += 1;
            let user = serde_json::json!({
                "id": self.next_id,
                "email": input["email"],
                "name": input["name"],
            });
            self.users.insert(self.next_id, user.clone());
            Ok(user)
        }

        fn read_path_param(
            &mut self,
            type_name: Option<&str>,
            name: &str,
        ) -> Result<serde_json::Value, InterpretError> {
            let Some(value) = self.path_params.get(name) else {
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

        fn get_model(
            &mut self,
            collection: &str,
            id: serde_json::Value,
        ) -> Result<serde_json::Value, InterpretError> {
            if collection != "users" {
                return Err(InterpretError::UnsupportedExpression(format!(
                    "ctx.db.{collection}.get"
                )));
            }

            let Some(id) = id.as_u64() else {
                return Err(api_invalid_input("invalid_id"));
            };

            self.users
                .get(&id)
                .cloned()
                .ok_or_else(|| InterpretError::raised_api_error("NotFound", Vec::new()))
        }
    }

    fn api_invalid_input(message: &str) -> InterpretError {
        InterpretError::raised_api_error(
            "InvalidInput",
            vec![serde_json::Value::String(message.to_string())],
        )
    }
}
