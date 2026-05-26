use crate::value::{EnumValue, HttpResponseValue, Value};
use serde_json::Value as JsonValue;

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
    ExpectedBool {
        context: String,
    },
    ExpectedEnum {
        context: String,
    },
    UnmatchedPattern {
        variant: String,
    },
    RaisedError(EnumValue),
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
            Self::ExpectedBool { context } => write!(f, "`{context}` expected a boolean value"),
            Self::ExpectedEnum { context } => write!(f, "`{context}` expected an enum value"),
            Self::UnmatchedPattern { variant } => {
                write!(f, "no match arm handled variant `{variant}`")
            }
            Self::RaisedError(error) => {
                write!(f, "raised {}.{}", error.enum_name, error.variant)
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
    pub fn raised_api_error(variant: &str, payload: Vec<JsonValue>) -> Self {
        Self::RaisedError(EnumValue {
            enum_name: "ApiError".to_string(),
            variant: variant.to_string(),
            payload: payload.into_iter().map(Value::Json).collect(),
        })
    }

    #[must_use]
    pub const fn as_http_response(&self) -> Option<&HttpResponseValue> {
        match self {
            Self::RaisedHttpResponse(response) => Some(response),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_raised_error(&self) -> Option<&EnumValue> {
        match self {
            Self::RaisedError(error) => Some(error),
            _ => None,
        }
    }
}
