mod install;
mod system;
mod systemd;

use std::io;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => {
            println!(
                "{} installed and started as {}",
                install::AGENT_BINARY_NAME,
                systemd::SERVICE_NAME
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
    install::ensure_agent_not_installed()?;
    systemd::ensure_service_not_installed()?;

    let agent_source = install::find_agent_source()?;
    install::install_agent_binary(&agent_source)?;
    systemd::install_unit()?;
    systemd::reload()?;
    systemd::enable_and_start_service()?;

    Ok(())
}
