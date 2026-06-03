use crate::paths;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(crate) fn ensure_agent_not_installed() -> io::Result<()> {
    let agent_install_path = paths::agent_install_path();

    if agent_install_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", paths::display(&agent_install_path)),
        ));
    }

    Ok(())
}

pub(crate) fn find_agent_source() -> io::Result<PathBuf> {
    for candidate in paths::agent_source_candidates()? {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "{} must be present next to billow-init or in the current directory",
            paths::AGENT_BINARY_NAME
        ),
    ))
}

pub(crate) fn install_agent_binary(agent_source: &Path) -> io::Result<()> {
    let agent_install_path = paths::agent_install_path();

    move_file(agent_source, &agent_install_path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to move {} to {}: {error}",
                agent_source.display(),
                paths::display(&agent_install_path)
            ),
        )
    })?;

    fs::set_permissions(&agent_install_path, fs::Permissions::from_mode(0o755)).map_err(
        |error| {
            io::Error::new(
                error.kind(),
                format!(
                    "failed to set permissions on {}: {error}",
                    paths::display(&agent_install_path)
                ),
            )
        },
    )?;

    Ok(())
}

fn move_file(source: &Path, destination: &Path) -> io::Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            fs::copy(source, destination)?;
            fs::remove_file(source)
        }
        Err(error) => Err(error),
    }
}
