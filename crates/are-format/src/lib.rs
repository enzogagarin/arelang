use are_ast::{
    Block, CallArg, EnumDecl, Expr, Field, FieldValidation, FunctionBody, FunctionDecl, Item,
    MatchArm, ModelDecl, ModelField, ModelFieldAttr, Module, ObjectField, Param, Path, Pattern,
    ServiceDecl, Stmt, StructDecl, TypeDecl, TypeExpr, UseDecl,
};
use are_diagnostics::{Diagnostic, Position, SourceRange};
use are_lexer::lex_source;
use are_parser::parse_tokens;
use std::path::Path as FsPath;

const INDENT: &str = "    ";

/// Format one Arelang source file.
///
/// # Errors
///
/// Returns lexer/parser diagnostics, plus formatter diagnostics for syntax the
/// current formatter intentionally refuses to rewrite.
pub fn format_source(file: &FsPath, source: &str) -> Result<String, Vec<Diagnostic>> {
    if let Some(range) = first_comment_range(source) {
        return Err(vec![Diagnostic::error(
            "E_FMT_0001",
            file,
            range,
            "comments are not preserved by are fmt yet",
            "remove the comment before formatting or wait for comment-preserving formatting",
        )]);
    }

    let (tokens, lex_diagnostics) = lex_source(file, source);
    if !lex_diagnostics.is_empty() {
        return Err(lex_diagnostics);
    }

    let (module, parse_diagnostics) = parse_tokens(file, &tokens);
    if !parse_diagnostics.is_empty() {
        return Err(parse_diagnostics);
    }

    let Some(module) = module else {
        return Err(Vec::new());
    };

    let mut diagnostics = Vec::new();
    reject_unsupported_constructs(file, &module, &mut diagnostics);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    Ok(format_module(&module))
}

#[must_use]
pub fn format_module(module: &Module) -> String {
    let mut output = String::new();

    for (index, item) in module.items.iter().enumerate() {
        if index > 0 && needs_blank_line(&module.items[index - 1], item) {
            output.push('\n');
        }
        format_item(&mut output, item);
    }

    output
}

fn needs_blank_line(left: &Item, right: &Item) -> bool {
    !matches!(
        (left, right),
        (Item::Use(_), Item::Use(_)) | (Item::Type(_), Item::Type(_))
    )
}

fn reject_unsupported_constructs(
    file: &FsPath,
    module: &Module,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for item in &module.items {
        if let Item::Function(function) = item
            && let FunctionBody::Raw { block } = &function.body
        {
            diagnostics.push(Diagnostic::error(
                "E_FMT_0002",
                file,
                block.range,
                format!("function `{}` cannot be formatted yet", function.name),
                "are fmt currently supports parsed Arelang statement bodies only",
            ));
        }

        if let Item::Function(function) = item
            && let FunctionBody::Parsed { block } = &function.body
        {
            reject_unsupported_block(file, block, diagnostics);
        }
    }
}

fn reject_unsupported_block(file: &FsPath, block: &Block, diagnostics: &mut Vec<Diagnostic>) {
    for statement in &block.statements {
        reject_unsupported_statement(file, statement, diagnostics);
    }
}

fn reject_unsupported_statement(
    file: &FsPath,
    statement: &Stmt,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Stmt::Match { arms, .. } = statement {
        for arm in arms {
            if matches!(arm.body.as_ref(), Stmt::Match { .. }) {
                diagnostics.push(Diagnostic::error(
                    "E_FMT_0003",
                    file,
                    arm.body.range(),
                    "nested match arms cannot be formatted yet",
                    "move the nested match into a helper function before formatting",
                ));
            }
            reject_unsupported_statement(file, arm.body.as_ref(), diagnostics);
        }
    }
}

fn format_item(output: &mut String, item: &Item) {
    match item {
        Item::Use(decl) => format_use(output, decl),
        Item::Type(decl) => format_type_decl(output, decl),
        Item::Struct(decl) => format_struct(output, decl),
        Item::Model(decl) => format_model(output, decl),
        Item::Enum(decl) => format_enum(output, decl),
        Item::Function(decl) => format_function(output, decl),
        Item::Service(decl) => format_service(output, decl),
    }
}

fn format_use(output: &mut String, decl: &UseDecl) {
    output.push_str("use ");
    output.push_str(&format_path(&decl.path));
    if let Some(alias) = &decl.alias {
        output.push_str(" as ");
        output.push_str(alias);
    }
    output.push('\n');
}

