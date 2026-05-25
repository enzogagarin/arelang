use are_diagnostics::{Diagnostic, Severity};
use are_lexer::lex_source;
use are_parser::parse_tokens;
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use walkdir::{DirEntry, WalkDir};

#[derive(Debug, Parser)]
#[command(name = "are")]
#[command(about = "Arelang compiler and backend toolchain")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run static checks without starting a server or producing code.
    Check {
        /// Project directory or .are file to check.
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Emit machine-readable diagnostics.
        #[arg(long)]
        json: bool,
    },

    /// Format source files. This command is a placeholder until arefmt lands.
    Fmt {
        /// Project directory or .are file to format.
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Run an Arelang project. HTTP runtime support is the next milestone.
    Run {
        /// Project directory to run.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

#[derive(Debug, Serialize)]
struct CheckReport {
    ok: bool,
    files_checked: usize,
    diagnostics: Vec<Diagnostic>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::Check { path, json } => run_check(&path, json),
        Command::Fmt { path } => {
            println!("are fmt is planned; {} was left unchanged", path.display());
            ExitCode::SUCCESS
        }
        Command::Run { path } => {
            let status = run_check(&path, false);
            if status != ExitCode::SUCCESS {
                return status;
            }

            println!(
                "HTTP runtime is planned next; {} passed lexical checks",
                path.display()
            );
            ExitCode::SUCCESS
        }
    }
}

fn run_check(path: &Path, json: bool) -> ExitCode {
    let files = match collect_are_files(path) {
        Ok(files) => files,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };

    let mut diagnostics = Vec::new();

    for file in &files {
        match fs::read_to_string(file) {
            Ok(source) => {
                let (tokens, mut file_diagnostics) = lex_source(file, &source);
                if !file_diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.severity == Severity::Error)
                {
                    let (_module, mut parse_diagnostics) = parse_tokens(file, &tokens);
                    file_diagnostics.append(&mut parse_diagnostics);
                }
                diagnostics.append(&mut file_diagnostics);
            }
            Err(err) => diagnostics.push(read_error(file, &err.to_string())),
        }
    }

    let ok = !diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error);

    if json {
        let report = CheckReport {
            ok,
            files_checked: files.len(),
            diagnostics,
        };

        match serde_json::to_string_pretty(&report) {
            Ok(encoded) => println!("{encoded}"),
            Err(err) => {
                eprintln!("failed to encode diagnostic JSON: {err}");
                return ExitCode::FAILURE;
            }
        }
    } else if ok {
        println!("checked {} Arelang file(s)", files.len());
    } else {
        for diagnostic in &diagnostics {
            eprintln!("{diagnostic}");
        }
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn collect_are_files(path: &Path) -> Result<Vec<PathBuf>, String> {
    if path.is_file() {
        return if path.extension().is_some_and(|extension| extension == "are") {
            Ok(vec![path.to_path_buf()])
        } else {
            Err(format!("{} is not an .are file", path.display()))
        };
    }

    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
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
    use are_diagnostics::{Position, SourceRange};

    Diagnostic::error(
        "E_IO_0001",
        file,
        SourceRange::new(Position::new(1, 1), Position::new(1, 1)),
        "failed to read source file",
        reason,
    )
}
