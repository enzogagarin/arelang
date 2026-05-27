use crate::error::InterpretError;
use serde_json::Value as JsonValue;

pub trait Host {
    /// Decode the current HTTP request body as JSON.
    ///
    /// # Errors
    ///
    /// Returns an error when the body cannot be decoded or the host has no
    /// request body available.
    fn read_json_body(&mut self, type_name: Option<&str>) -> Result<JsonValue, InterpretError>;

    /// Decode the current HTTP request query string as a typed JSON object.
    ///
    /// # Errors
    ///
    /// Returns an error when the query string is missing required fields or
    /// cannot be decoded as the requested type.
    fn read_query_params(&mut self, type_name: Option<&str>) -> Result<JsonValue, InterpretError>;

    /// Decode the current HTTP request headers as a typed JSON object.
    ///
    /// # Errors
    ///
    /// Returns an error when required headers are missing or cannot be decoded
    /// as the requested type.
    fn read_headers(&mut self, type_name: Option<&str>) -> Result<JsonValue, InterpretError>;

    /// Decode the current HTTP request cookies as a typed JSON object.
    ///
    /// # Errors
    ///
    /// Returns an error when required cookies are missing or cannot be decoded
    /// as the requested type.
    fn read_cookies(&mut self, type_name: Option<&str>) -> Result<JsonValue, InterpretError>;

    /// Check whether a JSON string is email-like.
    ///
    /// # Errors
    ///
    /// Returns an error when the host validator itself cannot run.
    fn validate_email(&mut self, value: &JsonValue) -> Result<bool, InterpretError>;

    /// Check whether a JSON string length is within bounds.
    ///
    /// # Errors
    ///
    /// Returns an error when the host validator itself cannot run.
    fn validate_length(
        &mut self,
        value: &JsonValue,
        min: i64,
        max: i64,
    ) -> Result<bool, InterpretError>;

    /// Insert a model-like JSON value into host state.
    ///
    /// # Errors
    ///
    /// Returns an error when the host cannot persist the value.
    fn insert_model(
        &mut self,
        collection: &str,
        input: JsonValue,
    ) -> Result<JsonValue, InterpretError>;

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

    /// Read a model-like JSON value from host state.
    ///
    /// # Errors
    ///
    /// Returns an error when the id is invalid or the value does not exist.
    fn get_model(&mut self, collection: &str, id: JsonValue) -> Result<JsonValue, InterpretError>;
}

pub(crate) struct NoopHost;

impl Host for NoopHost {
    fn read_json_body(&mut self, _type_name: Option<&str>) -> Result<JsonValue, InterpretError> {
        Err(InterpretError::UnsupportedExpression("req.json".into()))
    }

    fn read_query_params(&mut self, _type_name: Option<&str>) -> Result<JsonValue, InterpretError> {
        Err(InterpretError::UnsupportedExpression("req.query".into()))
    }

    fn read_headers(&mut self, _type_name: Option<&str>) -> Result<JsonValue, InterpretError> {
        Err(InterpretError::UnsupportedExpression("req.headers".into()))
    }

    fn read_cookies(&mut self, _type_name: Option<&str>) -> Result<JsonValue, InterpretError> {
        Err(InterpretError::UnsupportedExpression("req.cookies".into()))
    }

    fn validate_email(&mut self, _value: &JsonValue) -> Result<bool, InterpretError> {
        Err(InterpretError::UnsupportedExpression(
            "validate.email".into(),
        ))
    }

    fn validate_length(
        &mut self,
        _value: &JsonValue,
        _min: i64,
        _max: i64,
    ) -> Result<bool, InterpretError> {
        Err(InterpretError::UnsupportedExpression(
            "validate.length".into(),
        ))
    }

    fn insert_model(
        &mut self,
        collection: &str,
        _input: JsonValue,
    ) -> Result<JsonValue, InterpretError> {
        Err(InterpretError::UnsupportedExpression(format!(
            "ctx.db.{collection}.insert"
        )))
    }

    fn read_path_param(
        &mut self,
        _type_name: Option<&str>,
        _name: &str,
    ) -> Result<JsonValue, InterpretError> {
        Err(InterpretError::UnsupportedExpression("ctx.param".into()))
    }

    fn get_model(&mut self, collection: &str, _id: JsonValue) -> Result<JsonValue, InterpretError> {
        Err(InterpretError::UnsupportedExpression(format!(
            "ctx.db.{collection}.get"
        )))
    }
}
