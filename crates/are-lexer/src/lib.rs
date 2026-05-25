use are_diagnostics::{Diagnostic, Position, SourceRange};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum TokenKind {
    Identifier,
    Keyword(Keyword),
    Integer,
    Float,
    String,
    LeftBrace,
    RightBrace,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    Comma,
    Dot,
    Colon,
    Arrow,
    FatArrow,
    Bang,
    Question,
    Equals,
    Operator(String),
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Keyword {
    Use,
    As,
    Pub,
    Fn,
    Let,
    Mut,
    Struct,
    Enum,
    Type,
    Opaque,
    If,
    Else,
    While,
    For,
    In,
    Return,
    Break,
    Continue,
    Match,
    Service,
    Route,
    Test,
    Unsafe,
    Foreign,
    True,
    False,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Token {
    pub kind: TokenKind,
    pub lexeme: String,
    pub range: SourceRange,
}

#[must_use]
pub fn lex_source(file: &Path, source: &str) -> (Vec<Token>, Vec<Diagnostic>) {
    Lexer::new(file, source).lex()
}

struct Lexer<'a> {
    file: &'a Path,
    chars: Vec<char>,
    index: usize,
    line: usize,
    column: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    fn new(file: &'a Path, source: &'a str) -> Self {
        Self {
            file,
            chars: source.chars().collect(),
            index: 0,
            line: 1,
            column: 1,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn lex(mut self) -> (Vec<Token>, Vec<Diagnostic>) {
        while let Some(ch) = self.peek() {
            match ch {
                ' ' | '\t' | '\r' => {
                    self.advance();
                }
                '\n' => {
                    self.advance_newline();
                }
                '/' if self.peek_next() == Some('/') => self.skip_line_comment(),
                '/' if self.peek_next() == Some('*') => self.skip_block_comment(),
                '"' => self.lex_string(),
                '0'..='9' => self.lex_number(),
                'A'..='Z' | 'a'..='z' | '_' => self.lex_identifier_or_keyword(),
                '{' => self.single(TokenKind::LeftBrace),
                '}' => self.single(TokenKind::RightBrace),
                '(' => self.single(TokenKind::LeftParen),
                ')' => self.single(TokenKind::RightParen),
                '[' => self.single(TokenKind::LeftBracket),
                ']' => self.single(TokenKind::RightBracket),
                ',' => self.single(TokenKind::Comma),
                '.' => self.single(TokenKind::Dot),
                ':' => self.single(TokenKind::Colon),
                '!' => self.single(TokenKind::Bang),
                '?' => self.single(TokenKind::Question),
                '=' => {
                    if self.peek_next() == Some('>') {
                        self.double(TokenKind::FatArrow);
                    } else if self.peek_next() == Some('=') {
                        self.operator(2);
                    } else {
                        self.single(TokenKind::Equals);
                    }
                }
                '-' if self.peek_next() == Some('>') => self.double(TokenKind::Arrow),
                '+' | '-' | '*' | '/' | '%' | '<' | '>' => self.operator(1),
                _ => self.invalid_character(),
            }
        }

        let pos = self.position();
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            lexeme: String::new(),
            range: SourceRange::new(pos, pos),
        });

        (self.tokens, self.diagnostics)
    }

    fn single(&mut self, kind: TokenKind) {
        let start = self.position();
        let lexeme = self.advance().expect("single token exists").to_string();
        let end = self.position();
        self.tokens.push(Token {
            kind,
            lexeme,
            range: SourceRange::new(start, end),
        });
    }

    fn double(&mut self, kind: TokenKind) {
        let start = self.position();
        let first = self.advance().expect("double token first char exists");
        let second = self.advance().expect("double token second char exists");
        let end = self.position();
        self.tokens.push(Token {
            kind,
            lexeme: [first, second].iter().collect(),
            range: SourceRange::new(start, end),
        });
    }

    fn operator(&mut self, width: usize) {
        let start = self.position();
        let mut lexeme = String::new();
        for _ in 0..width {
            if let Some(ch) = self.advance() {
                lexeme.push(ch);
            }
        }
        let end = self.position();
        self.tokens.push(Token {
            kind: TokenKind::Operator(lexeme.clone()),
            lexeme,
            range: SourceRange::new(start, end),
        });
    }

    fn lex_identifier_or_keyword(&mut self) {
        let start = self.position();
        let start_index = self.index;

        while matches!(self.peek(), Some('A'..='Z' | 'a'..='z' | '0'..='9' | '_')) {
            self.advance();
        }

        let lexeme: String = self.chars[start_index..self.index].iter().collect();
        let kind = keyword(&lexeme).map_or(TokenKind::Identifier, TokenKind::Keyword);
        let end = self.position();

        self.tokens.push(Token {
            kind,
            lexeme,
            range: SourceRange::new(start, end),
        });
    }

    fn lex_number(&mut self) {
        let start = self.position();
        let start_index = self.index;

        while matches!(self.peek(), Some('0'..='9')) {
            self.advance();
        }

        let mut is_float = false;
        if self.peek() == Some('.') && matches!(self.peek_next(), Some('0'..='9')) {
            is_float = true;
            self.advance();
            while matches!(self.peek(), Some('0'..='9')) {
                self.advance();
            }
        }

        while matches!(self.peek(), Some('A'..='Z' | 'a'..='z' | '0'..='9' | '_')) {
            self.advance();
        }

        let lexeme: String = self.chars[start_index..self.index].iter().collect();
        let end = self.position();

        self.tokens.push(Token {
            kind: if is_float {
                TokenKind::Float
            } else {
                TokenKind::Integer
            },
            lexeme,
            range: SourceRange::new(start, end),
        });
    }

    fn lex_string(&mut self) {
        let start = self.position();
        let start_index = self.index;
        self.advance();

        while let Some(ch) = self.peek() {
            match ch {
                '"' => {
                    self.advance();
                    let lexeme: String = self.chars[start_index..self.index].iter().collect();
                    let end = self.position();
                    self.tokens.push(Token {
                        kind: TokenKind::String,
                        lexeme,
                        range: SourceRange::new(start, end),
                    });
                    return;
                }
                '\\' => {
                    self.advance();
                    if self.peek().is_some() {
                        self.advance();
                    }
                }
                '\n' => {
                    self.advance_newline();
                }
                _ => {
                    self.advance();
                }
            }
        }

        let end = self.position();
        self.diagnostics.push(Diagnostic::error(
            "E_LEX_0002",
            self.file,
            SourceRange::new(start, end),
            "unterminated string literal",
            "string literals must end with a closing quote",
        ));
    }

    fn skip_line_comment(&mut self) {
        while let Some(ch) = self.peek() {
            if ch == '\n' {
                break;
            }
            self.advance();
        }
    }

    fn skip_block_comment(&mut self) {
        let start = self.position();
        self.advance();
        self.advance();

        while let Some(ch) = self.peek() {
            if ch == '*' && self.peek_next() == Some('/') {
                self.advance();
                self.advance();
                return;
            }

            if ch == '\n' {
                self.advance_newline();
            } else {
                self.advance();
            }
        }

        let end = self.position();
        self.diagnostics.push(Diagnostic::error(
            "E_LEX_0003",
            self.file,
            SourceRange::new(start, end),
            "unterminated block comment",
            "block comments must end with */",
        ));
    }

    fn invalid_character(&mut self) {
        let start = self.position();
        let ch = self.advance().expect("invalid character exists");
        let end = self.position();

        self.diagnostics.push(Diagnostic::error(
            "E_LEX_0001",
            self.file,
            SourceRange::new(start, end),
            format!("invalid character `{ch}`"),
            "Arelang v0 source files use the ASCII token set",
        ));
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.index + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.index += 1;
        self.column += 1;
        Some(ch)
    }

    fn advance_newline(&mut self) {
        self.index += 1;
        self.line += 1;
        self.column = 1;
    }

    fn position(&self) -> Position {
        Position::new(self.line, self.column)
    }
}

fn keyword(text: &str) -> Option<Keyword> {
    match text {
        "use" => Some(Keyword::Use),
        "as" => Some(Keyword::As),
        "pub" => Some(Keyword::Pub),
        "fn" => Some(Keyword::Fn),
        "let" => Some(Keyword::Let),
        "mut" => Some(Keyword::Mut),
        "struct" => Some(Keyword::Struct),
        "enum" => Some(Keyword::Enum),
        "type" => Some(Keyword::Type),
        "opaque" => Some(Keyword::Opaque),
        "if" => Some(Keyword::If),
        "else" => Some(Keyword::Else),
        "while" => Some(Keyword::While),
        "for" => Some(Keyword::For),
        "in" => Some(Keyword::In),
        "return" => Some(Keyword::Return),
        "break" => Some(Keyword::Break),
        "continue" => Some(Keyword::Continue),
        "match" => Some(Keyword::Match),
        "service" => Some(Keyword::Service),
        "route" => Some(Keyword::Route),
        "test" => Some(Keyword::Test),
        "unsafe" => Some(Keyword::Unsafe),
        "foreign" => Some(Keyword::Foreign),
        "true" => Some(Keyword::True),
        "false" => Some(Keyword::False),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{TokenKind, lex_source};
    use std::path::Path;

    #[test]
    fn lexes_function_header() {
        let (tokens, diagnostics) = lex_source(
            Path::new("test.are"),
            "fn health() -> Http.Response { return Http.Response.ok() }",
        );

        assert!(diagnostics.is_empty());
        assert!(tokens.iter().any(|token| token.lexeme == "fn"));
        assert!(tokens.iter().any(|token| token.kind == TokenKind::Arrow));
    }

    #[test]
    fn reports_invalid_unicode_token() {
        let (_tokens, diagnostics) = lex_source(Path::new("test.are"), "let şehir = 1");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "E_LEX_0001");
    }

    #[test]
    fn reports_unterminated_string() {
        let (_tokens, diagnostics) = lex_source(Path::new("test.are"), "\"oops");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "E_LEX_0002");
    }
}
