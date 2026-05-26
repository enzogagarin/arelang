use are_ast::{
    EnumDecl, FunctionDecl, Item, ModelDecl, Module, Path, ServiceDecl, StructDecl, TypeDecl,
    TypeExpr, UseDecl,
};
use are_diagnostics::{Diagnostic, SourceRange, best_name_suggestion};
use std::collections::{HashMap, HashSet};
use std::path::{Path as FsPath, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolKind {
    Import,
    Type,
    Struct,
    Model,
    Enum,
    Function,
    Service,
}

#[derive(Debug, Clone, Copy)]
struct Symbol {
    kind: SymbolKind,
    range: SourceRange,
}

#[must_use]
pub fn resolve_module(file: &FsPath, module: &Module) -> Vec<Diagnostic> {
    Resolver::new(file, module).resolve()
}

struct Resolver<'a> {
    file: PathBuf,
    module: &'a Module,
    symbols: HashMap<String, Symbol>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Resolver<'a> {
    fn new(file: &FsPath, module: &'a Module) -> Self {
        Self {
            file: file.to_path_buf(),
            module,
            symbols: HashMap::new(),
            diagnostics: Vec::new(),
        }
    }

    fn resolve(mut self) -> Vec<Diagnostic> {
        self.collect_symbols();
        self.resolve_items();
        self.diagnostics
    }

    fn collect_symbols(&mut self) {
        for item in &self.module.items {
            match item {
                Item::Use(decl) => self.declare_use(decl),
                Item::Type(decl) => self.declare(&decl.name, SymbolKind::Type, decl.range),
                Item::Struct(decl) => self.declare(&decl.name, SymbolKind::Struct, decl.range),
                Item::Model(decl) => self.declare(&decl.name, SymbolKind::Model, decl.range),
                Item::Enum(decl) => self.declare(&decl.name, SymbolKind::Enum, decl.range),
                Item::Function(decl) => {
                    self.declare(&decl.name, SymbolKind::Function, decl.range);
                }
                Item::Service(decl) => self.declare(&decl.name, SymbolKind::Service, decl.range),
            }
        }
    }

    fn resolve_items(&mut self) {
        for item in &self.module.items {
            match item {
                Item::Use(_) => {}
                Item::Type(decl) => self.resolve_type_decl(decl),
                Item::Struct(decl) => self.resolve_struct(decl),
                Item::Model(decl) => self.resolve_model(decl),
                Item::Enum(decl) => self.resolve_enum(decl),
                Item::Function(decl) => self.resolve_function(decl),
                Item::Service(decl) => self.resolve_service(decl),
            }
        }
    }

    fn declare_use(&mut self, decl: &UseDecl) {
        let Some(name) = decl
            .alias
            .as_ref()
            .or_else(|| decl.path.segments.last())
            .map(String::as_str)
        else {
            return;
        };

        self.declare(name, SymbolKind::Import, decl.range);
    }

    fn declare(&mut self, name: &str, kind: SymbolKind, range: SourceRange) {
        if let Some(previous) = self.symbols.get(name).copied() {
            self.diagnostics.push(
                Diagnostic::error(
                    "E_RESOLVE_0001",
                    &self.file,
                    range,
                    format!("duplicate declaration `{name}`"),
                    format!(
                        "`{name}` was already declared as {:?} at {}:{}",
                        previous.kind, previous.range.start.line, previous.range.start.column
                    ),
                )
                .with_fix("rename this declaration or remove the duplicate", None),
            );
            return;
        }

        self.symbols
            .insert(name.to_string(), Symbol { kind, range });
    }

    fn resolve_type_decl(&mut self, decl: &TypeDecl) {
        self.resolve_type_expr(&decl.aliased);
    }

    fn resolve_struct(&mut self, decl: &StructDecl) {
        for field in &decl.fields {
            self.resolve_type_expr(&field.ty);
        }
    }

    fn resolve_model(&mut self, decl: &ModelDecl) {
        for field in &decl.fields {
            self.resolve_type_expr(&field.ty);
        }
    }

    fn resolve_enum(&mut self, decl: &EnumDecl) {
        for variant in &decl.variants {
            for field in &variant.payload {
                self.resolve_type_expr(&field.ty);
            }
        }
    }

    fn resolve_function(&mut self, decl: &FunctionDecl) {
        for param in &decl.params {
            self.resolve_type_expr(&param.ty);
        }

        if let Some(return_type) = &decl.return_type {
            self.resolve_type_expr(return_type);
        }
    }

    fn resolve_service(&mut self, decl: &ServiceDecl) {
        if let Some(state_param) = &decl.state_param {
            self.resolve_type_expr(&state_param.ty);
        }

        for service_use in &decl.uses {
            self.resolve_imported_or_local_path(&service_use.target);
        }

        let mut routes = HashSet::new();
        for route in &decl.routes {
            if let Some(body_type) = &route.body_type {
                self.resolve_type_expr(body_type);
            }
            if let Some(response_type) = &route.response_type {
                self.resolve_type_expr(response_type);
            }

            let key = (route.method.as_str(), route.path.as_str());
            if !routes.insert(key) {
                self.diagnostics.push(
                    Diagnostic::error(
                        "E_RESOLVE_0005",
                        &self.file,
                        route.range,
                        format!("duplicate route `{} {}`", route.method, route.path),
                        "each service route must have a unique method and path pair",
                    )
                    .with_fix("remove one duplicate route", None),
                );
            }

            self.resolve_route_handler(&route.handler);
        }
    }

    fn resolve_type_expr(&mut self, ty: &TypeExpr) {
        match ty {
            TypeExpr::Path { path } => self.resolve_type_path(path),
            TypeExpr::Generic { base, args, .. } => {
                self.resolve_type_path(base);
                for arg in args {
                    self.resolve_type_expr(arg);
                }
            }
            TypeExpr::Option { inner, .. } => self.resolve_type_expr(inner),
        }
    }

    fn resolve_type_path(&mut self, path: &Path) {
        let Some(root) = path.segments.first() else {
            return;
        };

        if is_builtin_type(root) {
            return;
        }

        if path.segments.len() > 1 {
            if self.is_import(root) || self.is_type_like(root) {
                return;
            }

            self.unknown_path(
                path,
                "E_RESOLVE_0003",
                format!("unknown type namespace `{root}`"),
                "qualified type paths must start with an import alias or a declared type",
                self.import_or_type_suggestion(root),
            );
            return;
        }

        if self.is_type_like(root) {
            return;
        }

        match self.symbols.get(root).copied() {
            Some(symbol) => self.diagnostics.push(Diagnostic::error(
                "E_RESOLVE_0004",
                &self.file,
                path.range,
                format!("`{root}` is not a type"),
                format!("`{root}` was declared as {:?}", symbol.kind),
            )),
            None => self.unknown_path(
                path,
                "E_RESOLVE_0003",
                format!("unknown type `{root}`"),
                "declare the type in this module, import it, or use a builtin type",
                self.type_suggestion(root),
            ),
        }
    }

    fn resolve_imported_or_local_path(&mut self, path: &Path) {
        let Some(root) = path.segments.first() else {
            return;
        };

        if self.is_import(root) || self.symbols.contains_key(root) {
            return;
        }

        self.unknown_path(
            path,
            "E_RESOLVE_0002",
            format!("unknown symbol `{root}`"),
            "service use paths must start with an import alias or local declaration",
            self.symbol_suggestion(root),
        );
    }

    fn resolve_route_handler(&mut self, path: &Path) {
        if path.segments.len() != 1 {
            self.diagnostics.push(Diagnostic::error(
                "E_RESOLVE_0006",
                &self.file,
                path.range,
                "route handler must be a local function",
                "the HTTP MVP accepts single-name route handlers such as `create_user`",
            ));
            return;
        }

        let handler = &path.segments[0];
        match self.symbols.get(handler).copied() {
            Some(Symbol {
                kind: SymbolKind::Function,
                ..
            }) => {}
            Some(symbol) => self.diagnostics.push(Diagnostic::error(
                "E_RESOLVE_0006",
                &self.file,
                path.range,
                format!("route handler `{handler}` is not a function"),
                format!("`{handler}` was declared as {:?}", symbol.kind),
            )),
            None => self.unknown_path(
                path,
                "E_RESOLVE_0002",
                format!("unknown route handler `{handler}`"),
                "declare a function with this name before wiring it in a service route",
                self.function_suggestion(handler),
            ),
        }
    }

    fn is_import(&self, name: &str) -> bool {
        self.symbols
            .get(name)
            .is_some_and(|symbol| symbol.kind == SymbolKind::Import)
    }

    fn is_type_like(&self, name: &str) -> bool {
        self.symbols.get(name).is_some_and(|symbol| {
            matches!(
                symbol.kind,
                SymbolKind::Type | SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Model
            )
        })
    }

    fn unknown_path(
        &mut self,
        path: &Path,
        code: &'static str,
        problem: String,
        reason: &'static str,
        suggestion: Option<String>,
    ) {
        let mut diagnostic = Diagnostic::error(code, &self.file, path.range, problem, reason);
        if let Some(suggestion) = suggestion {
            diagnostic =
                diagnostic.with_fix(format!("did you mean `{suggestion}`?"), Some(suggestion));
        }
        self.diagnostics.push(diagnostic);
    }

    fn function_suggestion(&self, name: &str) -> Option<String> {
        self.suggestion(name, |kind| kind == SymbolKind::Function)
    }

    fn type_suggestion(&self, name: &str) -> Option<String> {
        self.suggestion(name, SymbolKind::is_type_like)
    }

    fn import_or_type_suggestion(&self, name: &str) -> Option<String> {
        self.suggestion(name, |kind| {
            kind == SymbolKind::Import || kind.is_type_like()
        })
    }

    fn symbol_suggestion(&self, name: &str) -> Option<String> {
        self.suggestion(name, |_| true)
    }

    fn suggestion(&self, name: &str, accepts: impl Fn(SymbolKind) -> bool) -> Option<String> {
        let mut candidates = self
            .symbols
            .iter()
            .filter_map(|(candidate, symbol)| accepts(symbol.kind).then_some(candidate.as_str()))
            .collect::<Vec<_>>();
        candidates.sort_unstable();

        best_name_suggestion(name, candidates).map(str::to_string)
    }
}

