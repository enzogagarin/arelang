use are_ast::{CallArg, Expr, FunctionBody, FunctionDecl, Path, Stmt, TypeExpr};
use serde_json::{Map, Value as JsonValue};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Json(JsonValue),
    HttpResponse(HttpResponseValue),
    Unit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponseValue {
    pub status: u16,
    pub body: JsonValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InterpretError {
    UnsupportedBody(String),
    MissingReturn(String),
    UnsupportedExpression(String),
    UnknownBinding(String),
    Arity {
        callee: String,
        expected: usize,
        actual: usize,
    },
    MissingArgument {
        callee: String,
        label: String,
    },
    ExpectedJson {
        context: String,
    },
    ExpectedString {
        context: String,
    },
    ExpectedInteger {
        context: String,
    },
    RaisedHttpResponse(HttpResponseValue),
}

impl std::fmt::Display for InterpretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedBody(function) => {
                write!(
                    f,
                    "`{function}` uses a body shape the interpreter cannot run yet"
                )
            }
            Self::MissingReturn(function) => {
                write!(f, "`{function}` completed without returning a value")
            }
            Self::UnsupportedExpression(expression) => {
                write!(f, "unsupported expression `{expression}`")
            }
            Self::UnknownBinding(name) => write!(f, "unknown binding `{name}`"),
            Self::Arity {
                callee,
                expected,
                actual,
            } => write!(
                f,
                "`{callee}` expected {expected} argument(s), got {actual}"
            ),
            Self::MissingArgument { callee, label } => {
                write!(f, "`{callee}` is missing argument `{label}`")
            }
            Self::ExpectedJson { context } => write!(f, "`{context}` expected a JSON value"),
            Self::ExpectedString { context } => write!(f, "`{context}` expected a string value"),
            Self::ExpectedInteger { context } => {
                write!(f, "`{context}` expected an integer value")
            }
            Self::RaisedHttpResponse(response) => {
                write!(f, "raised HTTP response with status {}", response.status)
            }
        }
    }
}

impl std::error::Error for InterpretError {}

impl InterpretError {
    #[must_use]
    pub fn raised_json_error(status: u16, error: &str) -> Self {
        Self::RaisedHttpResponse(HttpResponseValue {
            status,
            body: serde_json::json!({ "error": error }),
        })
    }

    #[must_use]
    pub const fn as_http_response(&self) -> Option<&HttpResponseValue> {
        match self {
            Self::RaisedHttpResponse(response) => Some(response),
            _ => None,
        }
    }
}

pub trait Host {
    /// Decode the current HTTP request body as JSON.
    ///
    /// # Errors
    ///
    /// Returns an error when the body cannot be decoded or the host has no
    /// request body available.
    fn read_json_body(&mut self, type_name: Option<&str>) -> Result<JsonValue, InterpretError>;

    /// Validate an email-like JSON string.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is not accepted by the host validator.
    fn validate_email(&mut self, value: &JsonValue) -> Result<(), InterpretError>;

    /// Validate the character length of a JSON string.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is not accepted by the host validator.
    fn validate_length(
        &mut self,
        value: &JsonValue,
        min: i64,
        max: i64,
    ) -> Result<(), InterpretError>;

    /// Insert a user-like JSON value into host state.
    ///
    /// # Errors
    ///
    /// Returns an error when the host cannot persist the value.
    fn insert_user(&mut self, input: JsonValue) -> Result<JsonValue, InterpretError>;

    /// Read a route path parameter by name.
    ///
    /// # Errors
    ///
    /// Returns an error when the parameter is missing or cannot be decoded as
    /// the requested type.
    fn read_path_param(
        &mut self,
        type_name: Option<&str>,
        name: &str,
    ) -> Result<JsonValue, InterpretError>;

    /// Read a user-like JSON value from host state.
    ///
    /// # Errors
    ///
    /// Returns an error when the id is invalid or the value does not exist.
    fn get_user(&mut self, id: JsonValue) -> Result<JsonValue, InterpretError>;
}

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
    Interpreter::new(host).interpret_function(function)
}

struct Interpreter<'a, H> {
    host: &'a mut H,
    env: HashMap<String, Value>,
}

impl<'a, H: Host> Interpreter<'a, H> {
    fn new(host: &'a mut H) -> Self {
        Self {
            host,
            env: HashMap::new(),
        }
    }

