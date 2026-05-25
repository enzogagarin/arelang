use are_ast::Module;
use are_diagnostics::{Diagnostic, Position, Severity, SourceRange};
use are_lexer::lex_source;
use are_parser::parse_tokens;
use are_resolver::resolve_module;
use are_typecheck::typecheck_module;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

#[derive(Debug, Clone)]
pub struct CheckedFile {
    pub path: PathBuf,
    pub module: Module,
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub files_checked: usize,
    pub diagnostics: Vec<Diagnostic>,
    pub modules: Vec<CheckedFile>,
}

impl CheckResult {
    #[must_use]
    pub fn ok(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub package: PackageManifest,
    pub server: Option<ServerManifest>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    pub target: String,
    pub entry: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerManifest {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub enum ProjectError {
    InvalidPath(String),
    Manifest(String),
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPath(message) | Self::Manifest(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ProjectError {}

/// Run the static Arelang pipeline for a file or project directory.
///
/// # Errors
///
/// Returns an error if the path does not exist or points at a non-`.are` file.
pub fn check_path(path: &Path) -> Result<CheckResult, ProjectError> {
    let files = collect_are_files(path)?;
    let mut diagnostics = Vec::new();
    let mut modules = Vec::new();

    for file in &files {
        match fs::read_to_string(file) {
            Ok(source) => check_source_file(file, &source, &mut diagnostics, &mut modules),
            Err(err) => diagnostics.push(read_error(file, &err.to_string())),
        }
    }

    Ok(CheckResult {
        files_checked: files.len(),
        diagnostics,
        modules,
    })
}

/// Load `are.toml` from a project directory.
///
/// # Errors
///
/// Returns an error when the manifest cannot be read or parsed.
pub fn load_manifest(project_dir: &Path) -> Result<Manifest, ProjectError> {
    let manifest_path = project_dir.join("are.toml");
    let contents = fs::read_to_string(&manifest_path).map_err(|err| {
        ProjectError::Manifest(format!("failed to read {}: {err}", manifest_path.display()))
    })?;

    toml::from_str(&contents).map_err(|err| {
        ProjectError::Manifest(format!(
            "failed to parse {}: {err}",
            manifest_path.display()
        ))
    })
}

/// Resolve a project root from either a project directory or source file path.
///
/// # Errors
///
/// Returns an error when the path does not exist or a file has no parent path.
pub fn project_root(path: &Path) -> Result<PathBuf, ProjectError> {
    if path.is_file() {
        return path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| ProjectError::InvalidPath(format!("{} has no parent", path.display())));
    }

    if path.exists() {
        return Ok(path.to_path_buf());
    }

    Err(ProjectError::InvalidPath(format!(
        "{} does not exist",
        path.display()
    )))
}

fn check_source_file(
    file: &Path,
    source: &str,
    diagnostics: &mut Vec<Diagnostic>,
    modules: &mut Vec<CheckedFile>,
) {
    let (tokens, mut file_diagnostics) = lex_source(file, source);
    if file_diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        diagnostics.append(&mut file_diagnostics);
        return;
    }

    let (module, mut parse_diagnostics) = parse_tokens(file, &tokens);
    if !parse_diagnostics.is_empty() {
        file_diagnostics.append(&mut parse_diagnostics);
        diagnostics.append(&mut file_diagnostics);
        return;
    }

    let Some(module) = module else {
        diagnostics.append(&mut file_diagnostics);
        return;
    };

    let mut resolve_diagnostics = resolve_module(file, &module);
    let has_resolve_error = resolve_diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error);
    file_diagnostics.append(&mut resolve_diagnostics);

    if !has_resolve_error {
        let mut type_diagnostics = typecheck_module(file, &module);
        file_diagnostics.append(&mut type_diagnostics);
    }

    let has_file_error = file_diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error);
    diagnostics.append(&mut file_diagnostics);

    if !has_file_error {
        modules.push(CheckedFile {
            path: file.to_path_buf(),
            module,
        });
    }
}

fn collect_are_files(path: &Path) -> Result<Vec<PathBuf>, ProjectError> {
    if path.is_file() {
        return if path.extension().is_some_and(|extension| extension == "are") {
            Ok(vec![path.to_path_buf()])
        } else {
            Err(ProjectError::InvalidPath(format!(
                "{} is not an .are file",
                path.display()
            )))
        };
    }

    if !path.exists() {
        return Err(ProjectError::InvalidPath(format!(
            "{} does not exist",
            path.display()
        )));
    }

    let mut files = Vec::new();

    for entry in WalkDir::new(path)
        .into_iter()
        .filter_entry(should_descend)
        .filter_map(Result::ok)
    {
        let entry_path = entry.path();
        if entry_path.is_file() && entry_path.extension().is_some_and(|ext| ext == "are") {
            files.push(entry_path.to_path_buf());
        }
    }

    files.sort();
    Ok(files)
}

fn should_descend(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    !matches!(name.as_ref(), ".git" | "target")
}

fn read_error(file: &Path, reason: &str) -> Diagnostic {
    Diagnostic::error(
        "E_IO_0001",
        file,
        SourceRange::new(Position::new(1, 1), Position::new(1, 1)),
        "failed to read source file",
        reason,
    )
}

#[cfg(test)]
mod tests {
    use super::{check_path, load_manifest};
    use std::path::Path;

    #[test]
    fn checks_users_api_project() {
        let result = check_path(Path::new("../../examples/users_api")).expect("project checks");
        assert!(result.ok(), "{:#?}", result.diagnostics);
        assert_eq!(result.files_checked, 1);
        assert_eq!(result.modules.len(), 1);
    }

    #[test]
    fn loads_users_api_manifest() {
        let manifest = load_manifest(Path::new("../../examples/users_api")).expect("manifest");
        assert_eq!(manifest.package.name, "users-api");
        assert_eq!(manifest.package.target, "server");
        assert_eq!(manifest.server.expect("server config").port, 8080);
    }
}
