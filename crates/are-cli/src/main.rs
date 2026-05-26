use are_diagnostics::Diagnostic;
use are_http_runtime::run_project;
use are_project::check_path;
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
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
    /// Create a new Arelang HTTP server project.
    New {
        /// Directory to create.
        path: PathBuf,

        /// Package name to write into are.toml.
        #[arg(long)]
        name: Option<String>,

        /// Host for the generated HTTP server.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Port for the generated HTTP server.
        #[arg(long, default_value_t = 8080)]
        port: u16,

        /// Starter project shape.
        #[arg(long, value_enum, default_value = "minimal")]
        template: ProjectTemplate,
    },

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum ProjectTemplate {
    /// Minimal GET /ping HTTP service.
    Minimal,
    /// Backend-first users API with validation and typed errors.
    Users,
}

impl ProjectTemplate {
    const fn label(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Users => "users",
        }
    }

    fn source(self, service_name: &str) -> String {
        match self {
            Self::Minimal => minimal_source(service_name),
            Self::Users => users_source(service_name),
        }
    }
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
        Command::New {
            path,
            name,
            host,
            port,
            template,
        } => match create_project(&path, name.as_deref(), &host, port, template) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{err}");
                ExitCode::FAILURE
            }
        },
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

fn create_project(
    path: &Path,
    requested_name: Option<&str>,
    host: &str,
    port: u16,
    template: ProjectTemplate,
) -> Result<(), String> {
    if path.exists() && path.read_dir().map_err(io_error(path))?.next().is_some() {
        return Err(format!(
            "{} already exists and is not empty",
            path.display()
        ));
    }

    let package_name = requested_name.map_or_else(|| package_name_from_path(path), kebab_case);
    let service_name = service_name_from_package(&package_name);
    fs::create_dir_all(path).map_err(io_error(path))?;

    let manifest = project_manifest(&package_name, host, port);
    let source = template.source(&service_name);

    fs::write(path.join("are.toml"), manifest).map_err(io_error(&path.join("are.toml")))?;
    fs::write(path.join("main.are"), source).map_err(io_error(&path.join("main.are")))?;

    println!("created {} ({})", path.display(), template.label());
    println!("next:");
    println!("  ./are check {}", path.display());
    println!("  ./are run {}", path.display());
    match template {
        ProjectTemplate::Minimal => {
            println!("  curl http://{host}:{port}/ping");
        }
        ProjectTemplate::Users => {
            println!("  curl http://{host}:{port}/health");
            println!(
                "  curl -X POST http://{host}:{port}/users -H 'content-type: application/json' -d '{{\"email\":\"ada@example.com\",\"name\":\"Ada Lovelace\"}}'"
            );
            println!("  curl http://{host}:{port}/users/1");
        }
    }
    Ok(())
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
        print_human_diagnostics(&check.diagnostics);
    }

    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn print_human_diagnostics(diagnostics: &[Diagnostic]) {
    let mut sources = HashMap::<PathBuf, Option<String>>::new();

    for diagnostic in diagnostics {
        let source = sources
            .entry(diagnostic.file.clone())
            .or_insert_with(|| fs::read_to_string(&diagnostic.file).ok());
        eprintln!("{}", diagnostic.render(source.as_deref()));
    }
}

fn project_manifest(package_name: &str, host: &str, port: u16) -> String {
    format!(
        r#"[package]
name = "{package_name}"
version = "0.1.0"
target = "server"
entry = "main.are"

[server]
host = "{host}"
port = {port}

[capabilities]
network_listen = ["{host}:{port}"]
network_outbound = []
filesystem_read = []
filesystem_write = []
env_read = []
process_spawn = false
"#
    )
}

fn minimal_source(service_name: &str) -> String {
    format!(
        r#"use std.http as Http

struct AppState {{}}

fn ping(ctx: Http.Context<AppState>, req: Http.Request) -> Http.Response {{
    return Http.Response.ok({{ "message": "pong" }})
}}

service {service_name}(state: AppState) {{
    route GET "/ping" -> ping
}}
"#
    )
}

