mod contracts;
mod errors;
mod functions;
mod host;
mod request;
mod response;
mod scenarios;
mod schemas;
mod server;
mod store;

#[cfg(test)]
mod tests;

use contracts::find_single_service;
use functions::RuntimeFunctions;
use scenarios::{test_ping_scenario, test_users_scenario};
use serde::Serialize;
use server::{print_run_summary, run_http_server};
use std::fs;
use std::path::{Path, PathBuf};
use tiny_http::Server;

pub use contracts::{
    HttpAliasSchema, HttpContractManifest, HttpEnumSchema, HttpEnumVariantSchema, HttpFieldSchema,
    HttpModelFieldSchema, HttpModelSchema, HttpPathParam, HttpRouteContract, HttpSchemaManifest,
    HttpStructSchema, TestPathParam, TestRoute,
};

use are_project::{CheckResult, Manifest, ProjectError, check_path, load_manifest, project_root};

#[derive(Debug)]
pub enum RuntimeError {
    Project(ProjectError),
    StaticChecks(CheckResult),
    UnsupportedProject(String),
    Server(String),
    Test(String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project(err) => write!(f, "{err}"),
            Self::StaticChecks(check) => {
                writeln!(f, "static checks failed before runtime start")?;
                for diagnostic in &check.diagnostics {
                    let source = fs::read_to_string(&diagnostic.file).ok();
                    writeln!(f, "{}", diagnostic.render(source.as_deref()))?;
                }
                Ok(())
            }
            Self::UnsupportedProject(message) | Self::Server(message) | Self::Test(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<ProjectError> for RuntimeError {
    fn from(value: ProjectError) -> Self {
        Self::Project(value)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TestReport {
    pub package: String,
    pub version: String,
    pub service: String,
    pub routes: Vec<TestRoute>,
    pub scenarios: Vec<TestScenario>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestScenario {
    pub name: String,
    pub checks: Vec<String>,
}

/// Run an Arelang HTTP project.
///
/// # Errors
///
/// Returns an error if static checks fail, the manifest cannot be loaded, the
/// project is not supported by the current HTTP MVP runtime, or the TCP server
/// cannot be started.
pub fn run_project(path: &Path) -> Result<(), RuntimeError> {
    let prepared = prepare_project(path)?;
    let server_config = prepared.manifest.server.clone().ok_or_else(|| {
        RuntimeError::UnsupportedProject("server target requires [server] config".into())
    })?;
    let address = format!("{}:{}", server_config.host, server_config.port);
    let server = Server::http(&address).map_err(|err| {
        RuntimeError::Server(format!(
            "failed to start HTTP server at http://{address}\nreason: {err}\nhint: another process may already be using this address; stop it or change [server].port in are.toml"
        ))
    })?;

    print_run_summary(&prepared.manifest, &prepared.contracts, &address);

    run_http_server(
        &server,
        &prepared.contracts,
        &prepared.functions,
        &prepared.root,
        &prepared.manifest,
    );
    Ok(())
}

/// Return the checked HTTP contract manifest for an Arelang server project.
///
/// # Errors
///
/// Returns an error if the project fails the same preparation path as
/// [`run_project`].
pub fn inspect_project(path: &Path) -> Result<HttpContractManifest, RuntimeError> {
    Ok(prepare_project(path)?.contracts)
}

/// Run the MVP project test loop without opening a TCP listener.
///
/// # Errors
///
/// Returns an error if static checks fail, the project cannot be prepared for
/// the HTTP runtime, or a built-in MVP HTTP scenario fails.
pub fn test_project(path: &Path) -> Result<TestReport, RuntimeError> {
    let prepared = prepare_project(path)?;
    let mut scenarios = Vec::new();

    if prepared.contracts.has("GET", "/ping") {
        scenarios.push(test_ping_scenario(&prepared)?);
    }

    if prepared.contracts.has("GET", "/health")
        && prepared.contracts.has("POST", "/users")
        && prepared.contracts.has("GET", "/users/{id: UserId}")
    {
        scenarios.push(test_users_scenario(&prepared)?);
    }

    Ok(TestReport {
        package: prepared.manifest.package.name,
        version: prepared.manifest.package.version,
        service: prepared.contracts.service.clone(),
        routes: prepared.contracts.test_routes(),
        scenarios,
    })
}

#[derive(Debug)]
struct PreparedProject {
    root: PathBuf,
    manifest: Manifest,
    contracts: HttpContractManifest,
    functions: RuntimeFunctions,
}

fn prepare_project(path: &Path) -> Result<PreparedProject, RuntimeError> {
    let root = project_root(path)?;
    let manifest = load_manifest(&root)?;
    if manifest.package.target != "server" {
        return Err(RuntimeError::UnsupportedProject(format!(
            "HTTP runtime requires package target `server`, got `{}`",
            manifest.package.target
        )));
    }

    let entry_path = root.join(&manifest.package.entry);
    if !entry_path.exists() {
        return Err(RuntimeError::UnsupportedProject(format!(
            "package entry `{}` does not exist",
            entry_path.display()
        )));
    }

    let check = check_path(&root)?;

    if !check.ok() {
        return Err(RuntimeError::StaticChecks(check));
    }

    let service = find_single_service(&check.modules)?;
    let contracts = HttpContractManifest::from_service_and_modules(service, &check.modules)?;
    let functions = RuntimeFunctions::from_modules(&check.modules);

    Ok(PreparedProject {
        root,
        manifest,
        contracts,
        functions,
    })
}
