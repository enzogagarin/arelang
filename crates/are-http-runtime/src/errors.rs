use are_interpreter::InterpretError;

pub(crate) fn api_invalid_input(message: &str) -> InterpretError {
    InterpretError::raised_api_error(
        "InvalidInput",
        vec![serde_json::Value::String(message.to_string())],
    )
}
