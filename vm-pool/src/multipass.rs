use std::io;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(660);
const DESTROY_TIMEOUT: Duration = Duration::from_secs(120);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn launch_vm(vm_name: &str) -> io::Result<()> {
    let mut child = Command::new("multipass")
        .args([
            "launch",
            "--name",
            vm_name,
            "--cpus",
            "1",
            "--memory",
            "1G",
            "--disk",
            "5G",
            "--timeout",
            "600",
        ])
        .spawn()?;

    let status = wait_for_child(
        &mut child,
        LAUNCH_TIMEOUT,
        &format!("multipass launch {vm_name}"),
    )?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "multipass launch exited with {status}"
        )));
    }

    Ok(())
}

pub(crate) fn vm_exec_ready(vm_name: &str) -> bool {
    multipass_status(&["exec", vm_name, "--", "true"], PROBE_TIMEOUT)
        .map(|status| status.success())
        .unwrap_or(false)
}

pub(crate) fn destroy_vm(vm_name: &str) {
    eprintln!("vm-pool destroying: {vm_name}");
    if let Err(error) = multipass_status(&["stop", vm_name], DESTROY_TIMEOUT) {
        eprintln!("vm-pool failed to stop {vm_name}: {error}");
    }
    if let Err(error) = multipass_status(&["delete", "--purge", vm_name], DESTROY_TIMEOUT) {
        eprintln!("vm-pool failed to delete {vm_name}: {error}");
    }
}

pub(crate) fn purge() {
    let _ = multipass_status(&["purge"], DESTROY_TIMEOUT);
}

fn multipass_status(args: &[&str], timeout: Duration) -> io::Result<ExitStatus> {
    let mut child = Command::new("multipass")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    wait_for_child(
        &mut child,
        timeout,
        &format!("multipass {}", args.join(" ")),
    )
}

fn wait_for_child(
    child: &mut Child,
    timeout: Duration,
    description: &str,
) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{description} timed out after {}s", timeout.as_secs()),
            ));
        }

        thread::sleep(Duration::from_millis(250));
    }
}
