mod containerd;
mod install;
mod paths;
mod system;
mod systemd;

use std::io;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => {
            println!(
                "{} installed and started as {} with {}",
                paths::AGENT_BINARY_NAME,
                paths::AGENT_SERVICE_NAME,
                paths::CONTAINERD_SERVICE_NAME
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("billow-init: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    system::ensure_root()?;
    systemd::ensure_available()?;
    install::ensure_binaries_not_installed(paths::INSTALL_BINARY_NAMES)?;
    containerd::ensure_config_not_installed()?;
    systemd::ensure_units_not_installed()?;

    let binary_sources = install::find_binary_sources(paths::INSTALL_BINARY_NAMES)?;
    install::install_binaries(&binary_sources)?;
    containerd::install_config()?;
    systemd::install_units()?;
    systemd::reload()?;
    systemd::enable_and_start_services()?;

    Ok(())
}
