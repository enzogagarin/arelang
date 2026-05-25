use are_ast::{
    EnumDecl, EnumVariant, Field, FunctionDecl, Item, Module, Param, Path, RawBlock, RouteDecl,
    ServiceDecl, ServiceUse, StructDecl, TypeDecl, TypeExpr, UseDecl,
};
use are_diagnostics::{Diagnostic, SourceRange};
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
                "expected `use`, `type`, `struct`, `enum`, `fn`, or `service`, found `{}`",
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
        let body = self.parse_raw_block()?;
        let end = body.range.end;

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
        if self.match_kind(&TokenKind::LeftParen).is_some() {
            self.skip_balanced_parens()?;
        }
        let end = self.previous_range()?.end;

        Some(ServiceUse {
            target,
            range: SourceRange::new(start, end),
        })
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

    fn parse_raw_block(&mut self) -> Option<RawBlock> {
        let start = self
            .expect_kind(&TokenKind::LeftBrace, "expected function body block")?
            .range
            .start;
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

    fn skip_balanced_parens(&mut self) -> Option<()> {
        let start = self.previous_range()?.start;
        let mut depth = 1usize;

        while !self.at_eof() {
            let token = self.advance();
            match token.kind {
                TokenKind::LeftParen => depth += 1,
                TokenKind::RightParen => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(());
                    }
                }
                TokenKind::Eof => break,
                _ => {}
            }
        }

        self.diagnostics.push(Diagnostic::error(
            "E_PARSE_0004",
            &self.file,
            SourceRange::new(start, start),
            "unterminated call",
            "service use calls must end with a matching `)`",
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
    use are_ast::Item;
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
        assert_eq!(module.items.len(), 13);
        assert!(matches!(module.items.last(), Some(Item::Service(_))));
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
        assert_eq!(service.routes[0].method, "GET");
        assert_eq!(service.routes[0].path, "/health");
    }
}
