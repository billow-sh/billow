use billow_api::api::echo_service_client::EchoServiceClient;
use billow_api::api::task_service_client::TaskServiceClient;
use billow_api::api::{EchoRequest, RunRequest, RunResponse};
use std::io::{self, Write};

type Error = Box<dyn std::error::Error>;

enum Command {
    Echo { message: String },
    Run { image: String },
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let command = parse_command()?;

    let agent_ip = std::env::var("BILLOW_AGENT_IP").unwrap_or_else(|_| String::from("127.0.0.1"));
    let endpoint = format!("http://{agent_ip}:50052");

    match command {
        Command::Echo { message } => {
            let mut client = EchoServiceClient::connect(endpoint).await?;
            let response = client.echo(EchoRequest { message }).await?;
            println!("{}", response.into_inner().message);
        }
        Command::Run { image } => {
            let mut client = TaskServiceClient::connect(endpoint).await?;
            let response = client.run(RunRequest { image }).await?.into_inner();
            let exit_code = write_run_response(&mut io::stdout(), &mut io::stderr(), &response)?;
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        }
    }

    Ok(())
}

fn parse_command() -> Result<Command, Error> {
    let mut args = std::env::args().skip(1);

    match args.next().as_deref() {
        Some("echo") => {
            let message = args.next().ok_or_else(|| {
                usage_error("expected a message, e.g. billow-cli echo \"my text\"")
            })?;
            reject_extra_args(args)?;
            Ok(Command::Echo { message })
        }
        Some("run") => {
            let image = args.next().ok_or_else(|| {
                usage_error("expected an image, e.g. billow-cli run \"my.image:latest\"")
            })?;
            reject_extra_args(args)?;
            Ok(Command::Run { image })
        }
        Some(command) => Err(usage_error(format!("unknown subcommand '{command}'"))),
        None => Err(usage_error("expected a subcommand: echo or run")),
    }
}

fn reject_extra_args(mut args: impl Iterator<Item = String>) -> Result<(), Error> {
    if let Some(arg) = args.next() {
        return Err(usage_error(format!("unexpected argument '{arg}'")));
    }

    Ok(())
}

fn usage_error(message: impl Into<String>) -> Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into()).into()
}

fn write_run_response(
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    response: &RunResponse,
) -> io::Result<i32> {
    stdout.write_all(&response.stdout)?;
    stderr.write_all(&response.stderr)?;

    if response.stdout_truncated {
        writeln!(
            stderr,
            "\nbillow-cli: stdout from task {} was truncated",
            response.task_id
        )?;
    }
    if response.stderr_truncated {
        writeln!(
            stderr,
            "\nbillow-cli: stderr from task {} was truncated",
            response.task_id
        )?;
    }

    stdout.flush()?;
    stderr.flush()?;

    Ok(exit_code_for_process(response.exit_code))
}

fn exit_code_for_process(exit_code: u32) -> i32 {
    i32::try_from(exit_code).unwrap_or(i32::MAX).min(255)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(exit_code: u32, stdout: &[u8], stderr: &[u8]) -> RunResponse {
        RunResponse {
            task_id: String::from("task-1"),
            exit_code,
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }

    #[test]
    fn writes_container_output_to_matching_streams() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let response = response(0, b"hello\n", b"warning\n");

        let exit_code = write_run_response(&mut stdout, &mut stderr, &response).unwrap();

        assert_eq!(exit_code, 0);
        assert_eq!(stdout, b"hello\n");
        assert_eq!(stderr, b"warning\n");
    }

    #[test]
    fn returns_container_exit_code() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let response = response(42, b"", b"failed\n");

        let exit_code = write_run_response(&mut stdout, &mut stderr, &response).unwrap();

        assert_eq!(exit_code, 42);
        assert_eq!(stdout, b"");
        assert_eq!(stderr, b"failed\n");
    }

    #[test]
    fn caps_exit_code_to_process_range() {
        assert_eq!(exit_code_for_process(300), 255);
        assert_eq!(exit_code_for_process(u32::MAX), 255);
    }
}
