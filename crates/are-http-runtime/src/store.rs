use crate::errors::api_invalid_input;
use crate::schemas::{RuntimeSchemas, type_expr_is_optional};
use are_ast::{ModelDecl, ModelField, ModelFieldAttr};
use are_interpreter::InterpretError;
use serde_json::Map;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
pub(crate) struct RuntimeState {
    inner: Arc<Mutex<RuntimeStateInner>>,
}

#[derive(Debug, Default)]
struct RuntimeStateInner {
    stores: HashMap<String, RuntimeModelStore>,
}

#[derive(Debug, Default)]
struct RuntimeModelStore {
    next_id: u64,
    records: HashMap<String, serde_json::Value>,
}

impl RuntimeState {
    pub(crate) fn insert_model(
        &self,
        collection: &str,
        model: &ModelDecl,
        input: &serde_json::Value,
        schemas: &RuntimeSchemas,
    ) -> Result<serde_json::Value, InterpretError> {
        let mut inner = self.inner.lock().expect("runtime state lock poisoned");
        let store = inner.stores.entry(collection.to_string()).or_default();
        store.next_id += 1;

        let record = build_model_record(model, input, store.next_id, schemas)?;
        if !schemas.validate_model_fields(&model.fields, &record) {
            return Err(api_invalid_input("invalid_json"));
        }

        let key = primary_record_key(model, &record)?;
        store.records.insert(key, record.clone());
        Ok(record)
    }

    pub(crate) fn get_model(
        &self,
        collection: &str,
        id: &serde_json::Value,
    ) -> Result<serde_json::Value, InterpretError> {
        let key = json_key(id).ok_or_else(|| api_invalid_input("invalid_id"))?;

        let inner = self.inner.lock().expect("runtime state lock poisoned");
        let Some(record) = inner
            .stores
            .get(collection)
            .and_then(|store| store.records.get(&key))
        else {
            return Err(InterpretError::raised_api_error("NotFound", Vec::new()));
        };

        Ok(record.clone())
    }
}

fn build_model_record(
    model: &ModelDecl,
    input: &serde_json::Value,
    generated_id: u64,
    schemas: &RuntimeSchemas,
) -> Result<serde_json::Value, InterpretError> {
    let Some(input) = input.as_object() else {
        return Err(api_invalid_input("invalid_json"));
    };

    let mut record = Map::new();
    for field in &model.fields {
        if model_field_has_attr(field, ModelFieldAttr::Primary) {
            record.insert(
                field.name.clone(),
                generated_primary_value(field, generated_id, schemas)?,
            );
            continue;
        }

        if let Some(value) = input.get(&field.name) {
            record.insert(field.name.clone(), value.clone());
        } else if !type_expr_is_optional(&field.ty) {
            return Err(api_invalid_input("invalid_json"));
        }
    }

    Ok(serde_json::Value::Object(record))
}

fn generated_primary_value(
    field: &ModelField,
    id: u64,
    schemas: &RuntimeSchemas,
) -> Result<serde_json::Value, InterpretError> {
    match schemas.type_expr_primitive_root(&field.ty).as_deref() {
        Some("String" | "Text") => Ok(serde_json::Value::String(id.to_string())),
        Some("I64" | "Int") => i64::try_from(id)
            .map(serde_json::Value::from)
            .map_err(|_| api_invalid_input("invalid_id")),
        Some("F64") => Err(InterpretError::UnsupportedExpression(
            "F64 primary ids are not supported by the MVP store".into(),
        )),
        Some(_) | None => Ok(serde_json::Value::from(id)),
    }
}

fn primary_record_key(
    model: &ModelDecl,
    record: &serde_json::Value,
) -> Result<String, InterpretError> {
    let Some(primary) = model
        .fields
        .iter()
        .find(|field| model_field_has_attr(field, ModelFieldAttr::Primary))
    else {
        return Err(InterpretError::UnsupportedExpression(format!(
            "model {} has no primary field",
            model.name
        )));
    };

    let Some(value) = record.get(&primary.name) else {
        return Err(api_invalid_input("invalid_id"));
    };

    json_key(value).ok_or_else(|| api_invalid_input("invalid_id"))
}

fn model_field_has_attr(field: &ModelField, attr: ModelFieldAttr) -> bool {
    field.attrs.contains(&attr)
}

fn json_key(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            None
        }
    }
}
