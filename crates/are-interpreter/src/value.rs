use serde_json::Value as JsonValue;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Json(JsonValue),
    Bool(bool),
    HttpResponse(HttpResponseValue),
    Enum(EnumValue),
    Unit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpResponseValue {
    pub status: u16,
    pub body: JsonValue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumValue {
    pub enum_name: String,
    pub variant: String,
    pub payload: Vec<Value>,
}
