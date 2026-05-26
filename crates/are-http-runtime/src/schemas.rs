use crate::errors::api_invalid_input;
use are_ast::{Field, Item, ModelDecl, ModelField, StructDecl, TypeDecl, TypeExpr};
use are_interpreter::InterpretError;
use are_project::CheckedFile;
use are_semantics::collection_name_for_model;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub(crate) struct RuntimeSchemas {
    pub(crate) structs: HashMap<String, StructDecl>,
    pub(crate) models: HashMap<String, ModelDecl>,
    pub(crate) aliases: HashMap<String, TypeDecl>,
}

impl RuntimeSchemas {
    pub(crate) fn from_modules(modules: &[CheckedFile]) -> Self {
        let mut schemas = Self::default();

        for item in modules.iter().flat_map(|file| file.module.items.iter()) {
            match item {
                Item::Struct(decl) => {
                    schemas.structs.insert(decl.name.clone(), decl.clone());
                }
                Item::Model(decl) => {
                    schemas.models.insert(decl.name.clone(), decl.clone());
                }
                Item::Type(decl) => {
                    schemas.aliases.insert(decl.name.clone(), decl.clone());
                }
                Item::Use(_) | Item::Enum(_) | Item::Function(_) | Item::Service(_) => {}
            }
        }

        schemas
    }

    pub(crate) fn validate_json_body(&self, type_name: &str, body: &str) -> bool {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
            return false;
        };

        self.validate_value(type_name, &value)
    }

    pub(crate) fn model_for_collection(&self, collection: &str) -> Option<&ModelDecl> {
        self.models
            .values()
            .find(|model| collection_name_for_model(&model.name) == collection)
    }

    pub(crate) fn validate_value(&self, type_name: &str, value: &serde_json::Value) -> bool {
        if let Some(alias) = self.aliases.get(type_name) {
            return self.validate_type_expr(&alias.aliased, value);
        }

        if let Some(decl) = self.structs.get(type_name) {
            return self.validate_struct_fields(&decl.fields, value);
        }

        if let Some(decl) = self.models.get(type_name) {
            return self.validate_model_fields(&decl.fields, value);
        }

        validate_primitive(type_name, value)
    }

    pub(crate) fn decode_query_params(
        &self,
        type_name: &str,
        query: &str,
    ) -> Result<serde_json::Value, InterpretError> {
        let values = parse_query_string(query);
        let fields = self
            .payload_fields(type_name)
            .ok_or_else(|| api_invalid_input("invalid_query"))?;
        let mut object = serde_json::Map::new();

        for field in fields {
            let Some(raw) = values.get(field.name.as_str()) else {
                if field.optional {
                    continue;
                }

                return Err(api_invalid_input(&format!("missing_{}", field.name)));
            };

            let value = self.decode_text_value(field.ty, &field.name, raw)?;
            object.insert(field.name, value);
        }

        let value = serde_json::Value::Object(object);
        if self.validate_value(type_name, &value) {
            Ok(value)
        } else {
            Err(api_invalid_input("invalid_query"))
        }
    }

    pub(crate) fn decode_headers(
        &self,
        type_name: &str,
        headers: &HashMap<String, String>,
    ) -> Result<serde_json::Value, InterpretError> {
        let fields = self
            .payload_fields(type_name)
            .ok_or_else(|| api_invalid_input("invalid_headers"))?;
        let mut object = serde_json::Map::new();

        for field in fields {
            let header_name = header_name_for_field(&field.name);
            let raw_field_name = field.name.to_ascii_lowercase();
            let Some(raw) = headers
                .get(header_name.as_str())
                .or_else(|| headers.get(raw_field_name.as_str()))
            else {
                if field.optional {
                    continue;
                }

                return Err(api_invalid_input(&format!("missing_{}", field.name)));
            };

            let value = self.decode_text_value(field.ty, &field.name, raw)?;
            object.insert(field.name, value);
        }

        let value = serde_json::Value::Object(object);
        if self.validate_value(type_name, &value) {
            Ok(value)
        } else {
            Err(api_invalid_input("invalid_headers"))
        }
    }

    pub(crate) fn validate_model_fields(
        &self,
        fields: &[ModelField],
        value: &serde_json::Value,
    ) -> bool {
        let Some(object) = value.as_object() else {
            return false;
        };

        fields.iter().all(|field| match object.get(&field.name) {
            Some(value) => self.validate_type_expr(&field.ty, value),
            None => type_expr_is_optional(&field.ty),
        })
    }

    pub(crate) fn decode_path_param(
        &self,
        type_name: Option<&str>,
        name: &str,
        value: &str,
    ) -> Result<serde_json::Value, InterpretError> {
        let Some(type_name) = type_name else {
            return Ok(serde_json::Value::String(value.to_string()));
        };

        match self.primitive_root(type_name).as_deref() {
            Some("U64") => value
                .parse::<u64>()
                .map(serde_json::Value::from)
                .map_err(|_| api_invalid_input(&format!("invalid_{name}"))),
            Some("I64" | "Int") => value
                .parse::<i64>()
                .map(serde_json::Value::from)
                .map_err(|_| api_invalid_input(&format!("invalid_{name}"))),
            Some("Bool") => value
                .parse::<bool>()
                .map(serde_json::Value::from)
                .map_err(|_| api_invalid_input(&format!("invalid_{name}"))),
            Some("F64") => value
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number)
                .ok_or_else(|| api_invalid_input(&format!("invalid_{name}"))),
            _ => Ok(serde_json::Value::String(value.to_string())),
        }
    }

    pub(crate) fn type_expr_primitive_root(&self, ty: &TypeExpr) -> Option<String> {
        let TypeExpr::Path { path } = ty else {
            return None;
        };

        if path.segments.len() != 1 {
            return None;
        }

        self.primitive_root(&path.segments[0])
    }

    fn validate_type_expr(&self, ty: &TypeExpr, value: &serde_json::Value) -> bool {
        match ty {
            TypeExpr::Path { path } => {
                let Some(type_name) = path.segments.first() else {
                    return false;
                };

                if path.segments.len() != 1 {
                    return true;
                }

                self.validate_value(type_name, value)
            }
            TypeExpr::Generic { base, args, .. } => {
                if path_is(base, &["Option"]) && args.len() == 1 {
                    return value.is_null() || self.validate_type_expr(&args[0], value);
                }

                true
            }
            TypeExpr::Option { inner, .. } => {
                value.is_null() || self.validate_type_expr(inner, value)
            }
        }
    }

    fn validate_struct_fields(&self, fields: &[Field], value: &serde_json::Value) -> bool {
        let Some(object) = value.as_object() else {
            return false;
        };

        fields.iter().all(|field| match object.get(&field.name) {
            Some(value) => self.validate_type_expr(&field.ty, value),
            None => type_expr_is_optional(&field.ty),
        })
    }

    fn primitive_root(&self, type_name: &str) -> Option<String> {
        if is_primitive_type(type_name) {
            return Some(type_name.to_string());
        }

        let alias = self.aliases.get(type_name)?;
        let TypeExpr::Path { path } = &alias.aliased else {
            return None;
        };

        if path.segments.len() != 1 {
            return None;
        }

        self.primitive_root(&path.segments[0])
    }

    fn payload_fields(&self, type_name: &str) -> Option<Vec<PayloadField<'_>>> {
        if let Some(decl) = self.structs.get(type_name) {
            return Some(
                decl.fields
                    .iter()
                    .map(|field| PayloadField {
                        name: field.name.clone(),
                        ty: &field.ty,
                        optional: type_expr_is_optional(&field.ty),
                    })
                    .collect(),
            );
        }

        if let Some(decl) = self.models.get(type_name) {
            return Some(
                decl.fields
                    .iter()
                    .map(|field| PayloadField {
                        name: field.name.clone(),
                        ty: &field.ty,
                        optional: type_expr_is_optional(&field.ty),
                    })
                    .collect(),
            );
        }

        None
    }

    fn decode_text_value(
        &self,
        ty: &TypeExpr,
        name: &str,
        value: &str,
    ) -> Result<serde_json::Value, InterpretError> {
        let primitive = match ty {
            TypeExpr::Option { inner, .. } => self.type_expr_primitive_root(inner),
            TypeExpr::Generic { base, args, .. }
                if path_is(base, &["Option"]) && args.len() == 1 =>
            {
                self.type_expr_primitive_root(&args[0])
            }
            _ => self.type_expr_primitive_root(ty),
        };

        match primitive.as_deref() {
            Some("U64") => value
                .parse::<u64>()
                .map(serde_json::Value::from)
                .map_err(|_| api_invalid_input(&format!("invalid_{name}"))),
            Some("I64" | "Int") => value
                .parse::<i64>()
                .map(serde_json::Value::from)
                .map_err(|_| api_invalid_input(&format!("invalid_{name}"))),
            Some("Bool") => value
                .parse::<bool>()
                .map(serde_json::Value::from)
                .map_err(|_| api_invalid_input(&format!("invalid_{name}"))),
            Some("F64") => value
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number)
                .ok_or_else(|| api_invalid_input(&format!("invalid_{name}"))),
            _ => Ok(serde_json::Value::String(value.to_string())),
        }
    }
}