impl SymbolKind {
    const fn is_type_like(self) -> bool {
        matches!(self, Self::Type | Self::Struct | Self::Enum | Self::Model)
    }
}

fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "String"
            | "Str"
            | "Bool"
            | "Unit"
            | "I8"
            | "I16"
            | "I32"
            | "I64"
            | "U8"
            | "U16"
            | "U32"
            | "U64"
            | "F32"
            | "F64"
            | "Result"
            | "Option"
    )
}

#[cfg(test)]
mod tests {
    use super::resolve_module;
    use are_lexer::lex_source;
    use are_parser::parse_tokens;
    use std::path::Path;

    #[test]
    fn resolves_users_api_shape() {
        let source = include_str!("../../../examples/users_api/main.are");
        let diagnostics = diagnostics_for("examples/users_api/main.are", source);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    }

    #[test]
    fn reports_unknown_route_handler() {
        let source = r#"
            struct AppState {}
            fn create_user() {}

            service UsersApi(state: AppState) {
                route GET "/missing" -> create_usr
            }
        "#;
        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "E_RESOLVE_0002");
        assert_eq!(diagnostics[0].fixes[0].label, "did you mean `create_user`?");
    }

    #[test]
    fn reports_duplicate_top_level_name() {
        let source = r"
            struct User {}
            fn User() {}
        ";
        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "E_RESOLVE_0001");
    }

    #[test]
    fn reports_unknown_type_reference() {
        let source = r"
            struct UserStore {}

            struct AppState {
                users: UserSotre
            }
        ";
        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "E_RESOLVE_0003");
        assert_eq!(diagnostics[0].fixes[0].label, "did you mean `UserStore`?");
    }

    #[test]
    fn reports_duplicate_service_route() {
        let source = r#"
            struct AppState {}

            fn health() {}

            service UsersApi(state: AppState) {
                route GET "/health" -> health
                route GET "/health" -> health
            }
        "#;
        let diagnostics = diagnostics_for("test.are", source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "E_RESOLVE_0005");
    }

    fn diagnostics_for(file_name: &str, source: &str) -> Vec<are_diagnostics::Diagnostic> {
        let file = Path::new(file_name);
        let (tokens, lex_diagnostics) = lex_source(file, source);
        assert!(lex_diagnostics.is_empty(), "{lex_diagnostics:#?}");

        let (module, parse_diagnostics) = parse_tokens(file, &tokens);
        assert!(parse_diagnostics.is_empty(), "{parse_diagnostics:#?}");
        let module = module.expect("module parses");

        resolve_module(file, &module)
    }
}
