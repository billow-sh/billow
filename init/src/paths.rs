use std::env;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) const AGENT_BINARY_NAME: &str = "billow-agent";
pub(crate) const CONTAINERD_BINARY_NAME: &str = "containerd";
pub(crate) const CONTAINERD_SHIM_BINARY_NAME: &str = "containerd-shim-runc-v2";
pub(crate) const CRUN_BINARY_NAME: &str = "crun";
pub(crate) const INSTALL_BINARY_NAMES: &[&str] = &[
    AGENT_BINARY_NAME,
    CONTAINERD_BINARY_NAME,
    CONTAINERD_SHIM_BINARY_NAME,
    CRUN_BINARY_NAME,
];

pub(crate) const AGENT_SERVICE_NAME: &str = "billow-agent.service";
pub(crate) const CONTAINERD_SERVICE_NAME: &str = "billow-containerd.service";

pub(crate) const DOWNLOAD_DIR_ENV: &str = "BILLOW_DOWNLOAD_DIR";
pub(crate) const BIN_DIR_ENV: &str = "BILLOW_BIN_DIR";
pub(crate) const CONFIG_DIR_ENV: &str = "BILLOW_CONFIG_DIR";
pub(crate) const SYSTEMD_UNIT_DIR_ENV: &str = "BILLOW_SYSTEMD_UNIT_DIR";
pub(crate) const SYSTEMD_RUNTIME_DIR_ENV: &str = "BILLOW_SYSTEMD_RUNTIME_DIR";

pub(crate) const CONTAINERD_ROOT_DIR: &str = "/var/lib/billow/containerd";
pub(crate) const CONTAINERD_STATE_DIR: &str = "/run/billow/containerd";
pub(crate) const CONTAINERD_ADDRESS: &str = "/run/billow/containerd/containerd.sock";

const DEFAULT_BIN_DIR: &str = "/usr/local/lib/billow/bin";
const DEFAULT_CONFIG_DIR: &str = "/etc/billow";
const DEFAULT_SYSTEMD_UNIT_DIR: &str = "/etc/systemd/system";
const DEFAULT_SYSTEMD_RUNTIME_DIR: &str = "/run/systemd/system";

pub(crate) fn binary_source_candidates(binary_name: &str) -> io::Result<Vec<PathBuf>> {
    let mut candidates = Vec::new();

    if let Some(download_dir) = env_dir(DOWNLOAD_DIR_ENV) {
        candidates.push(download_dir.join(binary_name));
        return Ok(candidates);
    }

    let current_exe = env::current_exe().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to resolve billow-init path: {error}"),
        )
    })?;
    if let Some(exe_dir) = current_exe.parent() {
        candidates.push(exe_dir.join(binary_name));
    }

    let current_dir = env::current_dir().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to resolve current directory: {error}"),
        )
    })?;
    candidates.push(current_dir.join(binary_name));

    Ok(deduplicate(candidates))
}

pub(crate) fn agent_install_path() -> PathBuf {
    binary_install_path(AGENT_BINARY_NAME)
}

pub(crate) fn containerd_install_path() -> PathBuf {
    binary_install_path(CONTAINERD_BINARY_NAME)
}

pub(crate) fn containerd_shim_install_path() -> PathBuf {
    binary_install_path(CONTAINERD_SHIM_BINARY_NAME)
}

pub(crate) fn crun_install_path() -> PathBuf {
    binary_install_path(CRUN_BINARY_NAME)
}

pub(crate) fn bin_dir() -> PathBuf {
    env_dir_or_default(BIN_DIR_ENV, DEFAULT_BIN_DIR)
}

pub(crate) fn config_dir() -> PathBuf {
    env_dir_or_default(CONFIG_DIR_ENV, DEFAULT_CONFIG_DIR)
}

pub(crate) fn containerd_config_dir() -> PathBuf {
    config_dir().join(CONTAINERD_BINARY_NAME)
}

pub(crate) fn containerd_config_path() -> PathBuf {
    containerd_config_dir().join("config.toml")
}

pub(crate) fn binary_install_path(binary_name: &str) -> PathBuf {
    bin_dir().join(binary_name)
}

pub(crate) fn systemd_unit_dir() -> PathBuf {
    env_dir_or_default(SYSTEMD_UNIT_DIR_ENV, DEFAULT_SYSTEMD_UNIT_DIR)
}

pub(crate) fn systemd_runtime_dir() -> PathBuf {
    env_dir_or_default(SYSTEMD_RUNTIME_DIR_ENV, DEFAULT_SYSTEMD_RUNTIME_DIR)
}

pub(crate) fn agent_service_path() -> PathBuf {
    systemd_unit_dir().join(AGENT_SERVICE_NAME)
}

pub(crate) fn containerd_service_path() -> PathBuf {
    systemd_unit_dir().join(CONTAINERD_SERVICE_NAME)
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