fn format_type_decl(output: &mut String, decl: &TypeDecl) {
    output.push_str("type ");
    output.push_str(&decl.name);
    output.push_str(" = ");
    if decl.opaque {
        output.push_str("opaque ");
    }
    output.push_str(&format_type_expr(&decl.aliased));
    output.push('\n');
}

fn format_struct(output: &mut String, decl: &StructDecl) {
    output.push_str("struct ");
    output.push_str(&decl.name);
    format_fields(output, &decl.fields);
}

fn format_model(output: &mut String, decl: &ModelDecl) {
    output.push_str("model ");
    output.push_str(&decl.name);
    if decl.fields.is_empty() {
        output.push_str(" {}\n");
        return;
    }

    output.push_str(" {\n");
    for field in &decl.fields {
        output.push_str(INDENT);
        format_model_field(output, field);
        output.push('\n');
    }
    output.push_str("}\n");
}

fn format_enum(output: &mut String, decl: &EnumDecl) {
    output.push_str("enum ");
    output.push_str(&decl.name);
    if decl.variants.is_empty() {
        output.push_str(" {}\n");
        return;
    }

    output.push_str(" {\n");
    for variant in &decl.variants {
        output.push_str(INDENT);
        output.push_str(&variant.name);
        if !variant.payload.is_empty() {
            output.push('(');
            output.push_str(
                &variant
                    .payload
                    .iter()
                    .map(format_field_inline)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            output.push(')');
        }
        output.push('\n');
    }
    output.push_str("}\n");
}

fn format_function(output: &mut String, decl: &FunctionDecl) {
    output.push_str("fn ");
    output.push_str(&decl.name);
    output.push('(');
    output.push_str(
        &decl
            .params
            .iter()
            .map(format_param)
            .collect::<Vec<_>>()
            .join(", "),
    );
    output.push(')');
    if let Some(return_type) = &decl.return_type {
        output.push_str(" -> ");
        output.push_str(&format_type_expr(return_type));
    }
    output.push(' ');

    let FunctionBody::Parsed { block } = &decl.body else {
        return;
    };
    format_block(output, block, 0);
}

fn format_service(output: &mut String, decl: &ServiceDecl) {
    output.push_str("service ");
    output.push_str(&decl.name);
    if let Some(state_param) = &decl.state_param {
        output.push('(');
        output.push_str(&format_param(state_param));
        output.push(')');
    }
    output.push_str(" {\n");

    for service_use in &decl.uses {
        output.push_str(INDENT);
        output.push_str("use ");
        output.push_str(&format_path(&service_use.target));
        if !service_use.args.is_empty() {
            output.push('(');
            output.push_str(
                &service_use
                    .args
                    .iter()
                    .map(format_path)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            output.push(')');
        }
        output.push('\n');
    }

    if !decl.uses.is_empty() && !decl.routes.is_empty() {
        output.push('\n');
    }

    for route in &decl.routes {
        output.push_str(INDENT);
        output.push_str(&route.method.to_ascii_lowercase());
        output.push(' ');
        output.push_str(&quote_string(&route.path));
        if let Some(body_type) = &route.body_type {
            output.push_str(" body ");
            output.push_str(&format_type_expr(body_type));
        }
        if let Some(query_type) = &route.query_type {
            output.push_str(" query ");
            output.push_str(&format_type_expr(query_type));
        }
        if let Some(headers_type) = &route.headers_type {
            output.push_str(" headers ");
            output.push_str(&format_type_expr(headers_type));
        }
        if let Some(cookies_type) = &route.cookies_type {
            output.push_str(" cookies ");
            output.push_str(&format_type_expr(cookies_type));
        }
        output.push_str(" -> ");
        output.push_str(&format_path(&route.handler));
        if let Some(response_type) = &route.response_type {
            output.push_str(" returns ");
            output.push_str(&format_type_expr(response_type));
        }
        if let Some(status) = route.status {
            output.push_str(" status ");
            output.push_str(&status.value.to_string());
        }
        output.push('\n');
    }

    output.push_str("}\n");
}

fn format_fields(output: &mut String, fields: &[Field]) {
    if fields.is_empty() {
        output.push_str(" {}\n");
        return;
    }

    output.push_str(" {\n");
    for field in fields {
        output.push_str(INDENT);
        output.push_str(&format_field_inline(field));
        output.push('\n');
    }
    output.push_str("}\n");
}

fn format_model_field(output: &mut String, field: &ModelField) {
    output.push_str(&field.name);
    output.push_str(": ");
    output.push_str(&format_type_expr(&field.ty));
    for attr in &field.attrs {
        output.push(' ');
        output.push_str(match attr {
            ModelFieldAttr::Primary => "primary",
            ModelFieldAttr::Unique => "unique",
        });
    }
}

fn format_block(output: &mut String, block: &Block, indent: usize) {
    output.push_str("{\n");
    for statement in &block.statements {
        format_statement(output, statement, indent + 1);
    }
    output.push_str(&indent_string(indent));
    output.push_str("}\n");
}

fn format_statement(output: &mut String, statement: &Stmt, indent: usize) {
    output.push_str(&indent_string(indent));
    match statement {
        Stmt::Let { name, value, .. } => {
            output.push_str("let ");
            output.push_str(name);
            output.push_str(" = ");
            output.push_str(&format_expr(value));
            output.push('\n');
        }
        Stmt::Expr { value, .. } => {
            output.push_str(&format_expr(value));
            output.push('\n');
        }
        Stmt::Return { value, .. } => {
            output.push_str("return ");
            output.push_str(&format_expr(value));
            output.push('\n');
        }
        Stmt::Ensure {
            condition, error, ..
        } => {
            output.push_str("ensure ");
            output.push_str(&format_expr(condition));
            output.push_str(", ");
            output.push_str(&format_expr(error));
            output.push('\n');
        }
        Stmt::Match { value, arms, .. } => {
            output.push_str("match ");
            output.push_str(&format_expr(value));
            output.push_str(" {\n");
            for arm in arms {
                output.push_str(&indent_string(indent + 1));
                format_match_arm(output, arm);
                output.push('\n');
            }
            output.push_str(&indent_string(indent));
            output.push_str("}\n");
        }
    }
}

fn format_match_arm(output: &mut String, arm: &MatchArm) {
    output.push_str(&format_pattern(&arm.pattern));
    output.push_str(" => ");
    output.push_str(&format_statement_inline(&arm.body));
}

fn format_statement_inline(statement: &Stmt) -> String {
    match statement {
        Stmt::Let { name, value, .. } => format!("let {name} = {}", format_expr(value)),
        Stmt::Expr { value, .. } => format_expr(value),
        Stmt::Return { value, .. } => format!("return {}", format_expr(value)),
        Stmt::Ensure {
            condition, error, ..
        } => {
            format!("ensure {}, {}", format_expr(condition), format_expr(error))
        }
        Stmt::Match { value, .. } => format!("match {} {{ ... }}", format_expr(value)),
    }
}

fn format_pattern(pattern: &Pattern) -> String {
    match pattern {
        Pattern::Variant { name, bindings, .. } if bindings.is_empty() => name.clone(),
        Pattern::Variant { name, bindings, .. } => {
            format!("{name}({})", bindings.join(", "))
        }
    }
}

fn format_expr(expr: &Expr) -> String {
    match expr {
        Expr::String { value, .. } => quote_string(value),
        Expr::Integer { value, .. } => value.to_string(),
        Expr::Bool { value, .. } => value.to_string(),
        Expr::Object { fields, .. } => format_object(fields),
        Expr::Call {
            callee,
            type_args,
            args,
            ..
        } => format_call(callee, type_args, args),
        Expr::Try { value, .. } => format!("{}?", format_expr(value)),
        Expr::Path { path } => format_path(path),
    }
}

fn format_object(fields: &[ObjectField]) -> String {
    if fields.is_empty() {
        return "{}".to_string();
    }

    format!(
        "{{ {} }}",
        fields
            .iter()
            .map(|field| format!(
                "{}: {}",
                quote_string(&field.key),
                format_expr(&field.value)
            ))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn format_call(callee: &Path, type_args: &[TypeExpr], args: &[CallArg]) -> String {
    let mut output = format_path(callee);
    if !type_args.is_empty() {
        output.push('<');
        output.push_str(
            &type_args
                .iter()
                .map(format_type_expr)
                .collect::<Vec<_>>()
                .join(", "),
        );
        output.push('>');
    }
    output.push('(');
    output.push_str(
        &args
            .iter()
            .map(format_call_arg)
            .collect::<Vec<_>>()
            .join(", "),
    );
    output.push(')');
    output
}

fn format_call_arg(arg: &CallArg) -> String {
    if let Some(label) = &arg.label {
        return format!("{label}: {}", format_expr(&arg.value));
    }

    format_expr(&arg.value)
}

fn format_type_expr(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Path { path } => format_path(path),
        TypeExpr::Generic { base, args, .. } => {
            format!(
                "{}<{}>",
                format_path(base),
                args.iter()
                    .map(format_type_expr)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        TypeExpr::Option { inner, .. } => format!("{}?", format_type_expr(inner)),
    }
}

fn format_field_inline(field: &Field) -> String {
    let mut output = format!("{}: {}", field.name, format_type_expr(&field.ty));
    for validation in &field.validations {
        output.push(' ');
        output.push_str(&format_field_validation(validation));
    }
    output
}

fn format_field_validation(validation: &FieldValidation) -> String {
    match validation {
        FieldValidation::Email { .. } => "validate.email".to_string(),
        FieldValidation::Length { min, max, .. } => {
            format!("validate.length(min: {min}, max: {max})")
        }
    }
}

fn format_param(param: &Param) -> String {
    format!("{}: {}", param.name, format_type_expr(&param.ty))
}

fn format_path(path: &Path) -> String {
    path.segments.join(".")
}

fn quote_string(value: &str) -> String {
    let mut output = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            _ => output.push(ch),
        }
    }
    output.push('"');
    output
}

fn indent_string(level: usize) -> String {
    INDENT.repeat(level)
}

fn first_comment_range(source: &str) -> Option<SourceRange> {
    let mut chars = source.chars().peekable();
    let mut line = 1usize;
    let mut column = 1usize;
    let mut in_string = false;

    while let Some(ch) = chars.next() {
        let start = Position::new(line, column);
        match ch {
            '"' if !in_string => {
                in_string = true;
                column += 1;
            }
            '"' if in_string => {
                in_string = false;
                column += 1;
            }
            '\\' if in_string => {
                column += 1;
                if chars.next().is_some() {
                    column += 1;
                }
            }
            '/' if !in_string && matches!(chars.peek(), Some('/' | '*')) => {
                let end = Position::new(line, column + 2);
                return Some(SourceRange::new(start, end));
            }
            '\n' => {
                line += 1;
                column = 1;
            }
            _ => column += 1,
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::format_source;
    use std::path::Path;

    #[test]
    fn formats_minimal_service() {
        let source = r#"use   std.http   as   Http
struct AppState{}
fn ping(ctx:Http.Context<AppState>,req:Http.Request)->Http.Response{return Http.Response.ok({"message":"pong"})}
service HelloApi(state:AppState){route GET "/ping"->ping returns PingResponse status 200}
"#;

        let formatted = format_source(Path::new("test.are"), source).expect("formats");
        assert_eq!(
            formatted,
            r#"use std.http as Http

struct AppState {}

fn ping(ctx: Http.Context<AppState>, req: Http.Request) -> Http.Response {
    return Http.Response.ok({ "message": "pong" })
}

service HelloApi(state: AppState) {
    get "/ping" -> ping returns PingResponse status 200
}
"#
        );
    }

    #[test]
    fn formats_route_contracts() {
        let source = r#"use std.http as Http
struct AppState{}
struct CreateUserInput{name:String}
model User{id:U64 primary name:String}
fn create_user(ctx:Http.Context<AppState>,req:Http.Request)->Http.Response{return Http.Response.ok({})}
fn get_user(ctx:Http.Context<AppState>,req:Http.Request)->Http.Response{return Http.Response.ok({})}
service UsersApi(state:AppState){route POST "/users" body CreateUserInput -> create_user returns User status 201
route GET "/users/{id: UserId}" -> get_user returns User status 200}
"#;

        let formatted = format_source(Path::new("test.are"), source).expect("formats");
        assert!(formatted.contains(
            r#"post "/users" body CreateUserInput -> create_user returns User status 201"#
        ));
        assert!(
            formatted.contains(r#"get "/users/{id: UserId}" -> get_user returns User status 200"#)
        );
    }

    #[test]
    fn leaves_users_api_canonical() {
        let source = include_str!("../../../examples/users_api/main.are");
        let formatted = format_source(Path::new("users.are"), source).expect("formats");
        assert_eq!(formatted, source);
    }

    #[test]
    fn rejects_comments_until_preservation_exists() {
        let diagnostics =
            format_source(Path::new("test.are"), "use std.http as Http // hi\n").unwrap_err();
        assert_eq!(diagnostics[0].code, "E_FMT_0001");
    }

    #[test]
    fn rejects_nested_match_arm_bodies_until_multiline_arm_formatting_exists() {
        let diagnostics = format_source(
            Path::new("test.are"),
            r#"enum Outer {
    A
    B
}

enum Inner {
    C
    D
}

fn choose(outer: Outer, inner: Inner) -> String {
    match outer {
        A => match inner {
            C => return "c"
            D => return "d"
        }
        B => return "b"
    }
}
"#,
        )
        .unwrap_err();
        assert_eq!(diagnostics[0].code, "E_FMT_0003");
    }
}
