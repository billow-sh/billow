use crate::paths;
use crate::system;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;

pub(crate) fn ensure_available() -> io::Result<()> {
    let systemd_unit_dir = paths::systemd_unit_dir();

    if !systemd_unit_dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "systemd unit directory {} does not exist",
                paths::display(&systemd_unit_dir)
            ),
        ));
    }

    if !system::command_succeeds("systemctl", &["--version"]) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "systemctl is not available",
        ));
    }

    let systemd_runtime_dir = paths::systemd_runtime_dir();

    if !systemd_runtime_dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "systemd runtime directory {} does not exist",
                paths::display(&systemd_runtime_dir)
            ),
        ));
    }

    Ok(())
}

pub(crate) fn ensure_service_not_installed() -> io::Result<()> {
    let service_path = paths::service_path();

    if service_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", paths::display(&service_path)),
        ));
    }

    Ok(())
}

pub(crate) fn install_unit() -> io::Result<()> {
    let service_path = paths::service_path();

    let mut unit = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&service_path)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "failed to create {}: {error}",
                    paths::display(&service_path)
                ),
            )
        })?;

    unit.write_all(service_unit().as_bytes()).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to write {}: {error}", paths::display(&service_path)),
        )
    })?;

    fs::set_permissions(&service_path, fs::Permissions::from_mode(0o644)).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to set permissions on {}: {error}",
                paths::display(&service_path)
            ),
        )
    })?;

    Ok(())
}

pub(crate) fn reload() -> io::Result<()> {
    system::run_command("systemctl", &["daemon-reload"])
}

pub(crate) fn enable_and_start_service() -> io::Result<()> {
    system::run_command("systemctl", &["enable", "--now", paths::SERVICE_NAME])
}

fn service_unit() -> String {
    let agent_install_path = paths::agent_install_path();

    format!(
        "\
[Unit]
Description=Billow Agent
After=network.target

[Service]
Type=simple
ExecStart={}
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=multi-user.target
",
        paths::display(&agent_install_path)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_unit_runs_installed_agent() {
        let unit = service_unit();

        assert!(unit.contains("Description=Billow Agent"));
        assert!(unit.contains("ExecStart="));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=multi-user.target"));
    }
}
