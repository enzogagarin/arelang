use crate::errors::api_invalid_input;
use crate::schemas::RuntimeSchemas;
use crate::store::RuntimeState;
use are_interpreter::{Host, InterpretError};
use std::collections::HashMap;

pub(crate) struct RuntimeHost<'a> {
    pub(crate) state: &'a RuntimeState,
    pub(crate) params: &'a HashMap<String, String>,
    pub(crate) request_body: &'a str,
    pub(crate) query: &'a str,
    pub(crate) headers: &'a HashMap<String, String>,
    pub(crate) schemas: &'a RuntimeSchemas,
}

impl Host for RuntimeHost<'_> {
    fn read_json_body(
        &mut self,
        type_name: Option<&str>,
    ) -> Result<serde_json::Value, InterpretError> {
        let value = serde_json::from_str::<serde_json::Value>(self.request_body)
            .map_err(|_| api_invalid_input("invalid_json"))?;

        if let Some(type_name) = type_name
            && !self.schemas.validate_value(type_name, &value)
        {
            return Err(api_invalid_input("invalid_json"));
        }

        Ok(value)
    }

    fn read_query_params(
        &mut self,
        type_name: Option<&str>,
    ) -> Result<serde_json::Value, InterpretError> {
        let Some(type_name) = type_name else {
            return Ok(serde_json::Value::Object(serde_json::Map::new()));
        };

        self.schemas.decode_query_params(type_name, self.query)
    }

    fn read_headers(
        &mut self,
        type_name: Option<&str>,
    ) -> Result<serde_json::Value, InterpretError> {
        let Some(type_name) = type_name else {
            return Ok(serde_json::Value::Object(serde_json::Map::new()));
        };

        self.schemas.decode_headers(type_name, self.headers)
    }

    fn read_cookies(
        &mut self,
        type_name: Option<&str>,
    ) -> Result<serde_json::Value, InterpretError> {
        let Some(type_name) = type_name else {
            return Ok(serde_json::Value::Object(serde_json::Map::new()));
        };

        self.schemas.decode_cookies(type_name, self.headers)
    }

    fn validate_email(&mut self, value: &serde_json::Value) -> Result<bool, InterpretError> {
        let Some(email) = value.as_str() else {
            return Ok(false);
        };

        Ok(email.contains('@'))
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
            InterpretError::UnsupportedExpression("validate.length input is too large".into())
        })?;
        Ok((min..=max).contains(&len))
    }

    fn insert_model(
        &mut self,
        collection: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, InterpretError> {
        let model = self
            .schemas
            .model_for_collection(collection)
            .ok_or_else(|| InterpretError::UnsupportedExpression(format!("ctx.db.{collection}")))?;

        self.state
            .insert_model(collection, model, &input, self.schemas)
    }

    fn read_path_param(
        &mut self,
        type_name: Option<&str>,
        name: &str,
    ) -> Result<serde_json::Value, InterpretError> {
        let Some(value) = self.params.get(name) else {
            return Err(api_invalid_input(&format!("missing_{name}")));
        };

        self.schemas.decode_path_param(type_name, name, value)
    }

    fn get_model(
        &mut self,
        collection: &str,
        id: serde_json::Value,
    ) -> Result<serde_json::Value, InterpretError> {
        self.schemas
            .model_for_collection(collection)
            .ok_or_else(|| InterpretError::UnsupportedExpression(format!("ctx.db.{collection}")))?;

        self.state.get_model(collection, &id)
    }
}
