use crate::contracts::{
    HttpAliasSchema, HttpContractManifest, HttpEnumSchema, HttpFieldSchema, HttpModelFieldSchema,
    HttpModelSchema, HttpRouteContract, HttpSchemaManifest, HttpStructSchema,
};
use crate::schemas::header_name_for_field;
use are_project::Manifest;
use serde_json::{Map, Value, json};

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

    operation.insert("responses".to_string(), responses(route));
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
            .map(|field| field_parameter(&field.name, &field.ty, !field.optional, location))
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
            .map(|field| field_parameter(&field.name, &field.ty, !field.optional, location))
            .collect();
    }

    Vec::new()
}

fn field_parameter(name: &str, ty: &str, required: bool, location: &str) -> Value {
    let name = if location == "header" {
        header_name_for_field(name)
    } else {
        name.to_string()
    };

    json!({
        "name": name,
        "in": location,
        "required": required,
        "schema": type_schema(ty),
    })
}

fn responses(route: &HttpRouteContract) -> Value {
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
    Value::Object(responses)
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
                object_component(fields)
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

fn optional_inner(type_name: &str) -> Option<&str> {
    type_name.strip_suffix('?').map(str::trim).or_else(|| {
        type_name
            .strip_prefix("Option<")?
            .strip_suffix('>')
            .map(str::trim)
    })
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
}
