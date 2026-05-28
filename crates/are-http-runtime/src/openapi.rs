use crate::contracts::{
    HttpAliasSchema, HttpContractManifest, HttpEnumSchema, HttpEnumVariantSchema, HttpFieldSchema,
    HttpFieldValidationSchema, HttpModelFieldSchema, HttpModelSchema, HttpRouteContract,
    HttpSchemaManifest, HttpStructSchema,
};
use crate::schemas::{cookie_name_for_field, header_name_for_field};
use are_project::Manifest;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub(crate) fn openapi_document(manifest: &Manifest, contracts: &HttpContractManifest) -> Value {
    let mut document = Map::new();
    document.insert("openapi".to_string(), Value::String("3.1.0".to_string()));
    document.insert(
        "info".to_string(),
        json!({
            "title": manifest.package.name,
            "version": manifest.package.version,
            "x-are-service": contracts.service,
        }),
    );

    if let Some(server) = &manifest.server {
        document.insert(
            "servers".to_string(),
            json!([
                {
                    "url": format!("http://{}:{}", server.host, server.port),
                }
            ]),
        );
    }

    document.insert("paths".to_string(), paths(contracts));
    document.insert(
        "components".to_string(),
        json!({
            "schemas": component_schemas(&contracts.schemas),
        }),
    );

    Value::Object(document)
}

fn paths(contracts: &HttpContractManifest) -> Value {
    let mut paths = Map::new();
    for route in &contracts.routes {
        let path = openapi_path(&route.path);
        let path_item = paths
            .entry(path)
            .or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(path_item) = path_item else {
            continue;
        };

        path_item.insert(
            route.method.to_ascii_lowercase(),
            operation(route, contracts),
        );
    }

    Value::Object(paths)
}

fn operation(route: &HttpRouteContract, contracts: &HttpContractManifest) -> Value {
    let mut operation = Map::new();
    operation.insert(
        "operationId".to_string(),
        Value::String(operation_id(route)),
    );
    operation.insert("tags".to_string(), json!([contracts.service]));

    let parameters = parameters(route, &contracts.schemas);
    if !parameters.is_empty() {
        operation.insert("parameters".to_string(), Value::Array(parameters));
    }

    if let Some(body_type) = &route.body_type {
        operation.insert(
            "requestBody".to_string(),
            json!({
                "required": true,
                "content": {
                    "application/json": {
                        "schema": type_schema(body_type),
                    },
                },
            }),
        );
    }

    operation.insert("responses".to_string(), responses(route, contracts));
    Value::Object(operation)
}

fn parameters(route: &HttpRouteContract, schemas: &HttpSchemaManifest) -> Vec<Value> {
    let mut parameters = route
        .path_params
        .iter()
        .map(|param| {
            json!({
                "name": param.name,
                "in": "path",
                "required": true,
                "schema": param.ty.as_deref().map_or_else(string_schema, type_schema),
            })
        })
        .collect::<Vec<_>>();

    if let Some(query_type) = &route.query_type {
        parameters.extend(field_parameters(query_type, schemas, "query"));
    }
    if let Some(headers_type) = &route.headers_type {
        parameters.extend(field_parameters(headers_type, schemas, "header"));
    }
    if let Some(cookies_type) = &route.cookies_type {
        parameters.extend(field_parameters(cookies_type, schemas, "cookie"));
    }

    parameters
}

fn field_parameters(type_name: &str, schemas: &HttpSchemaManifest, location: &str) -> Vec<Value> {
    if let Some(schema) = schemas
        .structs
        .iter()
        .find(|schema| schema.name == type_name)
    {
        return schema
            .fields
            .iter()
            .map(|field| {
                field_parameter(
                    &field.name,
                    &field.ty,
                    !field.optional,
                    location,
                    &field.validations,
                )
            })
            .collect();
    }

    if let Some(schema) = schemas
        .models
        .iter()
        .find(|schema| schema.name == type_name)
    {
        return schema
            .fields
            .iter()
            .map(|field| field_parameter(&field.name, &field.ty, !field.optional, location, &[]))
            .collect();
    }

    Vec::new()
}

fn field_parameter(
    name: &str,
    ty: &str,
    required: bool,
    location: &str,
    validations: &[HttpFieldValidationSchema],
) -> Value {
    let name = if location == "header" {
        header_name_for_field(name)
    } else if location == "cookie" {
        cookie_name_for_field(name)
    } else {
        name.to_string()
    };

    let mut schema = type_schema(ty);
    apply_field_validations(&mut schema, validations);

    json!({
        "name": name,
        "in": location,
        "required": required,
        "schema": schema,
    })
}