    fn interpret_function(&mut self, function: &FunctionDecl) -> Result<Value, InterpretError> {
        let FunctionBody::Parsed { block } = &function.body else {
            return Err(InterpretError::UnsupportedBody(function.name.clone()));
        };

        for statement in &block.statements {
            if let Some(value) = self.exec_stmt(statement)? {
                return Ok(value);
            }
        }

        Err(InterpretError::MissingReturn(function.name.clone()))
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
        }
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, InterpretError> {
        match expr {
            Expr::String { value, .. } => Ok(Value::Json(JsonValue::String(value.clone()))),
            Expr::Integer { value, .. } => Ok(Value::Json((*value).into())),
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
            } => self.eval_call(&callee.segments.join("."), type_args, args),
            Expr::Try { value, .. } => self.eval_expr(value),
            Expr::Path { path } => self.eval_path(path),
        }
    }

    fn eval_path(&self, path: &Path) -> Result<Value, InterpretError> {
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
        callee: &str,
        type_args: &[TypeExpr],
        args: &[CallArg],
    ) -> Result<Value, InterpretError> {
        match callee {
            "Http.Response.ok" => {
                let body = self.single_json_arg(callee, args)?;
                Ok(Value::HttpResponse(HttpResponseValue { status: 200, body }))
            }
            "Http.Response.created" => {
                let body = self.single_json_arg(callee, args)?;
                Ok(Value::HttpResponse(HttpResponseValue { status: 201, body }))
            }
            "req.json" => {
                Self::expect_arity(callee, args, 0)?;
                let type_name = type_args.first().and_then(type_expr_name);
                self.host
                    .read_json_body(type_name.as_deref())
                    .map(Value::Json)
            }
            "validate.email" => {
                let value = self.single_json_arg(callee, args)?;
                self.host.validate_email(&value)?;
                Ok(Value::Unit)
            }
            "validate.length" => {
                let value = self.positional_json_arg(callee, args, 0, 1)?;
                let min = self.named_i64_arg(callee, args, "min")?;
                let max = self.named_i64_arg(callee, args, "max")?;
                self.host.validate_length(&value, min, max)?;
                Ok(Value::Unit)
            }
            "ctx.state.users.insert" => {
                let input = self.single_json_arg(callee, args)?;
                self.host.insert_user(input).map(Value::Json)
            }
            "ctx.param" => {
                let name = self.single_string_arg(callee, args)?;
                let type_name = type_args.first().and_then(type_expr_name);
                self.host
                    .read_path_param(type_name.as_deref(), &name)
                    .map(Value::Json)
            }
            "ctx.state.users.get" => {
                let id = self.single_json_arg(callee, args)?;
                self.host.get_user(id).map(Value::Json)
            }
            _ => Err(InterpretError::UnsupportedExpression(callee.to_string())),
        }
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
}

struct NoopHost;

impl Host for NoopHost {
    fn read_json_body(&mut self, _type_name: Option<&str>) -> Result<JsonValue, InterpretError> {
        Err(InterpretError::UnsupportedExpression("req.json".into()))
    }

    fn validate_email(&mut self, _value: &JsonValue) -> Result<(), InterpretError> {
        Err(InterpretError::UnsupportedExpression(
            "validate.email".into(),
        ))
    }

    fn validate_length(
        &mut self,
        _value: &JsonValue,
        _min: i64,
        _max: i64,
    ) -> Result<(), InterpretError> {
        Err(InterpretError::UnsupportedExpression(
            "validate.length".into(),
        ))
    }

    fn insert_user(&mut self, _input: JsonValue) -> Result<JsonValue, InterpretError> {
        Err(InterpretError::UnsupportedExpression(
            "ctx.state.users.insert".into(),
        ))
    }

    fn read_path_param(
        &mut self,
        _type_name: Option<&str>,
        _name: &str,
    ) -> Result<JsonValue, InterpretError> {
        Err(InterpretError::UnsupportedExpression("ctx.param".into()))
    }

    fn get_user(&mut self, _id: JsonValue) -> Result<JsonValue, InterpretError> {
        Err(InterpretError::UnsupportedExpression(
            "ctx.state.users.get".into(),
        ))
    }
}

fn type_expr_name(ty: &TypeExpr) -> Option<String> {
    match ty {
        TypeExpr::Path { path } => Some(path.segments.join(".")),
        TypeExpr::Generic { base, .. } => Some(base.segments.join(".")),
        TypeExpr::Option { inner, .. } => type_expr_name(inner),
    }
}

