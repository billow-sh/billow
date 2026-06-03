use std::env;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) const AGENT_BINARY_NAME: &str = "billow-agent";
pub(crate) const SERVICE_NAME: &str = "billow-agent.service";

pub(crate) const DOWNLOAD_DIR_ENV: &str = "BILLOW_DOWNLOAD_DIR";
pub(crate) const BIN_DIR_ENV: &str = "BILLOW_BIN_DIR";
pub(crate) const SYSTEMD_UNIT_DIR_ENV: &str = "BILLOW_SYSTEMD_UNIT_DIR";
pub(crate) const SYSTEMD_RUNTIME_DIR_ENV: &str = "BILLOW_SYSTEMD_RUNTIME_DIR";

const DEFAULT_BIN_DIR: &str = "/usr/local/bin";
const DEFAULT_SYSTEMD_UNIT_DIR: &str = "/etc/systemd/system";
const DEFAULT_SYSTEMD_RUNTIME_DIR: &str = "/run/systemd/system";

pub(crate) fn agent_source_candidates() -> io::Result<Vec<PathBuf>> {
    let mut candidates = Vec::new();

    if let Some(download_dir) = env_dir(DOWNLOAD_DIR_ENV) {
        candidates.push(download_dir.join(AGENT_BINARY_NAME));
        return Ok(candidates);
    }

    let current_exe = env::current_exe().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to resolve billow-init path: {error}"),
        )
    })?;
    if let Some(exe_dir) = current_exe.parent() {
        candidates.push(exe_dir.join(AGENT_BINARY_NAME));
    }

    let current_dir = env::current_dir().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to resolve current directory: {error}"),
        )
    })?;
    candidates.push(current_dir.join(AGENT_BINARY_NAME));

    Ok(deduplicate(candidates))
}

pub(crate) fn agent_install_path() -> PathBuf {
    bin_dir().join(AGENT_BINARY_NAME)
}

pub(crate) fn systemd_unit_dir() -> PathBuf {
    env_dir_or_default(SYSTEMD_UNIT_DIR_ENV, DEFAULT_SYSTEMD_UNIT_DIR)
}

pub(crate) fn systemd_runtime_dir() -> PathBuf {
    env_dir_or_default(SYSTEMD_RUNTIME_DIR_ENV, DEFAULT_SYSTEMD_RUNTIME_DIR)
}

pub(crate) fn service_path() -> PathBuf {
    systemd_unit_dir().join(SERVICE_NAME)
}

fn bin_dir() -> PathBuf {
    env_dir_or_default(BIN_DIR_ENV, DEFAULT_BIN_DIR)
}

fn env_dir_or_default(env_name: &str, default: &str) -> PathBuf {
    env_dir(env_name).unwrap_or_else(|| PathBuf::from(default))
}

fn env_dir(env_name: &str) -> Option<PathBuf> {
    env::var_os(env_name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn deduplicate(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut deduplicated = Vec::new();

    for path in paths {
        if !deduplicated
            .iter()
            .any(|existing: &PathBuf| existing == &path)
        {
            deduplicated.push(path);
        }
    }

    deduplicated
}

pub(crate) fn display(path: &Path) -> String {
    path.display().to_string()
}
