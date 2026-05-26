use are_ast::{
    EnumDecl, Field, FunctionDecl, Item, ModelDecl, ModelField, ModelFieldAttr, Module, Param,
    Path, ServiceDecl, StructDecl, TypeDecl, TypeExpr, UseDecl,
};
use are_diagnostics::{Diagnostic, SourceRange, best_name_suggestion};
use std::collections::{HashMap, HashSet};
use std::path::{Path as FsPath, PathBuf};

mod body;
mod http;

#[cfg(test)]
mod tests;

#[must_use]
pub fn typecheck_module(file: &FsPath, module: &Module) -> Vec<Diagnostic> {
    TypeChecker::new(file, module).check()
}

struct TypeChecker<'a> {
    file: PathBuf,
    module: &'a Module,
    http_aliases: HashSet<String>,
    functions: HashMap<String, &'a FunctionDecl>,
    structs: HashMap<String, &'a StructDecl>,
    models: HashMap<String, &'a ModelDecl>,
    enums: HashMap<String, &'a EnumDecl>,
    types: HashMap<String, &'a TypeDecl>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> TypeChecker<'a> {
    fn new(file: &FsPath, module: &'a Module) -> Self {
        let http_aliases = collect_http_aliases(module);
        let functions = collect_functions(module);
        let structs = collect_structs(module);
        let models = collect_models(module);
        let enums = collect_enums(module);
        let types = collect_types(module);

        Self {
            file: file.to_path_buf(),
            module,
            http_aliases,
            functions,
            structs,
            models,
            enums,
            types,
            diagnostics: Vec::new(),
        }
    }

    fn check(mut self) -> Vec<Diagnostic> {
        self.check_items();
        self.diagnostics
    }

    fn check_items(&mut self) {
        for item in &self.module.items {
            match item {
                Item::Use(_) | Item::Type(_) => {}
                Item::Struct(decl) => self.check_struct(decl),
                Item::Model(decl) => self.check_model(decl),
                Item::Enum(decl) => self.check_enum(decl),
                Item::Function(decl) => self.check_function(decl),
                Item::Service(decl) => self.check_service(decl),
            }
        }

        self.check_generic_arities();
    }

    fn check_struct(&mut self, decl: &StructDecl) {
        self.check_duplicate_fields(&decl.fields, DuplicateScope::StructField);
    }

    fn check_model(&mut self, decl: &ModelDecl) {
        self.check_duplicate_model_fields(&decl.fields);
        self.check_model_attrs(decl);
    }

    fn check_enum(&mut self, decl: &EnumDecl) {
        let mut variants = HashMap::new();

        for variant in &decl.variants {
            if let Some(previous) = variants.insert(variant.name.as_str(), variant.range) {
                self.duplicate(
                    "E_TYPE_0005",
                    variant.range,
                    format!("duplicate enum variant `{}`", variant.name),
                    previous,
                );
            }

            self.check_duplicate_fields(&variant.payload, DuplicateScope::EnumPayload);
        }
    }

    fn check_function(&mut self, decl: &FunctionDecl) {
        self.check_duplicate_params(&decl.params);
        self.check_function_body(decl);
    }

    fn check_service(&mut self, decl: &ServiceDecl) {
        if self.http_aliases.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0001",
                &self.file,
                decl.range,
                "HTTP service requires std.http import",
                "add `use std.http as Http` so service handler types can be checked",
            ));
            return;
        }

        let Some(state_param) = &decl.state_param else {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0002",
                &self.file,
                decl.range,
                "service state is required for the HTTP MVP",
                "declare the service as `service Name(state: AppState)`",
            ));
            return;
        };

        if !is_local_struct_type(&state_param.ty, &self.structs) {
            self.diagnostics.push(Diagnostic::error(
                "E_HTTP_0003",
                &self.file,
                state_param.ty.range(),
                "service state must be a local struct",
                "the HTTP MVP uses a concrete AppState-style struct as service state",
            ));
        }

        self.check_routes(decl, &state_param.ty);
    }

    fn check_duplicate_fields(&mut self, fields: &[Field], scope: DuplicateScope) {
        let mut names = HashMap::new();

        for field in fields {
            if let Some(previous) = names.insert(field.name.as_str(), field.range) {
                let (code, label) = match scope {
                    DuplicateScope::StructField => ("E_TYPE_0004", "field"),
                    DuplicateScope::EnumPayload => ("E_TYPE_0006", "payload field"),
                };
                self.duplicate(
                    code,
                    field.range,
                    format!("duplicate {label} `{}`", field.name),
                    previous,
                );
            }
        }
    }

    fn check_duplicate_model_fields(&mut self, fields: &[ModelField]) {
        let mut names = HashMap::new();

        for field in fields {
            if let Some(previous) = names.insert(field.name.as_str(), field.range) {
                self.duplicate(
                    "E_TYPE_0007",
                    field.range,
                    format!("duplicate model field `{}`", field.name),
                    previous,
                );
            }
        }
    }

    fn check_model_attrs(&mut self, decl: &ModelDecl) {
        let mut primary = None;

        for field in &decl.fields {
            let mut attrs = HashMap::new();
            for attr in &field.attrs {
                if let Some(previous) = attrs.insert(*attr, field.range) {
                    self.duplicate(
                        "E_TYPE_0008",
                        field.range,
                        format!(
                            "duplicate model field attribute `{}`",
                            model_attr_name(*attr)
                        ),
                        previous,
                    );
                }

                if *attr == ModelFieldAttr::Primary
                    && let Some(previous) = primary.replace(field.range)
                {
                    self.duplicate(
                        "E_TYPE_0009",
                        field.range,
                        format!("model `{}` has multiple primary fields", decl.name),
                        previous,
                    );
                }
            }
        }
    }

    fn check_duplicate_params(&mut self, params: &[Param]) {
        let mut names = HashMap::new();

        for param in params {
            if let Some(previous) = names.insert(param.name.as_str(), param.range) {
                self.duplicate(
                    "E_TYPE_0003",
                    param.range,
                    format!("duplicate parameter `{}`", param.name),
                    previous,
                );
            }
        }
    }

    fn duplicate(
        &mut self,
        code: &'static str,
        range: SourceRange,
        problem: String,
        previous: SourceRange,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code,
                &self.file,
                range,
                problem,
                format!(
                    "the previous declaration is at {}:{}",
                    previous.start.line, previous.start.column
                ),
            )
            .with_fix("rename this item or remove the duplicate", None),
        );
    }

    fn function_suggestion(&self, name: &str) -> Option<String> {
        let mut candidates = self
            .functions
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        candidates.sort_unstable();

        best_name_suggestion(name, candidates).map(str::to_string)
    }
}

