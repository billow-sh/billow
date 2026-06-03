use std::env;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(crate) const AGENT_BINARY_NAME: &str = "billow-agent";
pub(crate) const AGENT_INSTALL_PATH: &str = "/usr/local/bin/billow-agent";

pub(crate) fn ensure_agent_not_installed() -> io::Result<()> {
    if Path::new(AGENT_INSTALL_PATH).exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{AGENT_INSTALL_PATH} already exists"),
        ));
    }

    Ok(())
}

pub(crate) fn find_agent_source() -> io::Result<PathBuf> {
    let current_exe = env::current_exe().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to resolve billow-init path: {error}"),
        )
    })?;
    let current_dir = env::current_dir().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to resolve current directory: {error}"),
        )
    })?;

    for candidate in agent_source_candidates(&current_exe, &current_dir) {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "{AGENT_BINARY_NAME} must be present next to billow-init or in the current directory"
        ),
    ))
}

pub(crate) fn install_agent_binary(agent_source: &Path) -> io::Result<()> {
    move_file(agent_source, Path::new(AGENT_INSTALL_PATH)).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to move {} to {AGENT_INSTALL_PATH}: {error}",
                agent_source.display()
            ),
        )
    })?;

    fs::set_permissions(AGENT_INSTALL_PATH, fs::Permissions::from_mode(0o755)).map_err(
        |error| {
            io::Error::new(
                error.kind(),
                format!("failed to set permissions on {AGENT_INSTALL_PATH}: {error}"),
            )
        },
    )?;

    Ok(())
}

fn agent_source_candidates(current_exe: &Path, current_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(exe_dir) = current_exe.parent() {
        candidates.push(exe_dir.join(AGENT_BINARY_NAME));
    }

    let current_dir_candidate = current_dir.join(AGENT_BINARY_NAME);
    if !candidates
        .iter()
        .any(|candidate| candidate == &current_dir_candidate)
    {
        candidates.push(current_dir_candidate);
    }

    candidates
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_source_candidates_prefer_binary_next_to_init() {
        let candidates = agent_source_candidates(
            Path::new("/tmp/billow-download/billow-init"),
            Path::new("/different/current-dir"),
        );

        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/tmp/billow-download/billow-agent"),
                PathBuf::from("/different/current-dir/billow-agent"),
            ]
        );
    }

    #[test]
    fn agent_source_candidates_deduplicate_current_dir() {
        let candidates = agent_source_candidates(
            Path::new("/tmp/billow-download/billow-init"),
            Path::new("/tmp/billow-download"),
        );

        assert_eq!(
            candidates,
            vec![PathBuf::from("/tmp/billow-download/billow-agent")]
        );
    }
}
