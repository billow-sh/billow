mod client;
mod daemon;
mod multipass;
mod pool;

use std::env;
use std::io;
use std::path::PathBuf;

fn main() {
    if let Err(error) = run() {
        eprintln!("vm-pool: {error}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".to_string());

    match command.as_str() {
        "start" => {
            let log_path = args
                .next()
                .map(PathBuf::from)
                .unwrap_or_else(|| env::temp_dir().join("billow-vm-pool.log"));
            let pid_path = args.next().map(PathBuf::from);
            daemon::start(client::socket_path(), log_path, pid_path)
        }
        "serve" => daemon::serve(client::socket_path()),
        "take" => {
            let vm_name = client::send_command("take")?;
            println!("{vm_name}");
            Ok(())
        }
        "drop" => {
            let vm_name = args.next().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "usage: vm-pool drop <vm>")
            })?;
            client::send_command(&format!("drop {vm_name}"))?;
            Ok(())
        }
        "stop" => {
            let response = client::send_command("stop")?;
            if !response.is_empty() {
                println!("{response}");
            }
            Ok(())
        }
        "status" => {
            println!("{}", client::send_command("status")?);
            Ok(())
        }
        "wait-ready" => {
            let timeout = args
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(600);
            client::send_command(&format!("wait-ready {timeout}"))?;
            Ok(())
        }
        "ping" => {
            client::send_command("ping")?;
            Ok(())
        }
        "help" | "-h" | "--help" => {
            print_usage();
            Ok(())
        }
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown command '{other}'"),
        )),
    }
}

fn print_usage() {
    println!(
        "usage: vm-pool <start <log> [pid-file]|serve|take|drop <vm>|stop|status|wait-ready [seconds]|ping>\n\
         set BILLOW_VM_POOL_SOCKET to override the Unix socket path"
    );
}