fn responses(route: &HttpRouteContract, contracts: &HttpContractManifest) -> Value {
    let status = route.status.unwrap_or(200).to_string();
    let schema = route
        .response_type
        .as_deref()
        .map_or_else(|| json!({}), type_schema);

    let mut responses = Map::new();
    responses.insert(
        status,
        json!({
            "description": "Success",
            "content": {
                "application/json": {
                    "schema": schema,
                },
            },
        }),
    );

    add_error_responses(route, contracts, &mut responses);
    Value::Object(responses)
}

fn add_error_responses(
    route: &HttpRouteContract,
    contracts: &HttpContractManifest,
    responses: &mut Map<String, Value>,
) {
    let Some(error_contract) = &contracts.error_contract else {
        return;
    };
    if route.error_type.as_deref() != Some(error_contract.as_str()) {
        return;
    }

    let Some(error_schema) = contracts
        .schemas
        .enums
        .iter()
        .find(|schema| schema.name == *error_contract)
    else {
        return;
    };

    let mut by_status = BTreeMap::<u16, Vec<&HttpEnumVariantSchema>>::new();
    for variant in &error_schema.variants {
        if let Some(status) = variant.status {
            by_status.entry(status).or_default().push(variant);
        }
    }

    for (status, variants) in by_status {
        responses.entry(status.to_string()).or_insert_with(|| {
            let schema = if variants.len() == 1 {
                error_variant_body_schema(variants[0])
            } else {
                json!({
                    "oneOf": variants
                        .iter()
                        .map(|variant| error_variant_body_schema(variant))
                        .collect::<Vec<_>>(),
                })
            };

            json!({
                "description": format!("{error_contract} error"),
                "content": {
                    "application/json": {
                        "schema": schema,
                    },
                },
            })
        });
    }
}

fn error_variant_body_schema(variant: &HttpEnumVariantSchema) -> Value {
    let mut properties = Map::new();
    let mut required = vec![Value::String("error".to_string())];

    if variant.payload.is_empty() {
        properties.insert(
            "error".to_string(),
            json!({
                "type": "string",
                "const": error_code(&variant.name),
            }),
        );
    } else if variant.payload.len() == 1
        && matches!(variant.payload[0].name.as_str(), "message" | "error")
    {
        properties.insert("error".to_string(), type_schema(&variant.payload[0].ty));
    } else {
        properties.insert("error".to_string(), string_schema());
        for field in &variant.payload {
            required.push(Value::String(field.name.clone()));
            properties.insert(field.name.clone(), type_schema(&field.ty));
        }
    }

    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
        "required": required,
    })
}

fn component_schemas(schemas: &HttpSchemaManifest) -> Value {
    let mut components = Map::new();

    for alias in &schemas.aliases {
        components.insert(alias.name.clone(), alias_component(alias));
    }
    for schema in &schemas.structs {
        components.insert(schema.name.clone(), struct_component(schema));
    }
    for schema in &schemas.models {
        components.insert(schema.name.clone(), model_component(schema));
    }
    for schema in &schemas.enums {
        components.insert(schema.name.clone(), enum_component(schema));
    }

    Value::Object(components)
}

fn alias_component(alias: &HttpAliasSchema) -> Value {
    let mut schema = type_schema(&alias.aliased_type);
    apply_field_validations(&mut schema, &alias.validations);
    insert_extension(&mut schema, "x-are-opaque", Value::Bool(alias.opaque));
    schema
}

fn struct_component(schema: &HttpStructSchema) -> Value {
    object_component(schema.fields.iter().map(FieldLike::Struct))
}

fn model_component(schema: &HttpModelSchema) -> Value {
    let mut component = object_component(schema.fields.iter().map(FieldLike::Model));
    insert_extension(
        &mut component,
        "x-are-collection",
        Value::String(schema.collection.clone()),
    );
    component
}

fn enum_component(schema: &HttpEnumSchema) -> Value {
    json!({
        "oneOf": schema
            .variants
            .iter()
            .map(|variant| {
                let mut fields = vec![enum_tag_field(&variant.name)];
                fields.extend(variant.payload.iter().map(FieldLike::Struct));
                let mut component = object_component(fields);
                if let Some(status) = variant.status {
                    insert_extension(&mut component, "x-are-status", Value::from(status));
                }
                component
            })
            .collect::<Vec<_>>(),
    })
}

fn object_component<'a>(fields: impl IntoIterator<Item = FieldLike<'a>>) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();

    for field in fields {
        let name = field.name().to_string();
        if !field.optional() {
            required.push(Value::String(name.clone()));
        }

        let mut schema = type_schema(field.ty());
        if field.primary() == Some(true) {
            insert_extension(&mut schema, "x-are-primary", Value::Bool(true));
        }
        if field.unique() == Some(true) {
            insert_extension(&mut schema, "x-are-unique", Value::Bool(true));
        }
        if let Some(value) = field.const_value() {
            insert_extension(&mut schema, "const", Value::String(value.to_string()));
        }
        apply_field_validations(&mut schema, field.validations());
        properties.insert(name, schema);
    }

    let mut schema = Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    schema.insert("additionalProperties".to_string(), Value::Bool(false));
    schema.insert("properties".to_string(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".to_string(), Value::Array(required));
    }

    Value::Object(schema)
}

