use are_ast::{
    Block, CallArg, EnumDecl, EnumVariant, Expr, Field, FunctionBody, FunctionDecl, Item,
    ModelDecl, ModelField, ModelFieldAttr, Module, ObjectField, Param, Path, Pattern, RawBlock,
    RouteDecl, ServiceDecl, ServiceUse, Stmt, StructDecl, TypeDecl, TypeExpr, UseDecl,
};
use are_diagnostics::{Diagnostic, Position, SourceRange};
use are_lexer::{Keyword, Token, TokenKind};
use std::path::{Path as FsPath, PathBuf};

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
            fields.push(self.parse_field()?);
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
                    payload.push(self.parse_field()?);
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
                routes.push(self.parse_route()?);
            } else {
                self.error_at_current(
                    "E_PARSE_0002",
                    "expected service item",
                    "service bodies currently accept only `use` and `route` items",
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

    fn parse_route(&mut self) -> Option<RouteDecl> {
        let start = self.previous_range()?.start;
        let method = self.expect_identifier("expected HTTP method after `route`")?;
        let path_token = self.expect_kind(&TokenKind::String, "expected route path string")?;
        self.expect_kind(&TokenKind::Arrow, "expected `->` before route handler")?;
        let handler = self.parse_path()?;
        let end = handler.range.end;

        Some(RouteDecl {
            method,
            path: unquote(&path_token.lexeme),
            handler,
            range: SourceRange::new(start, end),
        })
    }

    fn parse_field(&mut self) -> Option<Field> {
        let start = self.peek().range.start;
        let name = self.expect_identifier("expected field name")?;
        self.expect_kind(&TokenKind::Colon, "expected `:` after field name")?;
        let ty = self.parse_type_expr()?;
        let end = ty.range().end;

        Some(Field {
            name,
            ty,
            range: SourceRange::new(start, end),
        })
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

    fn match_keyword(&mut self, keyword: Keyword) -> Option<SourceRange> {
        if self.check_keyword(keyword) {
            return Some(self.advance().range);
        }
        None
    }

    fn check_keyword(&self, keyword: Keyword) -> bool {
        matches!(self.peek().kind, TokenKind::Keyword(found) if found == keyword)
    }

    fn match_kind(&mut self, kind: &TokenKind) -> Option<SourceRange> {
        if self.check_kind(kind) {
            return Some(self.advance().range);
        }
        None
    }

    fn check_kind(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    fn check_next_kind(&self, kind: &TokenKind) -> bool {
        self.tokens
            .get(self.index + 1)
            .is_some_and(|token| &token.kind == kind)
    }

    fn match_operator(&mut self, expected: &str) -> bool {
        if self.check_operator(expected) {
            self.advance();
            return true;
        }
        false
    }

    fn check_operator(&self, expected: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Operator(operator) if operator == expected)
    }

    fn expect_operator(&mut self, expected: &str, problem: &str) -> Option<SourceRange> {
        if self.check_operator(expected) {
            return Some(self.advance().range);
        }
        self.error_at_current(
            "E_PARSE_0005",
            problem,
            format!("found `{}`", self.peek().lexeme),
        );
        None
    }

    fn expect_kind(&mut self, kind: &TokenKind, problem: &str) -> Option<Token> {
        if self.check_kind(kind) {
            return Some(self.advance().clone());
        }

        self.error_at_current(
            "E_PARSE_0005",
            problem,
            format!("found `{}`", self.peek().lexeme),
        );
        None
    }

    fn expect_identifier(&mut self, problem: &str) -> Option<String> {
        if self.peek().kind == TokenKind::Identifier {
            return Some(self.advance().lexeme.clone());
        }

        self.error_at_current(
            "E_PARSE_0006",
            problem,
            format!("found `{}`", self.peek().lexeme),
        );
        None
    }

    fn recover_top_level(&mut self) {
        while !self.at_eof() {
            if matches!(
                self.peek().kind,
                TokenKind::Keyword(
                    Keyword::Use
                        | Keyword::Type
                        | Keyword::Struct
                        | Keyword::Model
                        | Keyword::Enum
                        | Keyword::Fn
                        | Keyword::Service
                )
            ) {
                return;
            }
            self.advance();
        }
    }

    fn error_at_current(
        &mut self,
        code: impl Into<String>,
        problem: impl Into<String>,
        reason: impl Into<String>,
    ) {
        let token = self.peek();
        self.diagnostics.push(Diagnostic::error(
            code,
            &self.file,
            token.range,
            problem,
            reason,
        ));
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Eof)
    }

    fn peek(&self) -> &Token {
        self.tokens
            .get(self.index)
            .or_else(|| self.tokens.last())
            .expect("parser requires at least EOF token")
    }

    fn advance(&mut self) -> Token {
        let token = self.peek().clone();
        if !matches!(token.kind, TokenKind::Eof) {
            self.index += 1;
        }
        token
    }

    fn previous_range(&self) -> Option<SourceRange> {
        self.tokens
            .get(self.index.checked_sub(1)?)
            .map(|token| token.range)
    }
}

fn unquote(lexeme: &str) -> String {
    lexeme
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or(lexeme)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::parse_tokens;
    use are_ast::{Expr, FunctionBody, FunctionDecl, Item, Module, Stmt};
    use are_lexer::lex_source;
    use std::path::Path;

    #[test]
    fn parses_users_api_shape() {
        let source = include_str!("../../../examples/users_api/main.are");
        let file = Path::new("examples/users_api/main.are");
        let (tokens, lex_diagnostics) = lex_source(file, source);
        assert!(lex_diagnostics.is_empty());

        let (module, diagnostics) = parse_tokens(file, &tokens);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");

        let module = module.expect("module parses");
        assert_eq!(module.items.len(), 14);
        assert!(matches!(module.items.last(), Some(Item::Service(_))));
        assert!(module.items.iter().any(|item| {
            matches!(
                item,
                Item::Model(model)
                    if model.name == "User"
                        && model.fields.iter().any(|field| field.name == "id")
                        && model.fields.iter().any(|field| field.name == "email")
            )
        }));

        let health = function_named(&module, "health");
        let FunctionBody::Parsed { block } = &health.body else {
            panic!("health body should parse into a return block");
        };
        let Some(Stmt::Return { value, .. }) = block.statements.first() else {
            panic!("health should return a response");
        };
        assert!(matches!(value, Expr::Call { .. }));

        let validate_user = function_named(&module, "validate_user");
        let FunctionBody::Parsed { block } = &validate_user.body else {
            panic!("validate_user body should parse into statements");
        };
        assert_eq!(block.statements.len(), 3);
        assert!(matches!(
            block.statements.first(),
            Some(Stmt::Ensure { .. })
        ));

        let create_user = function_named(&module, "create_user");
        let FunctionBody::Parsed { block } = &create_user.body else {
            panic!("create_user body should parse into statements");
        };
        assert_eq!(block.statements.len(), 3);
        assert!(matches!(block.statements.first(), Some(Stmt::Let { .. })));
        assert!(matches!(block.statements.last(), Some(Stmt::Return { .. })));

        let get_user = function_named(&module, "get_user");
        let FunctionBody::Parsed { block } = &get_user.body else {
            panic!("get_user body should parse into statements");
        };
        assert_eq!(block.statements.len(), 3);
        assert!(matches!(block.statements.first(), Some(Stmt::Let { .. })));
        assert!(matches!(block.statements.last(), Some(Stmt::Return { .. })));

        let map_error = function_named(&module, "map_error");
        let FunctionBody::Parsed { block } = &map_error.body else {
            panic!("map_error body should parse into a match statement");
        };
        assert!(matches!(
            block.statements.first(),
            Some(Stmt::Match { arms, .. }) if arms.len() == 3
        ));
    }

    fn function_named<'a>(module: &'a Module, name: &str) -> &'a FunctionDecl {
        module
            .items
            .iter()
            .find_map(|item| {
                if let Item::Function(function) = item {
                    (function.name == name).then_some(function)
                } else {
                    None
                }
            })
            .expect("function exists")
    }

    #[test]
    fn parses_service_routes() {
        let source = r#"
            service UsersApi(state: AppState) {
                use Http.error_map(map_error)
                route GET "/health" -> health
                route POST "/users" -> create_user
            }
        "#;
        let file = Path::new("test.are");
        let (tokens, lex_diagnostics) = lex_source(file, source);
        assert!(lex_diagnostics.is_empty());

        let (module, diagnostics) = parse_tokens(file, &tokens);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");

        let module = module.expect("module parses");
        let Some(Item::Service(service)) = module.items.first() else {
            panic!("expected service");
        };

        assert_eq!(service.routes.len(), 2);
        assert_eq!(service.uses.len(), 1);
        assert_eq!(service.uses[0].target.segments, ["Http", "error_map"]);
        assert_eq!(service.uses[0].args[0].segments, ["map_error"]);
        assert_eq!(service.routes[0].method, "GET");
        assert_eq!(service.routes[0].path, "/health");
    }
}
