use crate::paths;
use crate::system;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

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

pub(crate) fn ensure_units_not_installed() -> io::Result<()> {
    for service_path in [
        paths::containerd_service_path(),
        paths::agent_service_path(),
    ] {
        if service_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists", paths::display(&service_path)),
            ));
        }
    }

    Ok(())
}

pub(crate) fn install_units() -> io::Result<()> {
    write_unit(
        &paths::containerd_service_path(),
        &containerd_service_unit(),
    )?;
    write_unit(&paths::agent_service_path(), &agent_service_unit())
}

fn write_unit(service_path: &Path, unit_contents: &str) -> io::Result<()> {
    let mut unit = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(service_path)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to create {}: {error}", paths::display(service_path)),
            )
        })?;

    unit.write_all(unit_contents.as_bytes()).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to write {}: {error}", paths::display(service_path)),
        )
    })?;

    fs::set_permissions(service_path, fs::Permissions::from_mode(0o644)).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to set permissions on {}: {error}",
                paths::display(service_path)
            ),
        )
    })?;

    Ok(())
}

pub(crate) fn reload() -> io::Result<()> {
    system::run_command("systemctl", &["daemon-reload"])
}

pub(crate) fn enable_and_start_services() -> io::Result<()> {
    system::run_command(
        "systemctl",
        &[
            "enable",
            "--now",
            paths::CONTAINERD_SERVICE_NAME,
            paths::AGENT_SERVICE_NAME,
        ],
    )
}

fn agent_service_unit() -> String {
    let agent_install_path = paths::agent_install_path();
    let containerd_shim_path = paths::containerd_shim_install_path();
    let crun_path = paths::crun_install_path();

    format!(
        "\
[Unit]
Description=Billow Agent
Requires={}
After=network.target {}

[Service]
Type=simple
Environment={}={}
Environment={}={}
Environment={}={}
Environment={}={}
ExecStart={}
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=multi-user.target
",
        paths::CONTAINERD_SERVICE_NAME,
        paths::CONTAINERD_SERVICE_NAME,
        paths::AGENT_CONTAINERD_SOCKET_ENV,
        paths::CONTAINERD_ADDRESS,
        paths::AGENT_CONTAINERD_SHIM_ENV,
        paths::display(&containerd_shim_path),
        paths::AGENT_CRUN_ENV,
        paths::display(&crun_path),
        paths::AGENT_TASK_DIR_ENV,
        paths::TASK_DIR,
        paths::display(&agent_install_path)
    )
}

fn containerd_service_unit() -> String {
    let containerd_install_path = paths::containerd_install_path();
    let containerd_config_path = paths::containerd_config_path();

    format!(
        "\
[Unit]
Description=Billow Containerd
After=network.target

[Service]
Type=notify
ExecStartPre=/usr/bin/install -d -m 0755 {} {}
ExecStart={} --config {} --root {} --state {} --address {}
Delegate=yes
KillMode=process
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=multi-user.target
",
        paths::CONTAINERD_ROOT_DIR,
        paths::CONTAINERD_STATE_DIR,
        paths::display(&containerd_install_path),
        paths::display(&containerd_config_path),
        paths::CONTAINERD_ROOT_DIR,
        paths::CONTAINERD_STATE_DIR,
        paths::CONTAINERD_ADDRESS
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_service_unit_runs_installed_agent() {
        let unit = agent_service_unit();

        assert!(unit.contains("Description=Billow Agent"));
        assert!(unit.contains("Requires=billow-containerd.service"));
        assert!(unit.contains("After=network.target billow-containerd.service"));
        assert!(unit.contains(
            "Environment=BILLOW_CONTAINERD_SOCKET=/run/billow/containerd/containerd.sock"
        ));
        assert!(unit.contains("Environment=BILLOW_CONTAINERD_SHIM="));
        assert!(unit.contains("Environment=BILLOW_CRUN="));
        assert!(unit.contains("Environment=BILLOW_TASK_DIR=/run/billow/tasks"));
        assert!(unit.contains("ExecStart="));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=multi-user.target"));
    }

    #[test]
    fn containerd_service_unit_runs_installed_containerd() {
        let unit = containerd_service_unit();

        assert!(unit.contains("Description=Billow Containerd"));
        assert!(unit.contains("Type=notify"));
        assert!(unit.contains("ExecStart="));
        assert!(unit.contains("Delegate=yes"));
        assert!(unit.contains("KillMode=process"));
        assert!(unit.contains("--config "));
        assert!(unit.contains("--root /var/lib/billow/containerd"));
        assert!(unit.contains("--state /run/billow/containerd"));
        assert!(unit.contains("--address /run/billow/containerd/containerd.sock"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=multi-user.target"));
    }
}
