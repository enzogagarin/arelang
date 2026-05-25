use are_ast::{Expr, FunctionBody, FunctionDecl, Stmt};
use serde_json::{Map, Value as JsonValue};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Json(JsonValue),
    HttpResponse(HttpResponseValue),
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponseValue {
    pub status: u16,
    pub body: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterpretError {
    UnsupportedBody(String),
    MissingReturn(String),
    UnsupportedExpression(String),
    Arity {
        callee: String,
        expected: usize,
        actual: usize,
    },
    ExpectedJson {
        context: String,
    },
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
            Self::Arity {
                callee,
                expected,
                actual,
            } => write!(
                f,
                "`{callee}` expected {expected} argument(s), got {actual}"
            ),
            Self::ExpectedJson { context } => write!(f, "`{context}` expected a JSON value"),
        }
    }
}

impl std::error::Error for InterpretError {}

/// Interpret an Arelang function body.
///
/// # Errors
///
/// Returns an error when the function uses syntax or runtime values that this
/// MVP interpreter does not support yet.
pub fn interpret_function(function: &FunctionDecl) -> Result<Value, InterpretError> {
    let FunctionBody::Parsed { block } = &function.body else {
        return Err(InterpretError::UnsupportedBody(function.name.clone()));
    };

    let Some(statement) = block.statements.first() else {
        return Err(InterpretError::MissingReturn(function.name.clone()));
    };

    match statement {
        Stmt::Return { value, .. } => eval_expr(value),
    }
}

fn eval_expr(expr: &Expr) -> Result<Value, InterpretError> {
    match expr {
        Expr::String { value, .. } => Ok(Value::Json(JsonValue::String(value.clone()))),
        Expr::Object { fields, .. } => {
            let mut object = Map::new();
            for field in fields {
                object.insert(
                    field.key.clone(),
                    expect_json(eval_expr(&field.value)?, "object")?,
                );
            }
            Ok(Value::Json(JsonValue::Object(object)))
        }
        Expr::Call { callee, args, .. } => eval_call(&callee.segments.join("."), args),
        Expr::Path { path } => Err(InterpretError::UnsupportedExpression(
            path.segments.join("."),
        )),
    }
}

fn eval_call(callee: &str, args: &[Expr]) -> Result<Value, InterpretError> {
    match callee {
        "Http.Response.ok" => {
            let body = single_json_arg(callee, args)?;
            Ok(Value::HttpResponse(HttpResponseValue { status: 200, body }))
        }
        "Http.Response.created" => {
            let body = single_json_arg(callee, args)?;
            Ok(Value::HttpResponse(HttpResponseValue { status: 201, body }))
        }
        _ => Err(InterpretError::UnsupportedExpression(callee.to_string())),
    }
}

fn single_json_arg(callee: &str, args: &[Expr]) -> Result<JsonValue, InterpretError> {
    if args.len() != 1 {
        return Err(InterpretError::Arity {
            callee: callee.to_string(),
            expected: 1,
            actual: args.len(),
        });
    }

    expect_json(eval_expr(&args[0])?, callee)
}

fn expect_json(value: Value, context: &str) -> Result<JsonValue, InterpretError> {
    match value {
        Value::Json(value) => Ok(value),
        Value::HttpResponse(_) => Err(InterpretError::ExpectedJson {
            context: context.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{HttpResponseValue, Value, interpret_function};
    use are_ast::Item;
    use are_lexer::lex_source;
    use are_parser::parse_tokens;
    use std::path::Path;

    #[test]
    fn interprets_health_response_from_arelang_body() {
        let source = include_str!("../../../examples/users_api/main.are");
        let file = Path::new("examples/users_api/main.are");
        let (tokens, lex_diagnostics) = lex_source(file, source);
        assert!(lex_diagnostics.is_empty(), "{lex_diagnostics:#?}");
        let (module, parse_diagnostics) = parse_tokens(file, &tokens);
        assert!(parse_diagnostics.is_empty(), "{parse_diagnostics:#?}");
        let module = module.expect("module parses");

        let health = module
            .items
            .iter()
            .find_map(|item| {
                if let Item::Function(function) = item {
                    (function.name == "health").then_some(function)
                } else {
                    None
                }
            })
            .expect("health function exists");

        let value = interpret_function(health).expect("health interprets");
        assert_eq!(
            value,
            Value::HttpResponse(HttpResponseValue {
                status: 200,
                body: serde_json::json!({ "status": "ok" }),
            })
        );
    }
}
