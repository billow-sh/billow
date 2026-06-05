use std::env;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

const DEFAULT_SOCKET_NAME: &str = "billow-vm-pool.sock";

pub(crate) fn socket_path() -> PathBuf {
    match env::var_os("BILLOW_VM_POOL_SOCKET") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => env::temp_dir().join(DEFAULT_SOCKET_NAME),
    }
}

pub(crate) fn send_command(command: &str) -> io::Result<String> {
    send_command_to(&socket_path(), command)
}

pub(crate) fn send_command_to(socket_path: &Path, command: &str) -> io::Result<String> {
    let mut stream = UnixStream::connect(socket_path)?;
    stream.write_all(command.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let response = response.trim_end_matches('\n');

    if response == "OK" {
        return Ok(String::new());
    }

    if let Some(rest) = response.strip_prefix("OK ") {
        return Ok(rest.to_string());
    }

    if let Some(rest) = response.strip_prefix("ERR ") {
        return Err(io::Error::other(rest.to_string()));
    }

    Err(io::Error::other(format!(
        "invalid response from vm-pool daemon: {response:?}"
    )))
}