struct PayloadField<'a> {
    name: String,
    ty: &'a TypeExpr,
    optional: bool,
}

pub(crate) fn type_expr_is_optional(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::Option { .. } => true,
        TypeExpr::Generic { base, .. } => path_is(base, &["Option"]),
        TypeExpr::Path { .. } => false,
    }
}

fn path_is(path: &are_ast::Path, expected: &[&str]) -> bool {
    path.segments.len() == expected.len()
        && path
            .segments
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
}

fn validate_primitive(type_name: &str, value: &serde_json::Value) -> bool {
    match type_name {
        "String" | "Text" => value.is_string(),
        "Bool" => value.is_boolean(),
        "Int" | "I64" => value.as_i64().is_some(),
        "U64" => value.as_u64().is_some(),
        "F64" => value.as_f64().is_some(),
        _ => false,
    }
}

fn is_primitive_type(type_name: &str) -> bool {
    matches!(
        type_name,
        "String" | "Text" | "Bool" | "Int" | "I64" | "U64" | "F64"
    )
}

fn parse_query_string(query: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair
            .split_once('=')
            .map_or((pair, ""), |(key, value)| (key, value));
        values.insert(percent_decode(key), percent_decode(value));
    }

    values
}

pub(crate) fn header_name_for_field(name: &str) -> String {
    name.replace('_', "-").to_ascii_lowercase()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                if let Some(decoded) = decode_hex_pair(bytes[index + 1], bytes[index + 2]) {
                    output.push(decoded);
                    index += 3;
                } else {
                    output.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&output).into_owned()
}

fn decode_hex_pair(high: u8, low: u8) -> Option<u8> {
    Some(hex_value(high)? * 16 + hex_value(low)?)
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
