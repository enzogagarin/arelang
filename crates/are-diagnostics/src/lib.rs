use serde::Serialize;
use std::fmt;
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
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

    #[must_use]
    pub fn render(&self, source: Option<&str>) -> String {
        let mut output = String::new();
        writeln!(
            output,
            "{}[{}]: {}",
            self.severity.label(),
            self.code,
            self.problem
        )
        .expect("writing to string cannot fail");
        writeln!(
            output,
            "  --> {}:{}:{}",
            self.file.display(),
            self.range.start.line,
            self.range.start.column
        )
        .expect("writing to string cannot fail");

        if let Some(line) = source.and_then(|source| source_line(source, self.range.start.line)) {
            render_source_line(&mut output, self.range, line);
        }

        writeln!(output, "note: {}", self.reason).expect("writing to string cannot fail");
        for fix in &self.fixes {
            writeln!(output, "help: {}", fix.label).expect("writing to string cannot fail");
        }

        output
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render(None))
    }
}

#[must_use]
pub fn best_name_suggestion<'a>(
    needle: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let needle_key = needle.to_ascii_lowercase();
    candidates
        .into_iter()
        .filter(|candidate| *candidate != needle)
        .filter_map(|candidate| {
            let candidate_key = candidate.to_ascii_lowercase();
            let distance = levenshtein(&needle_key, &candidate_key);
            let max_len = needle_key
                .chars()
                .count()
                .max(candidate_key.chars().count());
            (distance <= suggestion_threshold(max_len)).then_some((candidate, distance, max_len))
        })
        .min_by_key(|(_candidate, distance, max_len)| (*distance, *max_len))
        .map(|(candidate, _distance, _max_len)| candidate)
}

fn render_source_line(output: &mut String, range: SourceRange, line: &str) {
    let number_width = range.start.line.to_string().len();
    let highlight_len =
        if range.end.line == range.start.line && range.end.column > range.start.column {
            range.end.column - range.start.column
        } else {
            1
        };
    let indent = " ".repeat(range.start.column.saturating_sub(1));
    let underline = "^".repeat(highlight_len.max(1));

    writeln!(output, "{:>number_width$} |", "").expect("writing to string cannot fail");
    writeln!(output, "{:>number_width$} | {line}", range.start.line)
        .expect("writing to string cannot fail");
    writeln!(output, "{:>number_width$} | {indent}{underline}", "")
        .expect("writing to string cannot fail");
    writeln!(output, "{:>number_width$} |", "").expect("writing to string cannot fail");
}

fn source_line(source: &str, line: usize) -> Option<&str> {
    source.lines().nth(line.checked_sub(1)?)
}

fn suggestion_threshold(max_len: usize) -> usize {
    match max_len {
        0..=4 => 1,
        5..=8 => 2,
        _ => 3,
    }
}

fn levenshtein(left: &str, right: &str) -> usize {
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut costs = (0..=right_chars.len()).collect::<Vec<_>>();

    for (left_index, left_char) in left.chars().enumerate() {
        let mut previous = costs[0];
        costs[0] = left_index + 1;

        for (right_index, right_char) in right_chars.iter().enumerate() {
            let old = costs[right_index + 1];
            let substitution = previous + usize::from(left_char != *right_char);
            let insertion = costs[right_index] + 1;
            let deletion = costs[right_index + 1] + 1;
            costs[right_index + 1] = substitution.min(insertion).min(deletion);
            previous = old;
        }
    }

    costs[right_chars.len()]
}

#[cfg(test)]
mod tests {
    use super::{Diagnostic, Position, SourceRange, best_name_suggestion};

    #[test]
    fn renders_diagnostics_with_source_snippets_and_help() {
        let range = SourceRange::new(Position::new(2, 18), Position::new(2, 28));
        let diagnostic = Diagnostic::error(
            "E_RESOLVE_0002",
            "main.are",
            range,
            "unknown route handler `create_usr`",
            "declare a function with this name before wiring it in a service route",
        )
        .with_fix(
            "did you mean `create_user`?",
            Some("create_user".to_string()),
        );

        let rendered = diagnostic.render(Some(
            "service UsersApi(state: AppState) {\n    route POST \"/users\" -> create_usr\n}\n",
        ));

        assert!(rendered.contains("error[E_RESOLVE_0002]: unknown route handler `create_usr`"));
        assert!(rendered.contains("2 |     route POST \"/users\" -> create_usr"));
        assert!(rendered.contains("help: did you mean `create_user`?"));
    }

    #[test]
    fn finds_nearby_name_suggestions() {
        let candidates = ["health", "create_user", "get_user"];
        assert_eq!(
            best_name_suggestion("create_usr", candidates),
            Some("create_user")
        );
        assert_eq!(best_name_suggestion("zzzzzz", candidates), None);
    }
}
