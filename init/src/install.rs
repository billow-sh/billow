use crate::paths;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(crate) struct BinarySource {
    name: &'static str,
    path: PathBuf,
}

pub(crate) fn ensure_binaries_not_installed(binary_names: &[&str]) -> io::Result<()> {
    ensure_not_installed(binary_names, paths::binary_install_path)
}

pub(crate) fn ensure_cni_plugins_not_installed() -> io::Result<()> {
    ensure_not_installed(
        paths::CNI_PLUGIN_BINARY_NAMES,
        paths::cni_plugin_install_path,
    )
}

fn ensure_not_installed(
    binary_names: &[&str],
    install_path: fn(&str) -> PathBuf,
) -> io::Result<()> {
    for binary_name in binary_names {
        let install_path = install_path(binary_name);

        if install_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists", paths::display(&install_path)),
            ));
        }
    }

    Ok(())
}

pub(crate) fn find_binary_sources(
    binary_names: &'static [&'static str],
) -> io::Result<Vec<BinarySource>> {
    find_sources(
        binary_names,
        paths::binary_source_candidates,
        "next to billow-init or in the current directory",
    )
}

pub(crate) fn find_cni_plugin_sources() -> io::Result<Vec<BinarySource>> {
    find_sources(
        paths::CNI_PLUGIN_BINARY_NAMES,
        paths::cni_plugin_source_candidates,
        "in the cni directory next to billow-init",
    )
}

fn find_sources(
    binary_names: &'static [&'static str],
    candidates: fn(&str) -> io::Result<Vec<PathBuf>>,
    not_found_hint: &str,
) -> io::Result<Vec<BinarySource>> {
    let mut sources = Vec::with_capacity(binary_names.len());

    for binary_name in binary_names {
        let mut found = None;

        for candidate in candidates(binary_name)? {
            if candidate.is_file() {
                found = Some(candidate);
                break;
            }
        }

        let Some(path) = found else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{binary_name} must be present {not_found_hint}"),
            ));
        };

        sources.push(BinarySource {
            name: binary_name,
            path,
        });
    }

    Ok(sources)
}

pub(crate) fn install_binaries(binary_sources: &[BinarySource]) -> io::Result<()> {
    let bin_dir = paths::bin_dir();

    fs::create_dir_all(&bin_dir).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to create binary directory {}: {error}",
                paths::display(&bin_dir)
            ),
        )
    })?;

    for binary_source in binary_sources {
        install_one(binary_source, paths::binary_install_path)?;
    }

    Ok(())
}

pub(crate) fn install_cni_plugins(binary_sources: &[BinarySource]) -> io::Result<()> {
    let cni_plugin_dir = paths::cni_plugin_dir();

    fs::create_dir_all(&cni_plugin_dir).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to create CNI plugin directory {}: {error}",
                paths::display(&cni_plugin_dir)
            ),
        )
    })?;

    for binary_source in binary_sources {
        install_one(binary_source, paths::cni_plugin_install_path)?;
    }

    Ok(())
}

fn install_one(binary_source: &BinarySource, install_path: fn(&str) -> PathBuf) -> io::Result<()> {
    let install_path = install_path(binary_source.name);

    move_file(&binary_source.path, &install_path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to move {} to {}: {error}",
                binary_source.path.display(),
                paths::display(&install_path)
            ),
        )
    })?;

    fs::set_permissions(&install_path, fs::Permissions::from_mode(0o755)).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to set permissions on {}: {error}",
                paths::display(&install_path)
            ),
        )
    })?;

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