#[derive(Debug, Clone, Copy)]
enum DuplicateScope {
    StructField,
    EnumPayload,
}

fn model_attr_name(attr: ModelFieldAttr) -> &'static str {
    match attr {
        ModelFieldAttr::Primary => "primary",
        ModelFieldAttr::Unique => "unique",
    }
}

fn collect_http_aliases(module: &Module) -> HashSet<String> {
    module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Use(UseDecl { path, alias, .. }) = item else {
                return None;
            };

            if !path_is(path, &["std", "http"]) {
                return None;
            }

            alias.clone().or_else(|| path.segments.last().cloned())
        })
        .collect()
}

fn collect_functions(module: &Module) -> HashMap<String, &FunctionDecl> {
    module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Function(decl) = item else {
                return None;
            };
            Some((decl.name.clone(), decl))
        })
        .collect()
}

fn collect_structs(module: &Module) -> HashMap<String, &StructDecl> {
    module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Struct(decl) = item else {
                return None;
            };
            Some((decl.name.clone(), decl))
        })
        .collect()
}

fn collect_models(module: &Module) -> HashMap<String, &ModelDecl> {
    module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Model(decl) = item else {
                return None;
            };
            Some((decl.name.clone(), decl))
        })
        .collect()
}

fn collect_enums(module: &Module) -> HashMap<String, &EnumDecl> {
    module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Enum(decl) = item else {
                return None;
            };
            Some((decl.name.clone(), decl))
        })
        .collect()
}

fn collect_types(module: &Module) -> HashMap<String, &TypeDecl> {
    module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Type(decl) = item else {
                return None;
            };
            Some((decl.name.clone(), decl))
        })
        .collect()
}

pub(crate) fn is_local_struct_type(ty: &TypeExpr, structs: &HashMap<String, &StructDecl>) -> bool {
    let TypeExpr::Path { path } = ty else {
        return false;
    };

    path.segments.len() == 1 && structs.contains_key(&path.segments[0])
}

pub(crate) fn path_is(path: &Path, expected: &[&str]) -> bool {
    path.segments.len() == expected.len()
        && path
            .segments
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
}

pub(crate) fn path_name(path: &Path) -> String {
    path.segments.join(".")
}

pub(crate) fn type_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Path { path } => path_name(path),
        TypeExpr::Generic { base, args, .. } => {
            let args = args.iter().map(type_name).collect::<Vec<_>>().join(", ");
            format!("{}<{args}>", path_name(base))
        }
        TypeExpr::Option { inner, .. } => format!("{}?", type_name(inner)),
    }
}

pub(crate) fn same_type(left: &TypeExpr, right: &TypeExpr) -> bool {
    match (left, right) {
        (TypeExpr::Path { path: left }, TypeExpr::Path { path: right }) => {
            left.segments == right.segments
        }
        (
            TypeExpr::Generic {
                base: left_base,
                args: left_args,
                ..
            },
            TypeExpr::Generic {
                base: right_base,
                args: right_args,
                ..
            },
        ) => {
            left_base.segments == right_base.segments
                && left_args.len() == right_args.len()
                && left_args
                    .iter()
                    .zip(right_args)
                    .all(|(left, right)| same_type(left, right))
        }
        (TypeExpr::Option { inner: left, .. }, TypeExpr::Option { inner: right, .. }) => {
            same_type(left, right)
        }
        _ => false,
    }
}

pub(crate) fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    matches!(first, 'A'..='Z' | 'a'..='z' | '_')
        && chars.all(|ch| matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_'))
}
