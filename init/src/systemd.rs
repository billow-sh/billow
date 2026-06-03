use crate::install::AGENT_INSTALL_PATH;
use crate::system;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(crate) const SERVICE_NAME: &str = "billow-agent.service";

const SERVICE_PATH: &str = "/etc/systemd/system/billow-agent.service";
const SYSTEMD_UNIT_DIR: &str = "/etc/systemd/system";
const SYSTEMD_RUNTIME_DIR: &str = "/run/systemd/system";

pub(crate) fn ensure_available() -> io::Result<()> {
    if !Path::new(SYSTEMD_UNIT_DIR).is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("systemd unit directory {SYSTEMD_UNIT_DIR} does not exist"),
        ));
    }

    if !system::command_succeeds("systemctl", &["--version"]) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "systemctl is not available",
        ));
    }

    if !Path::new(SYSTEMD_RUNTIME_DIR).is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("systemd runtime directory {SYSTEMD_RUNTIME_DIR} does not exist"),
        ));
    }

    Ok(())
}

pub(crate) fn ensure_service_not_installed() -> io::Result<()> {
    if Path::new(SERVICE_PATH).exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{SERVICE_PATH} already exists"),
        ));
    }

    Ok(())
}

pub(crate) fn install_unit() -> io::Result<()> {
    let mut unit = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(SERVICE_PATH)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to create {SERVICE_PATH}: {error}"),
            )
        })?;

    unit.write_all(service_unit().as_bytes()).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to write {SERVICE_PATH}: {error}"),
        )
    })?;

    fs::set_permissions(SERVICE_PATH, fs::Permissions::from_mode(0o644)).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to set permissions on {SERVICE_PATH}: {error}"),
        )
    })?;

    Ok(())
}

pub(crate) fn reload() -> io::Result<()> {
    system::run_command("systemctl", &["daemon-reload"])
}

pub(crate) fn enable_and_start_service() -> io::Result<()> {
    system::run_command("systemctl", &["enable", "--now", SERVICE_NAME])
}

fn service_unit() -> String {
    format!(
        "\
[Unit]
Description=Billow Agent
After=network.target

[Service]
Type=simple
ExecStart={AGENT_INSTALL_PATH}
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=multi-user.target
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_unit_runs_installed_agent() {
        let unit = service_unit();

        assert!(unit.contains("Description=Billow Agent"));
        assert!(unit.contains(&format!("ExecStart={AGENT_INSTALL_PATH}")));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=multi-user.target"));
    }
}
