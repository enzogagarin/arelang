use crate::errors::api_invalid_input;
use are_ast::{
    Field, FieldValidation, Item, ModelDecl, ModelField, StructDecl, TypeDecl, TypeExpr,
};
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

    pub(crate) fn validate_json_body(&self, type_name: &str, body: &str) -> Result<(), String> {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
            return Err("invalid_json".to_string());
        };

        self.validate_json_value(type_name, &value)
    }

    pub(crate) fn validate_json_value(
        &self,
        type_name: &str,
        value: &serde_json::Value,
    ) -> Result<(), String> {
        self.validate_value_as(type_name, value, "invalid_json")
    }

    pub(crate) fn model_for_collection(&self, collection: &str) -> Option<&ModelDecl> {
        self.models
            .values()
            .find(|model| collection_name_for_model(&model.name) == collection)
    }

    pub(crate) fn validate_value(&self, type_name: &str, value: &serde_json::Value) -> bool {
        self.validate_value_as(type_name, value, "invalid_value")
            .is_ok()
    }

    fn validate_value_as(
        &self,
        type_name: &str,
        value: &serde_json::Value,
        invalid_code: &str,
    ) -> Result<(), String> {
        if let Some(inner) = optional_inner(type_name) {
            if value.is_null() {
                return Ok(());
            }

            return self.validate_value_as(inner, value, invalid_code);
        }

        if let Some(inner) = list_inner(type_name) {
            let Some(items) = value.as_array() else {
                return Err(invalid_code.to_string());
            };

            for item in items {
                self.validate_value_as(inner, item, invalid_code)?;
            }
            return Ok(());
        }

        if let Some(alias) = self.aliases.get(type_name) {
            self.validate_type_expr_as(&alias.aliased, value, invalid_code)?;
            Self::validate_rules(&alias.validations, value, invalid_code)?;
            return Ok(());
        }

        if let Some(decl) = self.structs.get(type_name) {
            return self.validate_struct_fields(&decl.fields, value, invalid_code);
        }

        if let Some(decl) = self.models.get(type_name) {
            return self.validate_model_fields_as(&decl.fields, value, invalid_code);
        }

        if validate_primitive(type_name, value) {
            Ok(())
        } else {
            Err(invalid_code.to_string())
        }
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
        self.validate_value_as(type_name, &value, "invalid_query")
            .map_err(|code| api_invalid_input(&code))?;
        Ok(value)
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
        self.validate_value_as(type_name, &value, "invalid_headers")
            .map_err(|code| api_invalid_input(&code))?;
        Ok(value)
    }

    pub(crate) fn decode_cookies(
        &self,
        type_name: &str,
        headers: &HashMap<String, String>,
    ) -> Result<serde_json::Value, InterpretError> {
        let values = parse_cookie_header(headers);
        let fields = self
            .payload_fields(type_name)
            .ok_or_else(|| api_invalid_input("invalid_cookies"))?;
        let mut object = serde_json::Map::new();

        for field in fields {
            let cookie_name = cookie_name_for_field(&field.name);
            let Some(raw) = values.get(cookie_name.as_str()) else {
                if field.optional {
                    continue;
                }

                return Err(api_invalid_input(&format!("missing_{}", field.name)));
            };

            let value = self.decode_text_value(field.ty, &field.name, raw)?;
            object.insert(field.name, value);
        }

        let value = serde_json::Value::Object(object);
        self.validate_value_as(type_name, &value, "invalid_cookies")
            .map_err(|code| api_invalid_input(&code))?;
        Ok(value)
    }

    pub(crate) fn validate_model_fields(
        &self,
        fields: &[ModelField],
        value: &serde_json::Value,
    ) -> bool {
        self.validate_model_fields_as(fields, value, "invalid_value")
            .is_ok()
    }

    fn validate_model_fields_as(
        &self,
        fields: &[ModelField],
        value: &serde_json::Value,
        invalid_code: &str,
    ) -> Result<(), String> {
        let Some(object) = value.as_object() else {
            return Err(invalid_code.to_string());
        };

        for field in fields {
            match object.get(&field.name) {
                Some(value) => self.validate_type_expr_as(&field.ty, value, invalid_code)?,
                None if type_expr_is_optional(&field.ty) => {}
                None => return Err(format!("missing_{}", field.name)),
            }
        }

        Ok(())
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

        let decoded = match self.primitive_root(type_name).as_deref() {
            Some("U64") => value
                .parse::<u64>()
                .map(serde_json::Value::from)
                .map_err(|_| api_invalid_input(&format!("invalid_{name}")))?,
            Some("I64" | "Int") => value
                .parse::<i64>()
                .map(serde_json::Value::from)
                .map_err(|_| api_invalid_input(&format!("invalid_{name}")))?,
            Some("Bool") => value
                .parse::<bool>()
                .map(serde_json::Value::from)
                .map_err(|_| api_invalid_input(&format!("invalid_{name}")))?,
            Some("F64") => value
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number)
                .ok_or_else(|| api_invalid_input(&format!("invalid_{name}")))?,
            _ => serde_json::Value::String(value.to_string()),
        };

        self.validate_value_as(type_name, &decoded, &format!("invalid_{name}"))
            .map_err(|code| api_invalid_input(&code))?;
        Ok(decoded)
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

    fn validate_type_expr_as(
        &self,
        ty: &TypeExpr,
        value: &serde_json::Value,
        invalid_code: &str,
    ) -> Result<(), String> {
        match ty {
            TypeExpr::Path { path } => {
                let Some(type_name) = path.segments.first() else {
                    return Err(invalid_code.to_string());
                };

                if path.segments.len() != 1 {
                    return Ok(());
                }

                self.validate_value_as(type_name, value, invalid_code)
            }
            TypeExpr::Generic { base, args, .. } => {
                if path_is(base, &["Option"]) && args.len() == 1 {
                    if value.is_null() {
                        return Ok(());
                    }

                    return self.validate_type_expr_as(&args[0], value, invalid_code);
                }

                if path_is(base, &["List"]) && args.len() == 1 {
                    let Some(items) = value.as_array() else {
                        return Err(invalid_code.to_string());
                    };

                    for item in items {
                        self.validate_type_expr_as(&args[0], item, invalid_code)?;
                    }
                    return Ok(());
                }

                Ok(())
            }
            TypeExpr::Option { inner, .. } => {
                if value.is_null() {
                    return Ok(());
                }

                self.validate_type_expr_as(inner, value, invalid_code)
            }
        }
    }

    fn validate_struct_fields(
        &self,
        fields: &[Field],
        value: &serde_json::Value,
        invalid_code: &str,
    ) -> Result<(), String> {
        let Some(object) = value.as_object() else {
            return Err(invalid_code.to_string());
        };

        for field in fields {
            match object.get(&field.name) {
                Some(value) => {
                    let invalid_field = invalid_field_code(&field.name);
                    self.validate_type_expr_as(&field.ty, value, &invalid_field)?;
                    Self::validate_field_rules(field, value)?;
                }
                None if type_expr_is_optional(&field.ty) => {}
                None => return Err(format!("missing_{}", field.name)),
            }
        }

        Ok(())
    }

    fn validate_field_rules(field: &Field, value: &serde_json::Value) -> Result<(), String> {
        if value.is_null() && type_expr_is_optional(&field.ty) {
            return Ok(());
        }

        Self::validate_rules(&field.validations, value, &invalid_field_code(&field.name))
    }

    fn validate_rules(
        validations: &[FieldValidation],
        value: &serde_json::Value,
        invalid_code: &str,
    ) -> Result<(), String> {
        for validation in validations {
            match validation {
                FieldValidation::Email { .. } => {
                    if !value.as_str().is_some_and(|email| email.contains('@')) {
                        return Err(invalid_code.to_string());
                    }
                }
                FieldValidation::Length { min, max, .. } => {
                    let Some(text) = value.as_str() else {
                        return Err(invalid_code.to_string());
                    };
                    let Ok(len) = i64::try_from(text.chars().count()) else {
                        return Err(invalid_code.to_string());
                    };
                    if !(*min..=*max).contains(&len) {
                        return Err(invalid_code.to_string());
                    }
                }
            }
        }

        Ok(())
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

fn optional_inner(type_name: &str) -> Option<&str> {
    type_name.strip_suffix('?').map(str::trim).or_else(|| {
        type_name
            .strip_prefix("Option<")?
            .strip_suffix('>')
            .map(str::trim)
    })
}

fn list_inner(type_name: &str) -> Option<&str> {
    type_name
        .strip_prefix("List<")?
        .strip_suffix('>')
        .map(str::trim)
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

fn invalid_field_code(name: &str) -> String {
    format!("invalid_{name}")
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

fn parse_cookie_header(headers: &HashMap<String, String>) -> HashMap<String, String> {
    let Some(cookie_header) = headers.get("cookie") else {
        return HashMap::new();
    };

    let mut values = HashMap::new();
    for pair in cookie_header
        .split(';')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
    {
        let (key, value) = pair
            .split_once('=')
            .map_or((pair, ""), |(key, value)| (key.trim(), value.trim()));
        values.insert(percent_decode_cookie(key), percent_decode_cookie(value));
    }

    values
}

pub(crate) fn header_name_for_field(name: &str) -> String {
    name.replace('_', "-").to_ascii_lowercase()
}

pub(crate) fn cookie_name_for_field(name: &str) -> String {
    name.to_string()
}

fn percent_decode(value: &str) -> String {
    percent_decode_with_plus(value, true)
}

fn percent_decode_cookie(value: &str) -> String {
    percent_decode_with_plus(value, false)
}

fn percent_decode_with_plus(value: &str, plus_as_space: bool) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' if plus_as_space => {
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
