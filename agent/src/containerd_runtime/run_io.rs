use super::RuntimeResult;
use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const TASK_DIR_MODE: u32 = 0o700;
const STDIO_FILE_MODE: u32 = 0o600;

pub(super) struct StdioPaths {
    pub(super) stdin: PathBuf,
    pub(super) stdout: PathBuf,
    pub(super) stderr: PathBuf,
}

pub(super) fn create_task_dir(path: &Path) -> RuntimeResult<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(TASK_DIR_MODE)
        .create(path)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "failed to create task directory {}: {error}",
                    path.display()
                ),
            )
        })?;
    fs::set_permissions(path, Permissions::from_mode(TASK_DIR_MODE)).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to set task directory permissions on {}: {error}",
                path.display()
            ),
        )
    })?;
    Ok(())
}

pub(super) fn create_stdio_files(run_dir: &Path) -> RuntimeResult<StdioPaths> {
    let stdio = StdioPaths {
        stdin: run_dir.join("stdin"),
        stdout: run_dir.join("stdout"),
        stderr: run_dir.join("stderr"),
    };

    create_file(&stdio.stdin)?;
    create_file(&stdio.stdout)?;
    create_file(&stdio.stderr)?;

    Ok(stdio)
}

pub(super) fn read_bounded(path: &Path, limit: usize) -> RuntimeResult<(Vec<u8>, bool)> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), false)),
        Err(error) => {
            return Err(io::Error::new(
                error.kind(),
                format!("failed to open {}: {error}", path.display()),
            )
            .into());
        }
    };

    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > limit;
    if truncated {
        bytes.truncate(limit);
    }

    Ok((bytes, truncated))
}

pub(super) fn path_string(path: &Path) -> String {
    path.display().to_string()
}

fn create_file(path: &Path) -> RuntimeResult<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(STDIO_FILE_MODE)
        .open(path)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("failed to create {}: {error}", path.display()),
            )
        })?;
    fs::set_permissions(path, Permissions::from_mode(STDIO_FILE_MODE)).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to set permissions on {}: {error}", path.display()),
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use uuid::Uuid;

    #[test]
    fn create_task_dir_uses_private_permissions() -> RuntimeResult<()> {
        let run_dir = temp_run_dir();

        create_task_dir(&run_dir)?;

        assert_eq!(mode(&run_dir)?, TASK_DIR_MODE);
        fs::remove_dir_all(&run_dir)?;
        Ok(())
    }

    #[test]
    fn create_stdio_files_uses_private_permissions() -> RuntimeResult<()> {
        let run_dir = temp_run_dir();
        create_task_dir(&run_dir)?;

        let stdio = create_stdio_files(&run_dir)?;

        assert_eq!(mode(&stdio.stdin)?, STDIO_FILE_MODE);
        assert_eq!(mode(&stdio.stdout)?, STDIO_FILE_MODE);
        assert_eq!(mode(&stdio.stderr)?, STDIO_FILE_MODE);
        fs::remove_dir_all(&run_dir)?;
        Ok(())
    }

    fn temp_run_dir() -> PathBuf {
        std::env::temp_dir().join(format!("billow-test-{}", Uuid::new_v4().simple()))
    }

    fn mode(path: &Path) -> RuntimeResult<u32> {
        Ok(fs::metadata(path)?.permissions().mode() & 0o777)
    }
}
