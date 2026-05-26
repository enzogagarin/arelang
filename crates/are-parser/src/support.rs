use super::Parser;
use are_diagnostics::{Diagnostic, SourceRange};
use are_lexer::{Keyword, Token, TokenKind};

impl Parser<'_> {
    pub(super) fn match_keyword(&mut self, keyword: Keyword) -> Option<SourceRange> {
        if self.check_keyword(keyword) {
            return Some(self.advance().range);
        }
        None
    }

    pub(super) fn check_keyword(&self, keyword: Keyword) -> bool {
        matches!(self.peek().kind, TokenKind::Keyword(found) if found == keyword)
    }

    pub(super) fn match_kind(&mut self, kind: &TokenKind) -> Option<SourceRange> {
        if self.check_kind(kind) {
            return Some(self.advance().range);
        }
        None
    }

    pub(super) fn check_kind(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    pub(super) fn check_next_kind(&self, kind: &TokenKind) -> bool {
        self.tokens
            .get(self.index + 1)
            .is_some_and(|token| &token.kind == kind)
    }

    pub(super) fn match_identifier(&mut self, expected: &str) -> Option<SourceRange> {
        if self.peek().kind == TokenKind::Identifier && self.peek().lexeme == expected {
            return Some(self.advance().range);
        }
        None
    }

    pub(super) fn check_route_method(&self) -> bool {
        self.peek().kind == TokenKind::Identifier
            && matches!(
                self.peek().lexeme.to_ascii_uppercase().as_str(),
                "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
            )
    }

    pub(super) fn match_operator(&mut self, expected: &str) -> bool {
        if self.check_operator(expected) {
            self.advance();
            return true;
        }
        false
    }

    pub(super) fn check_operator(&self, expected: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Operator(operator) if operator == expected)
    }

    pub(super) fn expect_operator(&mut self, expected: &str, problem: &str) -> Option<SourceRange> {
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

    pub(super) fn expect_kind(&mut self, kind: &TokenKind, problem: &str) -> Option<Token> {
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

    pub(super) fn expect_identifier(&mut self, problem: &str) -> Option<String> {
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

    pub(super) fn recover_top_level(&mut self) {
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

    pub(super) fn error_at_current(
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

    pub(super) fn at_eof(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Eof)
    }

    pub(super) fn peek(&self) -> &Token {
        self.tokens
            .get(self.index)
            .or_else(|| self.tokens.last())
            .expect("parser requires at least EOF token")
    }

    pub(super) fn advance(&mut self) -> Token {
        let token = self.peek().clone();
        if !matches!(token.kind, TokenKind::Eof) {
            self.index += 1;
        }
        token
    }

    pub(super) fn previous_range(&self) -> Option<SourceRange> {
        self.tokens
            .get(self.index.checked_sub(1)?)
            .map(|token| token.range)
    }
}

pub(super) fn unquote(lexeme: &str) -> String {
    lexeme
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or(lexeme)
        .to_string()
}
