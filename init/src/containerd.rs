use crate::paths;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;

pub(crate) fn ensure_config_not_installed() -> io::Result<()> {
    let config_path = paths::containerd_config_path();

    if config_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", paths::display(&config_path)),
        ));
    }

    Ok(())
}

pub(crate) fn install_config() -> io::Result<()> {
    let config_dir = paths::containerd_config_dir();

    fs::create_dir_all(&config_dir).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to create containerd config directory {}: {error}",
                paths::display(&config_dir)
            ),
        )
    })?;

    let config_path = paths::containerd_config_path();
    let mut config = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&config_path)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to create {}: {error}", paths::display(&config_path)),
            )
        })?;

    config
        .write_all(config_contents().as_bytes())
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to write {}: {error}", paths::display(&config_path)),
            )
        })?;

    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o644)).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to set permissions on {}: {error}",
                paths::display(&config_path)
            ),
        )
    })
}

fn config_contents() -> String {
    let shim_path = paths::containerd_shim_install_path();
    let crun_path = paths::crun_install_path();

    format!(
        "\
version = 3

[plugins.'io.containerd.cri.v1.runtime'.containerd]
default_runtime_name = 'runc'

[plugins.'io.containerd.cri.v1.runtime'.containerd.runtimes.runc]
runtime_type = 'io.containerd.runc.v2'
runtime_path = '{}'

[plugins.'io.containerd.cri.v1.runtime'.containerd.runtimes.runc.options]
BinaryName = '{}'
",
        paths::display(&shim_path),
        paths::display(&crun_path)
    )
}
