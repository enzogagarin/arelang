use are_diagnostics::Diagnostic;
use are_http_runtime::run_project;
use are_project::check_path;
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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

    /// Run an Arelang HTTP server project.
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
            if let Err(err) = run_project(&path) {
                eprintln!("{err}");
                return ExitCode::FAILURE;
            }

            ExitCode::SUCCESS
        }
    }
}

fn run_check(path: &Path, json: bool) -> ExitCode {
    let check = match check_path(path) {
        Ok(check) => check,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };

    let ok = check.ok();

    if json {
        let report = CheckReport {
            ok,
            files_checked: check.files_checked,
            diagnostics: check.diagnostics,
        };

        match serde_json::to_string_pretty(&report) {
            Ok(encoded) => println!("{encoded}"),
            Err(err) => {
                eprintln!("failed to encode diagnostic JSON: {err}");
                return ExitCode::FAILURE;
            }
        }
    } else if ok {
        println!("checked {} Arelang file(s)", check.files_checked);
    } else {
        for diagnostic in &check.diagnostics {
            eprintln!("{diagnostic}");
        }
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