fn expect_json(value: Value, context: &str) -> Result<JsonValue, InterpretError> {
    match value {
        Value::Json(value) => Ok(value),
        Value::HttpResponse(_) | Value::Unit => Err(InterpretError::ExpectedJson {
            context: context.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Host, HttpResponseValue, InterpretError, Value, interpret_function,
        interpret_function_with_host,
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
        assert_eq!(
            value,
            Value::HttpResponse(HttpResponseValue {
                status: 200,
                body: serde_json::json!({ "status": "ok" }),
            })
        );
    }

    #[test]
    fn interprets_create_user_flow_with_host_effects() {
        let create_user = users_api_function("create_user");
        let mut host = TestHost::new(r#"{"email":"ada@example.com","name":"Ada"}"#);

        let value =
            interpret_function_with_host(&create_user, &mut host).expect("create_user runs");

        assert_eq!(
            value,
            Value::HttpResponse(HttpResponseValue {
                status: 201,
                body: serde_json::json!({
                    "id": 1,
                    "email": "ada@example.com",
                    "name": "Ada",
                }),
            })
        );
    }

    #[test]
    fn propagates_create_user_validation_errors() {
        let create_user = users_api_function("create_user");
        let mut host = TestHost::new(r#"{"email":"invalid","name":"Ada"}"#);

        let err = interpret_function_with_host(&create_user, &mut host).expect_err("email fails");
        let response = err.as_http_response().expect("validation maps to HTTP");

        assert_eq!(response.status, 400);
        assert_eq!(
            response.body,
            serde_json::json!({ "error": "invalid_email" })
        );
    }

    #[test]
    fn interprets_get_user_flow_with_host_effects() {
        let create_user = users_api_function("create_user");
        let get_user = users_api_function("get_user");
        let mut host = TestHost::new(r#"{"email":"ada@example.com","name":"Ada"}"#);

        interpret_function_with_host(&create_user, &mut host).expect("create_user runs");
        host.set_path_param("id", "1");
        let value = interpret_function_with_host(&get_user, &mut host).expect("get_user runs");

        assert_eq!(
            value,
            Value::HttpResponse(HttpResponseValue {
                status: 200,
                body: serde_json::json!({
                    "id": 1,
                    "email": "ada@example.com",
                    "name": "Ada",
                }),
            })
        );
    }

    #[test]
    fn propagates_get_user_not_found_errors() {
        let get_user = users_api_function("get_user");
        let mut host = TestHost::new("");
        host.set_path_param("id", "42");

        let err = interpret_function_with_host(&get_user, &mut host).expect_err("user missing");
        let response = err.as_http_response().expect("not found maps to HTTP");

        assert_eq!(response.status, 404);
        assert_eq!(response.body, serde_json::json!({ "error": "not_found" }));
    }

    fn users_api_function(name: &str) -> FunctionDecl {
        let source = include_str!("../../../examples/users_api/main.are");
        let file = Path::new("examples/users_api/main.are");
        let (tokens, lex_diagnostics) = lex_source(file, source);
        assert!(lex_diagnostics.is_empty(), "{lex_diagnostics:#?}");
        let (module, parse_diagnostics) = parse_tokens(file, &tokens);
        assert!(parse_diagnostics.is_empty(), "{parse_diagnostics:#?}");
        let module = module.expect("module parses");

        module
            .items
            .iter()
            .find_map(|item| {
                if let Item::Function(function) = item {
                    (function.name == name).then_some(function.clone())
                } else {
                    None
                }
            })
            .expect("function exists")
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
                .map_err(|_| InterpretError::raised_json_error(400, "invalid_json"))?;
            if type_name == Some("CreateUserInput")
                && !(value.get("email").is_some_and(serde_json::Value::is_string)
                    && value.get("name").is_some_and(serde_json::Value::is_string))
            {
                return Err(InterpretError::raised_json_error(400, "invalid_json"));
            }

            Ok(value)
        }

        fn validate_email(&mut self, value: &serde_json::Value) -> Result<(), InterpretError> {
            if value.as_str().is_some_and(|email| email.contains('@')) {
                return Ok(());
            }

            Err(InterpretError::raised_json_error(400, "invalid_email"))
        }

        fn validate_length(
            &mut self,
            value: &serde_json::Value,
            min: i64,
            max: i64,
        ) -> Result<(), InterpretError> {
            let Some(text) = value.as_str() else {
                return Err(InterpretError::raised_json_error(400, "invalid_name"));
            };
            let len = i64::try_from(text.chars().count()).map_err(|_| {
                InterpretError::UnsupportedExpression("test string too long".into())
            })?;
            if (min..=max).contains(&len) {
                return Ok(());
            }

            Err(InterpretError::raised_json_error(400, "invalid_name"))
        }

        fn insert_user(
            &mut self,
            input: serde_json::Value,
        ) -> Result<serde_json::Value, InterpretError> {
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
                return Err(InterpretError::raised_json_error(400, "missing_id"));
            };

            if type_name == Some("UserId") {
                let id = value
                    .parse::<u64>()
                    .map_err(|_| InterpretError::raised_json_error(400, "invalid_id"))?;
                return Ok(serde_json::json!(id));
            }

            Ok(serde_json::Value::String(value.clone()))
        }

        fn get_user(&mut self, id: serde_json::Value) -> Result<serde_json::Value, InterpretError> {
            let Some(id) = id.as_u64() else {
                return Err(InterpretError::raised_json_error(400, "invalid_id"));
            };

            self.users
                .get(&id)
                .cloned()
                .ok_or_else(|| InterpretError::raised_json_error(404, "not_found"))
        }
    }
}
