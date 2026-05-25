use serde::Serialize;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    #[must_use]
    pub const fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SourceRange {
    pub start: Position,
    pub end: Position,
}

impl SourceRange {
    #[must_use]
    pub const fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FixSuggestion {
    pub label: String,
    pub replacement: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub file: PathBuf,
    pub range: SourceRange,
    pub problem: String,
    pub reason: String,
    pub fixes: Vec<FixSuggestion>,
}

impl Diagnostic {
    #[must_use]
    pub fn error(
        code: impl Into<String>,
        file: impl Into<PathBuf>,
        range: SourceRange,
        problem: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            severity: Severity::Error,
            file: file.into(),
            range,
            problem: problem.into(),
            reason: reason.into(),
            fixes: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_fix(mut self, label: impl Into<String>, replacement: Option<String>) -> Self {
        self.fixes.push(FixSuggestion {
            label: label.into(),
            replacement,
        });
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {:?} {}: {} ({})",
            self.file.display(),
            self.range.start.line,
            self.range.start.column,
            self.severity,
            self.code,
            self.problem,
            self.reason
        )
    }
}
