use are_http_runtime::{HttpContractManifest, RuntimeError, inspect_project};
use are_project::{
    CapabilityManifest, Manifest, ProjectError, check_path, load_manifest, project_root,
};
use serde::Serialize;
use std::path::Path;

#[derive(Debug)]
pub enum AuditError {
    Project(ProjectError),
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for AuditError {}

impl From<ProjectError> for AuditError {
    fn from(value: ProjectError) -> Self {
        Self::Project(value)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditReport {
    pub package: String,
    pub version: String,
    pub target: String,
    pub ok: bool,
    pub checks: Vec<AuditCheck>,
    pub capabilities: Option<CapabilityManifest>,
    pub http: Option<AuditHttpSurface>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditHttpSurface {
    pub service: String,
    pub routes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditCheck {
    pub id: &'static str,
    pub status: AuditStatus,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditStatus {
    Pass,
    Warn,
    Fail,
}

/// Audit an Arelang project for MVP production-shape safety checks.
///
/// # Errors
///
/// Returns an error when the project root or package manifest cannot be read.
pub fn audit_project(path: &Path) -> Result<AuditReport, AuditError> {
    let root = project_root(path)?;
    let manifest = load_manifest(&root)?;
    let mut checks = Vec::new();

    match check_path(&root) {
        Ok(check) if check.ok() => checks.push(pass(
            "static_checks",
            format!("checked {} Arelang file(s)", check.files_checked),
        )),
        Ok(check) => checks.push(fail(
            "static_checks",
            format!(
                "static checks reported {} diagnostic(s); run `are check` for details",
                check.diagnostics.len()
            ),
        )),
        Err(err) => checks.push(fail("static_checks", err.to_string())),
    }

    let http = audit_http_contracts(path, &manifest, &mut checks);
    audit_capabilities(&manifest, &mut checks);

    Ok(report(manifest, checks, http))
}

fn audit_http_contracts(
    path: &Path,
    manifest: &Manifest,
    checks: &mut Vec<AuditCheck>,
) -> Option<AuditHttpSurface> {
    if manifest.package.target != "server" {
        checks.push(warn(
            "http_contract_manifest",
            format!(
                "target `{}` is outside the HTTP MVP audit path",
                manifest.package.target
            ),
        ));
        return None;
    }

    match inspect_project(path) {
        Ok(contracts) => {
            checks.push(pass(
                "http_contract_manifest",
                format!(
                    "service `{}` exposes {} route(s)",
                    contracts.service,
                    contracts.routes.len()
                ),
            ));
            audit_route_contracts(&contracts, checks);
            Some(AuditHttpSurface {
                service: contracts.service,
                routes: contracts.routes.len(),
            })
        }
        Err(err) => {
            checks.push(fail("http_contract_manifest", runtime_error_message(&err)));
            None
        }
    }
}

fn audit_route_contracts(contracts: &HttpContractManifest, checks: &mut Vec<AuditCheck>) {
    let missing = contracts
        .routes
        .iter()
        .filter(|route| route.response_type.is_none() || route.status.is_none())
        .map(|route| format!("{} {}", route.method, route.path))
        .collect::<Vec<_>>();

    if missing.is_empty() {
        checks.push(pass(
            "route_success_contracts",
            "all routes declare response type and success status",
        ));
    } else {
        checks.push(fail(
            "route_success_contracts",
            format!("missing success contracts for {}", missing.join(", ")),
        ));
    }
}

fn audit_capabilities(manifest: &Manifest, checks: &mut Vec<AuditCheck>) {
    let Some(capabilities) = &manifest.capabilities else {
        checks.push(fail(
            "capabilities_manifest",
            "are.toml must declare an explicit [capabilities] table",
        ));
        return;
    };

    checks.push(pass(
        "capabilities_manifest",
        "are.toml declares an explicit [capabilities] table",
    ));

    audit_network_listen(manifest, capabilities, checks);
    audit_empty_list(
        "network_outbound",
        &capabilities.network_outbound,
        "no outbound network capability is declared",
        "outbound network capability is broader than the current MVP runtime uses",
        checks,
    );
    audit_empty_list(
        "filesystem_read",
        &capabilities.filesystem_read,
        "no filesystem read capability is declared",
        "filesystem read capability is broader than the current MVP runtime uses",
        checks,
    );
    audit_empty_list(
        "filesystem_write",
        &capabilities.filesystem_write,
        "no filesystem write capability is declared",
        "filesystem write capability is broader than the current MVP runtime uses",
        checks,
    );
    audit_empty_list(
        "env_read",
        &capabilities.env_read,
        "no environment read capability is declared",
        "environment read capability is broader than the current MVP runtime uses",
        checks,
    );

    if capabilities.process_spawn {
        checks.push(fail(
            "process_spawn",
            "process spawning is not part of the current backend MVP capability set",
        ));
    } else {
        checks.push(pass("process_spawn", "process spawning is disabled"));
    }
}

fn audit_network_listen(
    manifest: &Manifest,
    capabilities: &CapabilityManifest,
    checks: &mut Vec<AuditCheck>,
) {
    let Some(server) = &manifest.server else {
        checks.push(fail(
            "network_listen",
            "server target requires [server] config before network capability can be checked",
        ));
        return;
    };

    let expected = format!("{}:{}", server.host, server.port);
    if !capabilities
        .network_listen
        .iter()
        .any(|listen| listen == &expected)
    {
        checks.push(fail(
            "network_listen",
            format!("missing required listen capability `{expected}`"),
        ));
        return;
    }

    if capabilities.network_listen.len() == 1 {
        checks.push(pass(
            "network_listen",
            format!("declares required listen capability `{expected}`"),
        ));
    } else {
        checks.push(warn(
            "network_listen",
            format!(
                "declares `{expected}` plus {} extra listen capability entry(ies)",
                capabilities.network_listen.len() - 1
            ),
        ));
    }
}

fn audit_empty_list(
    id: &'static str,
    values: &[String],
    pass_message: &'static str,
    warn_message: &'static str,
    checks: &mut Vec<AuditCheck>,
) {
    if values.is_empty() {
        checks.push(pass(id, pass_message));
    } else {
        checks.push(warn(id, format!("{}: {}", warn_message, values.join(", "))));
    }
}

fn report(
    manifest: Manifest,
    checks: Vec<AuditCheck>,
    http: Option<AuditHttpSurface>,
) -> AuditReport {
    let ok = checks.iter().all(|check| check.status != AuditStatus::Fail);
    AuditReport {
        package: manifest.package.name,
        version: manifest.package.version,
        target: manifest.package.target,
        ok,
        checks,
        capabilities: manifest.capabilities,
        http,
    }
}

fn pass(id: &'static str, message: impl Into<String>) -> AuditCheck {
    check(id, AuditStatus::Pass, message)
}

fn warn(id: &'static str, message: impl Into<String>) -> AuditCheck {
    check(id, AuditStatus::Warn, message)
}

fn fail(id: &'static str, message: impl Into<String>) -> AuditCheck {
    check(id, AuditStatus::Fail, message)
}

fn check(id: &'static str, status: AuditStatus, message: impl Into<String>) -> AuditCheck {
    AuditCheck {
        id,
        status,
        message: message.into(),
    }
}

fn runtime_error_message(err: &RuntimeError) -> String {
    match err {
        RuntimeError::StaticChecks(_) => {
            "static checks failed before HTTP contract manifest generation".to_string()
        }
        RuntimeError::Project(_)
        | RuntimeError::UnsupportedProject(_)
        | RuntimeError::Server(_)
        | RuntimeError::Test(_) => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{AuditStatus, audit_project};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn audits_users_api_capabilities() {
        let report = audit_project(Path::new("../../examples/users_api")).expect("audit");

        assert!(report.ok, "{:#?}", report.checks);
        assert_eq!(report.package, "users-api");
        assert_eq!(report.http.expect("http surface").routes, 3);
        assert!(
            report
                .checks
                .iter()
                .any(|check| { check.id == "network_listen" && check.status == AuditStatus::Pass })
        );
        assert!(report.checks.iter().any(|check| {
            check.id == "route_success_contracts" && check.status == AuditStatus::Pass
        }));
    }

    #[test]
    fn rejects_missing_listen_capability() {
        let root = temp_project_dir("missing-listen-capability");
        fs::create_dir_all(&root).expect("temp project dir");
        fs::write(
            root.join("are.toml"),
            r#"[package]
name = "missing-listen-capability"
version = "0.1.0"
target = "server"
entry = "main.are"

[server]
host = "127.0.0.1"
port = 19001

[capabilities]
network_listen = []
network_outbound = []
filesystem_read = []
filesystem_write = []
env_read = []
process_spawn = false
"#,
        )
        .expect("write manifest");
        fs::write(
            root.join("main.are"),
            include_str!("../../../examples/hello_api/main.are"),
        )
        .expect("write source");

        let report = audit_project(&root).expect("audit");
        fs::remove_dir_all(&root).expect("cleanup temp project");

        assert!(!report.ok);
        assert!(
            report
                .checks
                .iter()
                .any(|check| { check.id == "network_listen" && check.status == AuditStatus::Fail })
        );
    }

    fn temp_project_dir(label: &str) -> PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_millis();
        std::env::temp_dir().join(format!("are-audit-{label}-{millis}"))
    }
}