fn users_source(service_name: &str) -> String {
    format!(
        r#"use std.http as Http
use std.validate

type UserId = opaque U64
type Email = opaque String

struct AppState {{}}

struct CreateUserInput {{
    email: String
    name: String
}}

model User {{
    id: UserId primary
    email: Email unique
    name: String
}}

enum ApiError {{
    InvalidInput(message: String)
    NotFound
    Internal(message: String)
}}

fn health(ctx: Http.Context<AppState>, req: Http.Request) -> Http.Response {{
    return Http.Response.ok({{ "status": "ok" }})
}}

fn validate_user(input: CreateUserInput) -> Result<CreateUserInput, ApiError> {{
    ensure validate.email(input.email), ApiError.InvalidInput("invalid_email")
    ensure validate.length(input.name, min: 2, max: 80), ApiError.InvalidInput("invalid_name")
    return input
}}

fn create_user(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {{
    let input = validate_user(req.json<CreateUserInput>()?)?
    let user = ctx.db.users.insert(input)?
    return Http.Response.created(user)
}}

fn get_user(ctx: Http.Context<AppState>, req: Http.Request) -> Result<Http.Response, ApiError> {{
    let id = ctx.param<UserId>("id")?
    let user = ctx.db.users.get(id)?
    return Http.Response.ok(user)
}}

fn map_error(err: ApiError) -> Http.Response {{
    match err {{
        InvalidInput(message) => return Http.Response.error(400, {{ "error": message }})
        NotFound => return Http.Response.error(404, {{ "error": "not_found" }})
        Internal(message) => return Http.Response.error(500, {{ "error": message }})
    }}
}}

service {service_name}(state: AppState) {{
    use Http.error_map(map_error)

    route GET "/health" -> health
    route POST "/users" -> create_user
    route GET "/users/:id" -> get_user
}}
"#
    )
}

fn package_name_from_path(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "are-app".to_string(), kebab_case)
}

fn kebab_case(input: &str) -> String {
    let mut output = String::new();
    let mut previous_was_dash = false;

    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
            previous_was_dash = false;
        } else if !previous_was_dash && !output.is_empty() {
            output.push('-');
            previous_was_dash = true;
        }
    }

    let trimmed = output.trim_matches('-');
    if trimmed.is_empty() {
        "are-app".to_string()
    } else {
        trimmed.to_string()
    }
}

fn pascal_case(input: &str) -> String {
    let mut output = String::new();
    let mut capitalize = true;

    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            if capitalize {
                output.push(ch.to_ascii_uppercase());
                capitalize = false;
            } else {
                output.push(ch.to_ascii_lowercase());
            }
        } else {
            capitalize = true;
        }
    }

    if output.is_empty() {
        "Are".to_string()
    } else {
        output
    }
}

fn service_name_from_package(package_name: &str) -> String {
    let base = pascal_case(package_name);
    if base.ends_with("Api") {
        base
    } else {
        format!("{base}Api")
    }
}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> String + '_ {
    |err| format!("{}: {err}", path.display())
}

#[cfg(test)]
mod tests {
    use super::{
        kebab_case, minimal_source, package_name_from_path, pascal_case, service_name_from_package,
        users_source,
    };
    use std::path::Path;

    #[test]
    fn derives_project_names() {
        assert_eq!(package_name_from_path(Path::new("hello_api")), "hello-api");
        assert_eq!(kebab_case("My Cool_API"), "my-cool-api");
        assert_eq!(pascal_case("my-cool-api"), "MyCoolApi");
        assert_eq!(service_name_from_package("demo-api"), "DemoApi");
        assert_eq!(service_name_from_package("demo"), "DemoApi");
    }

    #[test]
    fn renders_minimal_http_source() {
        let source = minimal_source("HelloApi");
        assert!(source.contains("fn ping"));
        assert!(source.contains("service HelloApi"));
        assert!(source.contains(r#"route GET "/ping" -> ping"#));
    }

    #[test]
    fn renders_users_http_source() {
        let source = users_source("GeneratedUsersApi");
        assert!(source.contains("model User"));
        assert!(source.contains("fn create_user"));
        assert!(source.contains("service GeneratedUsersApi"));
        assert!(source.contains("use Http.error_map(map_error)"));
        assert!(source.contains(r#"route POST "/users" -> create_user"#));
    }
}
