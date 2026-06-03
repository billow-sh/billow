use std::env;
use std::io;
use std::process::{Command, Stdio};

pub(crate) const OVERRIDE_UID_ENV: &str = "BILLOW_OVERRIDE_UID";

unsafe extern "C" {
    fn geteuid() -> u32;
}

pub(crate) fn ensure_root() -> io::Result<()> {
    if effective_uid()? == 0 {
        return Ok(());
    }

    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "must be run as root",
    ))
}

fn effective_uid() -> io::Result<u32> {
    if let Some(uid) = env::var_os(OVERRIDE_UID_ENV) {
        return uid.to_string_lossy().parse::<u32>().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{OVERRIDE_UID_ENV} must be an unsigned integer: {error}"),
            )
        });
    }

    // SAFETY: geteuid has no preconditions and does not modify memory.
    Ok(unsafe { geteuid() })
}

pub(crate) fn command_succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(crate) fn run_command(program: &str, args: &[&str]) -> io::Result<()> {
    let status = Command::new(program).args(args).status().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to run {program} {}: {error}", args.join(" ")),
        )
    })?;

    if status.success() {
        return Ok(());
    }

    Err(io::Error::other(format!(
        "{program} {} failed with {status}",
        args.join(" ")
    )))
}
