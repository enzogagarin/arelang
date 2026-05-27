use are_ast::{
    Block, CallArg, EnumDecl, EnumVariant, Expr, Field, FieldValidation, FunctionBody,
    FunctionDecl, Item, ModelDecl, ModelField, ModelFieldAttr, Module, ObjectField, Param, Path,
    Pattern, RawBlock, RouteDecl, RouteStatus, ServiceDecl, ServiceUse, Stmt, StructDecl, TypeDecl,
    TypeExpr, UseDecl,
};
use are_diagnostics::{Diagnostic, Position, SourceRange};
use are_lexer::{Keyword, Token, TokenKind};
use std::path::{Path as FsPath, PathBuf};

mod support;

#[cfg(test)]
mod tests;

use support::unquote;

#[must_use]
pub fn parse_tokens(file: &FsPath, tokens: &[Token]) -> (Option<Module>, Vec<Diagnostic>) {
    Parser::new(file, tokens).parse_module()
}

struct Parser<'a> {
    file: PathBuf,
    tokens: &'a [Token],
    index: usize,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn new(file: &FsPath, tokens: &'a [Token]) -> Self {
        Self {
            file: file.to_path_buf(),
            tokens,
            index: 0,
            diagnostics: Vec::new(),
        }
    }

    fn parse_module(mut self) -> (Option<Module>, Vec<Diagnostic>) {
        let mut items = Vec::new();

        while !self.at_eof() {
            match self.parse_item() {
                Some(item) => items.push(item),
                None => self.recover_top_level(),
            }
        }

        let module = if self.diagnostics.is_empty() {
            Some(Module { items })
        } else {
            None
        };

        (module, self.diagnostics)
    }

    fn parse_item(&mut self) -> Option<Item> {
        if self.match_keyword(Keyword::Use).is_some() {
            return self.parse_use().map(Item::Use);
        }

        if self.match_keyword(Keyword::Type).is_some() {
            return self.parse_type_decl().map(Item::Type);
        }

        if self.match_keyword(Keyword::Struct).is_some() {
            return self.parse_struct().map(Item::Struct);
        }

        if self.match_keyword(Keyword::Model).is_some() {
            return self.parse_model().map(Item::Model);
        }

        if self.match_keyword(Keyword::Enum).is_some() {
            return self.parse_enum().map(Item::Enum);
        }

        if self.match_keyword(Keyword::Fn).is_some() {
            return self.parse_function().map(Item::Function);
        }

        if self.match_keyword(Keyword::Service).is_some() {
            return self.parse_service().map(Item::Service);
        }

        let token = self.peek();
        self.error_at_current(
            "E_PARSE_0001",
            "expected top-level item",
            format!(
                "expected `use`, `type`, `struct`, `model`, `enum`, `fn`, or `service`, found `{}`",
                token.lexeme
            ),
        );
        None
    }

    fn parse_use(&mut self) -> Option<UseDecl> {
        let start = self.previous_range()?.start;
        let path = self.parse_path()?;
        let alias = if self.match_keyword(Keyword::As).is_some() {
            Some(self.expect_identifier("expected import alias")?)
        } else {
            None
        };
        let end = self.previous_range()?.end;

        Some(UseDecl {
            path,
            alias,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_type_decl(&mut self) -> Option<TypeDecl> {
        let start = self.previous_range()?.start;
        let name = self.expect_identifier("expected type name")?;
        self.expect_kind(&TokenKind::Equals, "expected `=` after type name")?;
        let opaque = self.match_keyword(Keyword::Opaque).is_some();
        let aliased = self.parse_type_expr()?;
        let end = aliased.range().end;

        Some(TypeDecl {
            name,
            aliased,
            opaque,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_struct(&mut self) -> Option<StructDecl> {
        let start = self.previous_range()?.start;
        let name = self.expect_identifier("expected struct name")?;
        self.expect_kind(&TokenKind::LeftBrace, "expected `{` after struct name")?;

        let mut fields = Vec::new();
        while !self.at_eof() && !self.check_kind(&TokenKind::RightBrace) {
            fields.push(self.parse_field(true)?);
        }

        let end = self
            .expect_kind(&TokenKind::RightBrace, "expected `}` after struct fields")?
            .range
            .end;

        Some(StructDecl {
            name,
            fields,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_model(&mut self) -> Option<ModelDecl> {
        let start = self.previous_range()?.start;
        let name = self.expect_identifier("expected model name")?;
        self.expect_kind(&TokenKind::LeftBrace, "expected `{` after model name")?;

        let mut fields = Vec::new();
        while !self.at_eof() && !self.check_kind(&TokenKind::RightBrace) {
            fields.push(self.parse_model_field()?);
        }

        let end = self
            .expect_kind(&TokenKind::RightBrace, "expected `}` after model fields")?
            .range
            .end;

        Some(ModelDecl {
            name,
            fields,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_model_field(&mut self) -> Option<ModelField> {
        let start = self.peek().range.start;
        let name = self.expect_identifier("expected model field name")?;
        self.expect_kind(&TokenKind::Colon, "expected `:` after model field name")?;
        let ty = self.parse_type_expr()?;
        let mut attrs = Vec::new();

        while let Some(attr) = self.match_model_field_attr() {
            attrs.push(attr);
        }

        let end = self.previous_range()?.end;
        Some(ModelField {
            name,
            ty,
            attrs,
            range: SourceRange::new(start, end),
        })
    }

    fn match_model_field_attr(&mut self) -> Option<ModelFieldAttr> {
        if self.peek().kind != TokenKind::Identifier {
            return None;
        }

        let attr = match self.peek().lexeme.as_str() {
            "primary" => ModelFieldAttr::Primary,
            "unique" => ModelFieldAttr::Unique,
            _ => return None,
        };
        self.advance();
        Some(attr)
    }

    fn parse_enum(&mut self) -> Option<EnumDecl> {
        let start = self.previous_range()?.start;
        let name = self.expect_identifier("expected enum name")?;
        self.expect_kind(&TokenKind::LeftBrace, "expected `{` after enum name")?;

        let mut variants = Vec::new();
        while !self.at_eof() && !self.check_kind(&TokenKind::RightBrace) {
            variants.push(self.parse_enum_variant()?);
        }

        let end = self
            .expect_kind(&TokenKind::RightBrace, "expected `}` after enum variants")?
            .range
            .end;

        Some(EnumDecl {
            name,
            variants,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_enum_variant(&mut self) -> Option<EnumVariant> {
        let start = self.peek().range.start;
        let name = self.expect_identifier("expected enum variant name")?;
        let mut payload = Vec::new();

        if self.match_kind(&TokenKind::LeftParen).is_some() {
            if !self.check_kind(&TokenKind::RightParen) {
                loop {
                    payload.push(self.parse_field(false)?);
                    if self.match_kind(&TokenKind::Comma).is_none() {
                        break;
                    }
                }
            }
            self.expect_kind(&TokenKind::RightParen, "expected `)` after enum payload")?;
        }

        let end = self.previous_range()?.end;
        Some(EnumVariant {
            name,
            payload,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_function(&mut self) -> Option<FunctionDecl> {
        let start = self.previous_range()?.start;
        let name = self.expect_identifier("expected function name")?;
        let params = self.parse_param_list()?;
        let return_type = if self.match_kind(&TokenKind::Arrow).is_some() {
            Some(self.parse_type_expr()?)
        } else {
            None
        };
        let body = self.parse_function_body()?;
        let end = body.range().end;

        Some(FunctionDecl {
            name,
            params,
            return_type,
            body,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_service(&mut self) -> Option<ServiceDecl> {
        let start = self.previous_range()?.start;
        let name = self.expect_identifier("expected service name")?;
        let state_param = if self.match_kind(&TokenKind::LeftParen).is_some() {
            let param = if self.check_kind(&TokenKind::RightParen) {
                None
            } else {
                Some(self.parse_param()?)
            };
            self.expect_kind(&TokenKind::RightParen, "expected `)` after service state")?;
            param
        } else {
            None
        };

        self.expect_kind(
            &TokenKind::LeftBrace,
            "expected `{` after service declaration",
        )?;
        let mut uses = Vec::new();
        let mut routes = Vec::new();

        while !self.at_eof() && !self.check_kind(&TokenKind::RightBrace) {
            if self.match_keyword(Keyword::Use).is_some() {
                uses.push(self.parse_service_use()?);
            } else if self.match_keyword(Keyword::Route).is_some() {
                routes.push(self.parse_route_after_route_keyword()?);
            } else if self.check_route_method() {
                routes.push(self.parse_route_shorthand()?);
            } else {
                self.error_at_current(
                    "E_PARSE_0002",
                    "expected service item",
                    "service bodies accept `use`, `route`, and HTTP method items such as `get` or `post`",
                );
                self.advance();
            }
        }

        let end = self
            .expect_kind(&TokenKind::RightBrace, "expected `}` after service body")?
            .range
            .end;

        Some(ServiceDecl {
            name,
            state_param,
            uses,
            routes,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_service_use(&mut self) -> Option<ServiceUse> {
        let start = self.previous_range()?.start;
        let target = self.parse_path()?;
        let mut args = Vec::new();
        if self.match_kind(&TokenKind::LeftParen).is_some() {
            args = self.parse_service_use_args()?;
        }
        let end = self.previous_range()?.end;

        Some(ServiceUse {
            target,
            args,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_service_use_args(&mut self) -> Option<Vec<Path>> {
        let mut args = Vec::new();

        if !self.check_kind(&TokenKind::RightParen) {
            loop {
                args.push(self.parse_path()?);
                if self.match_kind(&TokenKind::Comma).is_none() {
                    break;
                }
            }
        }

        self.expect_kind(&TokenKind::RightParen, "expected `)` after service use")?;
        Some(args)
    }

    fn parse_route_after_route_keyword(&mut self) -> Option<RouteDecl> {
        let start = self.previous_range()?.start;
        let method = self.expect_identifier("expected HTTP method after `route`")?;
        self.parse_route_contract(start, &method)
    }

    fn parse_route_shorthand(&mut self) -> Option<RouteDecl> {
        let method_token = self.advance();
        self.parse_route_contract(method_token.range.start, &method_token.lexeme)
    }

    fn parse_route_contract(&mut self, start: Position, method: &str) -> Option<RouteDecl> {
        let path_token = self.expect_kind(&TokenKind::String, "expected route path string")?;
        let mut body_type = None;
        let mut query_type = None;
        let mut headers_type = None;
        let mut cookies_type = None;
        while self.check_identifier("body")
            || self.check_identifier("query")
            || self.check_identifier("headers")
            || self.check_identifier("cookies")
        {
            if let Some(keyword_range) = self.match_identifier("body") {
                let parsed_body_type = self.parse_type_expr()?;
                if body_type.is_some() {
                    self.diagnostics.push(Diagnostic::error(
                        "E_PARSE_0010",
                        &self.file,
                        SourceRange::new(keyword_range.start, parsed_body_type.range().end),
                        "duplicate route body contract",
                        "a route can declare at most one `body Payload` clause",
                    ));
                } else {
                    body_type = Some(parsed_body_type);
                }
            } else if let Some(keyword_range) = self.match_identifier("query") {
                let parsed_query_type = self.parse_type_expr()?;
                if query_type.is_some() {
                    self.diagnostics.push(Diagnostic::error(
                        "E_PARSE_0010",
                        &self.file,
                        SourceRange::new(keyword_range.start, parsed_query_type.range().end),
                        "duplicate route query contract",
                        "a route can declare at most one `query Payload` clause",
                    ));
                } else {
                    query_type = Some(parsed_query_type);
                }
            } else if let Some(keyword_range) = self.match_identifier("headers") {
                let parsed_headers_type = self.parse_type_expr()?;
                if headers_type.is_some() {
                    self.diagnostics.push(Diagnostic::error(
                        "E_PARSE_0010",
                        &self.file,
                        SourceRange::new(keyword_range.start, parsed_headers_type.range().end),
                        "duplicate route headers contract",
                        "a route can declare at most one `headers Payload` clause",
                    ));
                } else {
                    headers_type = Some(parsed_headers_type);
                }
            } else if let Some(keyword_range) = self.match_identifier("cookies") {
                let parsed_cookies_type = self.parse_type_expr()?;
                if cookies_type.is_some() {
                    self.diagnostics.push(Diagnostic::error(
                        "E_PARSE_0010",
                        &self.file,
                        SourceRange::new(keyword_range.start, parsed_cookies_type.range().end),
                        "duplicate route cookies contract",
                        "a route can declare at most one `cookies Payload` clause",
                    ));
                } else {
                    cookies_type = Some(parsed_cookies_type);
                }
            }
        }
        self.expect_kind(&TokenKind::Arrow, "expected `->` before route handler")?;
        let handler = self.parse_path()?;
        let mut end = handler.range.end;
        let response_type = if self.match_identifier("returns").is_some() {
            let response_type = self.parse_type_expr()?;
            end = response_type.range().end;
            Some(response_type)
        } else {
            None
        };
        let status = if let Some(range) = self.match_identifier("status") {
            let status_token =
                self.expect_kind(&TokenKind::Integer, "expected integer HTTP status code")?;
            end = status_token.range.end;
            Some(RouteStatus {
                value: status_token.lexeme.parse().unwrap_or(0),
                range: SourceRange::new(range.start, status_token.range.end),
            })
        } else {
            None
        };

        Some(RouteDecl {
            method: method.to_ascii_uppercase(),
            path: unquote(&path_token.lexeme),
            body_type,
            query_type,
            headers_type,
            cookies_type,
            handler,
            response_type,
            status,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_field(&mut self, allow_validations: bool) -> Option<Field> {
        let start = self.peek().range.start;
        let name = self.expect_identifier("expected field name")?;
        self.expect_kind(&TokenKind::Colon, "expected `:` after field name")?;
        let ty = self.parse_type_expr()?;
        let mut validations = Vec::new();

        if allow_validations {
            while self.check_field_validation_start() {
                validations.push(self.parse_field_validation()?);
            }
        }

        let end = validations
            .last()
            .map_or_else(|| ty.range().end, |validation| validation.range().end);

        Some(Field {
            name,
            ty,
            validations,
            range: SourceRange::new(start, end),
        })
    }

    fn check_field_validation_start(&self) -> bool {
        self.check_identifier("validate") && self.check_next_kind(&TokenKind::Dot)
    }

    fn parse_field_validation(&mut self) -> Option<FieldValidation> {
        let start = self.peek().range.start;
        self.expect_identifier("expected field validation namespace")?;
        self.expect_kind(&TokenKind::Dot, "expected `.` after validation namespace")?;
        let name = self.expect_identifier("expected field validation name")?;

        match name.as_str() {
            "email" => {
                let end = self.previous_range()?.end;
                Some(FieldValidation::Email {
                    range: SourceRange::new(start, end),
                })
            }
            "length" => self.parse_length_validation(start),
            _ => {
                let range = self.previous_range()?;
                self.diagnostics.push(Diagnostic::error(
                    "E_PARSE_0011",
                    &self.file,
                    range,
                    format!("unknown field validation `validate.{name}`"),
                    "supported field validations are `validate.email` and `validate.length(min: N, max: N)`",
                ));
                None
            }
        }
    }

    fn parse_length_validation(&mut self, start: Position) -> Option<FieldValidation> {
        self.expect_kind(
            &TokenKind::LeftParen,
            "expected `(` after `validate.length`",
        )?;
        let mut min = None;
        let mut max = None;

        if !self.check_kind(&TokenKind::RightParen) {
            loop {
                let label_range = self.peek().range;
                let label = self.expect_identifier("expected validation argument label")?;
                self.expect_kind(&TokenKind::Colon, "expected `:` after validation argument")?;
                let value_token =
                    self.expect_kind(&TokenKind::Integer, "expected integer validation argument")?;
                let value = value_token.lexeme.parse::<i64>().unwrap_or(0);

                match label.as_str() {
                    "min" if min.is_none() => min = Some(value),
                    "max" if max.is_none() => max = Some(value),
                    "min" | "max" => {
                        self.diagnostics.push(Diagnostic::error(
                            "E_PARSE_0011",
                            &self.file,
                            SourceRange::new(label_range.start, value_token.range.end),
                            format!("duplicate validation argument `{label}`"),
                            "`validate.length` accepts each of `min` and `max` once",
                        ));
                        return None;
                    }
                    _ => {
                        self.diagnostics.push(Diagnostic::error(
                            "E_PARSE_0011",
                            &self.file,
                            label_range,
                            format!("unknown validation argument `{label}`"),
                            "`validate.length` accepts `min` and `max` integer arguments",
                        ));
                        return None;
                    }
                }

                if self.match_kind(&TokenKind::Comma).is_none() {
                    break;
                }
            }
        }

        let end = self
            .expect_kind(
                &TokenKind::RightParen,
                "expected `)` after validation arguments",
            )?
            .range
            .end;
        let range = SourceRange::new(start, end);

        let (Some(min), Some(max)) = (min, max) else {
            self.diagnostics.push(Diagnostic::error(
                "E_PARSE_0011",
                &self.file,
                range,
                "missing validation argument",
                "`validate.length` requires both `min` and `max` integer arguments",
            ));
            return None;
        };

        Some(FieldValidation::Length { min, max, range })
    }

    fn parse_param_list(&mut self) -> Option<Vec<Param>> {
        self.expect_kind(&TokenKind::LeftParen, "expected `(` before parameter list")?;
        let mut params = Vec::new();

        if !self.check_kind(&TokenKind::RightParen) {
            loop {
                params.push(self.parse_param()?);
                if self.match_kind(&TokenKind::Comma).is_none() {
                    break;
                }
            }
        }

        self.expect_kind(&TokenKind::RightParen, "expected `)` after parameter list")?;
        Some(params)
    }

    fn parse_param(&mut self) -> Option<Param> {
        let start = self.peek().range.start;
        let name = self.expect_identifier("expected parameter name")?;
        self.expect_kind(&TokenKind::Colon, "expected `:` after parameter name")?;
        let ty = self.parse_type_expr()?;
        let end = ty.range().end;

        Some(Param {
            name,
            ty,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_type_expr(&mut self) -> Option<TypeExpr> {
        let base = self.parse_path()?;
        let mut ty = if self.match_operator("<") {
            let start = base.range.start;
            let mut args = Vec::new();
            if !self.check_operator(">") {
                loop {
                    args.push(self.parse_type_expr()?);
                    if self.match_kind(&TokenKind::Comma).is_none() {
                        break;
                    }
                }
            }
            let end = self
                .expect_operator(">", "expected `>` after generic arguments")?
                .end;
            TypeExpr::Generic {
                base,
                args,
                range: SourceRange::new(start, end),
            }
        } else {
            TypeExpr::Path { path: base }
        };

        if let Some(question) = self.match_kind(&TokenKind::Question) {
            let range = SourceRange::new(ty.range().start, question.end);
            ty = TypeExpr::Option {
                inner: Box::new(ty),
                range,
            };
        }

        Some(ty)
    }

    fn parse_path(&mut self) -> Option<Path> {
        let start = self.peek().range.start;
        let mut segments = vec![self.expect_identifier("expected identifier")?];

        while self.match_kind(&TokenKind::Dot).is_some() {
            segments.push(self.expect_identifier("expected path segment after `.`")?);
        }

        let end = self.previous_range()?.end;
        Some(Path {
            segments,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_function_body(&mut self) -> Option<FunctionBody> {
        let start = self
            .expect_kind(&TokenKind::LeftBrace, "expected function body block")?
            .range
            .start;

        if self.can_start_statement() {
            return self.parse_statement_block(start);
        }

        self.parse_raw_block_after_open(start)
            .map(|block| FunctionBody::Raw { block })
    }

    fn can_start_statement(&self) -> bool {
        self.check_keyword(Keyword::Let)
            || self.check_keyword(Keyword::Return)
            || self.check_keyword(Keyword::Ensure)
            || self.check_keyword(Keyword::Match)
            || self.check_kind(&TokenKind::Identifier)
    }

    fn parse_statement_block(&mut self, block_start: Position) -> Option<FunctionBody> {
        let mut statements = Vec::new();

        while !self.at_eof() && !self.check_kind(&TokenKind::RightBrace) {
            statements.push(self.parse_statement()?);
        }

        let block_end = self
            .expect_kind(&TokenKind::RightBrace, "expected `}` after function body")?
            .range
            .end;

        Some(FunctionBody::Parsed {
            block: Block {
                statements,
                range: SourceRange::new(block_start, block_end),
            },
        })
    }

    fn parse_statement(&mut self) -> Option<Stmt> {
        if self.check_keyword(Keyword::Let) {
            return self.parse_let_statement();
        }

        if self.check_keyword(Keyword::Return) {
            return self.parse_return_statement();
        }

        if self.check_keyword(Keyword::Ensure) {
            return self.parse_ensure_statement();
        }

        if self.check_keyword(Keyword::Match) {
            return self.parse_match_statement();
        }

        let value = self.parse_expr()?;
        let range = value.range();
        Some(Stmt::Expr { value, range })
    }

    fn parse_let_statement(&mut self) -> Option<Stmt> {
        let start = self
            .match_keyword(Keyword::Let)
            .expect("let statement starts with let")
            .start;
        let name = self.expect_identifier("expected binding name after `let`")?;
        self.expect_kind(&TokenKind::Equals, "expected `=` after binding name")?;
        let value = self.parse_expr()?;
        let end = value.range().end;

        Some(Stmt::Let {
            name,
            value,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_return_statement(&mut self) -> Option<Stmt> {
        let return_start = self
            .match_keyword(Keyword::Return)
            .expect("return block starts with return")
            .start;
        let value = self.parse_expr()?;
        let stmt_end = value.range().end;
        Some(Stmt::Return {
            value,
            range: SourceRange::new(return_start, stmt_end),
        })
    }

    fn parse_ensure_statement(&mut self) -> Option<Stmt> {
        let start = self
            .match_keyword(Keyword::Ensure)
            .expect("ensure statement starts with ensure")
            .start;
        let condition = self.parse_expr()?;
        self.expect_kind(&TokenKind::Comma, "expected `,` after ensure condition")?;
        let error = self.parse_expr()?;
        let end = error.range().end;

        Some(Stmt::Ensure {
            condition,
            error,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_match_statement(&mut self) -> Option<Stmt> {
        let start = self
            .match_keyword(Keyword::Match)
            .expect("match statement starts with match")
            .start;
        let value = self.parse_expr()?;
        self.expect_kind(&TokenKind::LeftBrace, "expected `{` after match value")?;

        let mut arms = Vec::new();
        while !self.at_eof() && !self.check_kind(&TokenKind::RightBrace) {
            arms.push(self.parse_match_arm()?);
        }

        let end = self
            .expect_kind(&TokenKind::RightBrace, "expected `}` after match arms")?
            .range
            .end;

        Some(Stmt::Match {
            value,
            arms,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_match_arm(&mut self) -> Option<are_ast::MatchArm> {
        let pattern = self.parse_pattern()?;
        self.expect_kind(&TokenKind::FatArrow, "expected `=>` after match pattern")?;
        let body = self.parse_statement()?;
        let range = SourceRange::new(pattern.range().start, body.range().end);

        Some(are_ast::MatchArm {
            pattern,
            body: Box::new(body),
            range,
        })
    }

    fn parse_pattern(&mut self) -> Option<Pattern> {
        let start = self.peek().range.start;
        let name = self.expect_identifier("expected match variant name")?;
        let mut bindings = Vec::new();

        if self.match_kind(&TokenKind::LeftParen).is_some() {
            if !self.check_kind(&TokenKind::RightParen) {
                loop {
                    bindings.push(self.expect_identifier("expected payload binding name")?);
                    if self.match_kind(&TokenKind::Comma).is_none() {
                        break;
                    }
                }
            }
            self.expect_kind(&TokenKind::RightParen, "expected `)` after pattern payload")?;
        }

        let end = self.previous_range()?.end;
        Some(Pattern::Variant {
            name,
            bindings,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_expr(&mut self) -> Option<Expr> {
        let mut expr = self.parse_primary_expr()?;

        while let Some(question) = self.match_kind(&TokenKind::Question) {
            let range = SourceRange::new(expr.range().start, question.end);
            expr = Expr::Try {
                value: Box::new(expr),
                range,
            };
        }

        Some(expr)
    }

    fn parse_primary_expr(&mut self) -> Option<Expr> {
        if self.check_kind(&TokenKind::String) {
            let token = self.advance();
            return Some(Expr::String {
                value: unquote(&token.lexeme),
                range: token.range,
            });
        }

        if self.check_kind(&TokenKind::Integer) {
            let token = self.advance();
            let Ok(value) = token.lexeme.parse::<i64>() else {
                self.diagnostics.push(Diagnostic::error(
                    "E_PARSE_0008",
                    &self.file,
                    token.range,
                    "invalid integer literal",
                    format!("could not parse `{}` as a 64-bit integer", token.lexeme),
                ));
                return None;
            };

            return Some(Expr::Integer {
                value,
                range: token.range,
            });
        }

        if self.check_keyword(Keyword::True) || self.check_keyword(Keyword::False) {
            let token = self.advance();
            return Some(Expr::Bool {
                value: token.lexeme == "true",
                range: token.range,
            });
        }

        if self.check_kind(&TokenKind::LeftBrace) {
            return self.parse_object_expr();
        }

        if self.check_kind(&TokenKind::Identifier) {
            return self.parse_call_or_path_expr();
        }

        self.error_at_current(
            "E_PARSE_0007",
            "expected expression",
            format!("found `{}`", self.peek().lexeme),
        );
        None
    }

    fn parse_object_expr(&mut self) -> Option<Expr> {
        let start = self
            .expect_kind(&TokenKind::LeftBrace, "expected object literal")?
            .range
            .start;
        let mut fields = Vec::new();

        if !self.check_kind(&TokenKind::RightBrace) {
            loop {
                let key_token =
                    self.expect_kind(&TokenKind::String, "expected object field string key")?;
                let key = unquote(&key_token.lexeme);
                self.expect_kind(&TokenKind::Colon, "expected `:` after object field key")?;
                let value = self.parse_expr()?;
                let field_end = value.range().end;

                fields.push(ObjectField {
                    key,
                    value,
                    range: SourceRange::new(key_token.range.start, field_end),
                });

                if self.match_kind(&TokenKind::Comma).is_none() {
                    break;
                }

                if self.check_kind(&TokenKind::RightBrace) {
                    break;
                }
            }
        }

        let end = self
            .expect_kind(&TokenKind::RightBrace, "expected `}` after object literal")?
            .range
            .end;

        Some(Expr::Object {
            fields,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_call_or_path_expr(&mut self) -> Option<Expr> {
        let path = self.parse_path()?;
        let type_args = if self.match_operator("<") {
            self.parse_call_type_args()?
        } else {
            Vec::new()
        };

        if self.match_kind(&TokenKind::LeftParen).is_none() {
            if !type_args.is_empty() {
                self.error_at_current(
                    "E_PARSE_0009",
                    "expected call after generic arguments",
                    format!("found `{}`", self.peek().lexeme),
                );
                return None;
            }

            return Some(Expr::Path { path });
        }

        let mut args = Vec::new();
        if !self.check_kind(&TokenKind::RightParen) {
            loop {
                args.push(self.parse_call_arg()?);
                if self.match_kind(&TokenKind::Comma).is_none() {
                    break;
                }
            }
        }

        let end = self
            .expect_kind(&TokenKind::RightParen, "expected `)` after call arguments")?
            .range
            .end;
        let range = SourceRange::new(path.range.start, end);

        Some(Expr::Call {
            callee: path,
            type_args,
            args,
            range,
        })
    }

    fn parse_call_type_args(&mut self) -> Option<Vec<TypeExpr>> {
        let mut args = Vec::new();
        if !self.check_operator(">") {
            loop {
                args.push(self.parse_type_expr()?);
                if self.match_kind(&TokenKind::Comma).is_none() {
                    break;
                }
            }
        }
        self.expect_operator(">", "expected `>` after call type arguments")?;
        Some(args)
    }

    fn parse_call_arg(&mut self) -> Option<CallArg> {
        if self.check_kind(&TokenKind::Identifier) && self.check_next_kind(&TokenKind::Colon) {
            let start = self.peek().range.start;
            let label = self.expect_identifier("expected argument label")?;
            self.expect_kind(&TokenKind::Colon, "expected `:` after argument label")?;
            let value = self.parse_expr()?;
            let end = value.range().end;

            return Some(CallArg {
                label: Some(label),
                value,
                range: SourceRange::new(start, end),
            });
        }

        let value = self.parse_expr()?;
        let range = value.range();
        Some(CallArg {
            label: None,
            value,
            range,
        })
    }

    fn parse_raw_block_after_open(&mut self, start: Position) -> Option<RawBlock> {
        let mut depth = 1usize;
        let mut token_count = 0usize;

        while !self.at_eof() {
            let token = self.advance();
            token_count += 1;

            match token.kind {
                TokenKind::LeftBrace => depth += 1,
                TokenKind::RightBrace => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(RawBlock {
                            token_count,
                            range: SourceRange::new(start, token.range.end),
                        });
                    }
                }
                TokenKind::Eof => break,
                _ => {}
            }
        }

        self.diagnostics.push(Diagnostic::error(
            "E_PARSE_0003",
            &self.file,
            SourceRange::new(start, start),
            "unterminated block",
            "function bodies must end with a matching `}`",
        ));
        None
    }
}