fn enum_tag_field(name: &str) -> FieldLike<'static> {
    FieldLike::Synthetic {
        name: "variant",
        ty: "String",
        value: name.to_string(),
    }
}

fn type_schema(type_name: &str) -> Value {
    let type_name = type_name.trim();
    if let Some(inner) = optional_inner(type_name) {
        return json!({
            "anyOf": [
                type_schema(inner),
                { "type": "null" },
            ],
        });
    }

    if let Some(inner) = list_inner(type_name) {
        return json!({
            "type": "array",
            "items": type_schema(inner),
        });
    }

    match type_name {
        "String" | "Text" => string_schema(),
        "Bool" => json!({ "type": "boolean" }),
        "Int" | "I64" => json!({ "type": "integer", "format": "int64" }),
        "U64" => json!({ "type": "integer", "format": "uint64", "minimum": 0 }),
        "F64" => json!({ "type": "number", "format": "double" }),
        name if is_ref_name(name) => json!({ "$ref": format!("#/components/schemas/{name}") }),
        name => json!({ "x-are-type": name }),
    }
}

fn string_schema() -> Value {
    json!({ "type": "string" })
}

fn apply_field_validations(schema: &mut Value, validations: &[HttpFieldValidationSchema]) {
    for validation in validations {
        match validation {
            HttpFieldValidationSchema::Email => {
                insert_extension(schema, "format", Value::String("email".to_string()));
            }
            HttpFieldValidationSchema::Length { min, max } => {
                insert_extension(schema, "minLength", Value::from(*min));
                insert_extension(schema, "maxLength", Value::from(*max));
            }
        }
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

fn is_ref_name(type_name: &str) -> bool {
    type_name.chars().next().is_some_and(char::is_uppercase)
        && type_name
            .chars()
            .all(|character| character == '_' || character == '.' || character.is_alphanumeric())
}

fn operation_id(route: &HttpRouteContract) -> String {
    route.handler.replace('.', "_")
}

fn openapi_path(path: &str) -> String {
    let converted = path
        .split('/')
        .map(openapi_path_segment)
        .collect::<Vec<_>>()
        .join("/");

    if converted.starts_with('/') {
        converted
    } else {
        format!("/{converted}")
    }
}

fn openapi_path_segment(segment: &str) -> String {
    if let Some(name) = segment.strip_prefix(':') {
        return format!("{{{name}}}");
    }

    let Some(inner) = segment
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
    else {
        return segment.to_string();
    };
    let name = inner.split_once(':').map_or(inner, |(name, _)| name).trim();
    format!("{{{name}}}")
}

fn error_code(variant_name: &str) -> String {
    let mut output = String::new();
    for (index, ch) in variant_name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                output.push('_');
            }
            output.push(ch.to_ascii_lowercase());
        } else {
            output.push(ch);
        }
    }
    output
}

fn insert_extension(schema: &mut Value, key: &str, value: Value) {
    if let Value::Object(object) = schema {
        object.insert(key.to_string(), value);
    }
}

enum FieldLike<'a> {
    Struct(&'a HttpFieldSchema),
    Model(&'a HttpModelFieldSchema),
    Synthetic {
        name: &'static str,
        ty: &'static str,
        value: String,
    },
}

impl FieldLike<'_> {
    fn name(&self) -> &str {
        match self {
            Self::Struct(field) => &field.name,
            Self::Model(field) => &field.name,
            Self::Synthetic { name, .. } => name,
        }
    }

    fn ty(&self) -> &str {
        match self {
            Self::Struct(field) => &field.ty,
            Self::Model(field) => &field.ty,
            Self::Synthetic { ty, .. } => ty,
        }
    }

    fn optional(&self) -> bool {
        match self {
            Self::Struct(field) => field.optional,
            Self::Model(field) => field.optional,
            Self::Synthetic { .. } => false,
        }
    }

    fn primary(&self) -> Option<bool> {
        match self {
            Self::Model(field) => Some(field.primary),
            Self::Struct(_) | Self::Synthetic { .. } => None,
        }
    }

    fn unique(&self) -> Option<bool> {
        match self {
            Self::Model(field) => Some(field.unique),
            Self::Struct(_) | Self::Synthetic { .. } => None,
        }
    }

    fn const_value(&self) -> Option<&str> {
        match self {
            Self::Synthetic { value, .. } => Some(value),
            Self::Struct(_) | Self::Model(_) => None,
        }
    }

    fn validations(&self) -> &[HttpFieldValidationSchema] {
        match self {
            Self::Struct(field) => &field.validations,
            Self::Model(_) | Self::Synthetic { .. } => &[],
        }
    }
}
