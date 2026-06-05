use crate::client;
use crate::pool::Pool;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub(crate) fn start(
    socket_path: PathBuf,
    log_path: PathBuf,
    pid_path: Option<PathBuf>,
) -> io::Result<()> {
    if client::send_command_to(&socket_path, "ping").is_ok() {
        return Ok(());
    }

    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }

    if let Some(parent) = pid_path.as_deref().and_then(|path| path.parent()) {
        fs::create_dir_all(parent)?;
    }

    daemonize_and_serve(socket_path, log_path, pid_path)
}

fn daemonize_and_serve(
    socket_path: PathBuf,
    log_path: PathBuf,
    pid_path: Option<PathBuf>,
) -> io::Result<()> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let null = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")?;

    match unsafe { libc::fork() } {
        -1 => return Err(io::Error::last_os_error()),
        0 => {}
        _parent_pid => return Ok(()),
    }

    if unsafe { libc::setsid() } == -1 {
        unsafe { libc::_exit(1) };
    }

    match unsafe { libc::fork() } {
        -1 => unsafe { libc::_exit(1) },
        0 => {}
        _session_leader_pid => unsafe { libc::_exit(0) },
    }

    redirect_fd(null.as_raw_fd(), libc::STDIN_FILENO)?;
    redirect_fd(log.as_raw_fd(), libc::STDOUT_FILENO)?;
    redirect_fd(log.as_raw_fd(), libc::STDERR_FILENO)?;

    if let Some(pid_path) = pid_path {
        fs::write(pid_path, format!("{}\n", std::process::id()))?;
    }

    serve(socket_path)
}

fn redirect_fd(from: RawFd, to: RawFd) -> io::Result<()> {
    if unsafe { libc::dup2(from, to) } == -1 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

pub(crate) fn serve(socket_path: PathBuf) -> io::Result<()> {
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
    }

    remove_stale_socket(&socket_path)?;

    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;

    let pool = Arc::new(Pool::new());
    let launcher_pool = Arc::clone(&pool);
    let launcher = thread::spawn(move || launcher_pool.launcher_loop());
    let mut handlers = Vec::new();

    eprintln!("vm-pool listening on {}", socket_path.display());

    while !pool.is_shutdown() {
        match listener.accept() {
            Ok((stream, _)) => {
                let pool = Arc::clone(&pool);
                handlers.push(thread::spawn(move || handle_client(stream, pool)));
                reap_finished_handlers(&mut handlers);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
                reap_finished_handlers(&mut handlers);
            }
            Err(error) => return Err(error),
        }
    }

    for handler in handlers {
        if handler.join().is_err() {
            eprintln!("vm-pool client handler thread panicked");
        }
    }

    pool.request_shutdown();
    if launcher.join().is_err() {
        eprintln!("vm-pool launcher thread panicked");
    }

    let _ = fs::remove_file(&socket_path);
    Ok(())
}

fn reap_finished_handlers(handlers: &mut Vec<thread::JoinHandle<()>>) {
    let mut running = Vec::with_capacity(handlers.len());
    for handler in handlers.drain(..) {
        if handler.is_finished() {
            if handler.join().is_err() {
                eprintln!("vm-pool client handler thread panicked");
            }
        } else {
            running.push(handler);
        }
    }
    *handlers = running;
}

fn remove_stale_socket(socket_path: &Path) -> io::Result<()> {
    if !socket_path.exists() {
        return Ok(());
    }

    if UnixStream::connect(socket_path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("vm-pool is already listening on {}", socket_path.display()),
        ));
    }

    fs::remove_file(socket_path)
}

fn handle_client(mut stream: UnixStream, pool: Arc<Pool>) {
    let response = match read_command(&stream).and_then(|line| dispatch_command(&line, pool)) {
        Ok(response) if response.is_empty() => "OK\n".to_string(),
        Ok(response) => format!("OK {response}\n"),
        Err(error) => format!("ERR {error}\n"),
    };

    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn read_command(stream: &UnixStream) -> io::Result<String> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn dispatch_command(line: &str, pool: Arc<Pool>) -> io::Result<String> {
    let mut parts = line.split_whitespace();
    let command = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;

    match command {
        "take" => pool.take(),
        "drop" => {
            let vm_name = parts
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "usage: drop <vm>"))?;
            pool.drop_vm(vm_name)?;
            Ok(String::new())
        }
        "stop" => Ok(pool.stop_all()),
        "status" => Ok(pool.status()),
        "wait-ready" => {
            let timeout = parts
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(600);
            pool.wait_ready(Duration::from_secs(timeout))?;
            Ok(String::new())
        }
        "ping" => Ok(String::new()),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown command '{other}'"),
        )),
    }
}
