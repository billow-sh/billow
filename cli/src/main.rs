use billow_api::api::workload_service_client::WorkloadServiceClient;
use billow_api::api::{
    DeleteWorkloadRequest, EchoRequest, GetWorkloadLogsRequest, GetWorkloadRequest,
    StartWorkloadRequest, StopWorkloadRequest, SubmitWorkloadRequest, WorkloadKind,
    WorkloadResponse, echo_service_client::EchoServiceClient,
};
use std::io::{self, Write};

type Error = Box<dyn std::error::Error>;

enum Command {
    Echo { message: String },
    Workload(WorkloadCommand),
}

enum WorkloadCommand {
    Submit { kind: WorkloadKind, image: String },
    Get { workload_id: String },
    Logs { workload_id: String },
    Start { workload_id: String },
    Stop { workload_id: String },
    Delete { workload_id: String },
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
        Command::Workload(command) => run_workload_command(endpoint, command).await?,
    }

    Ok(())
}

async fn run_workload_command(endpoint: String, command: WorkloadCommand) -> Result<(), Error> {
    let mut client = WorkloadServiceClient::connect(endpoint).await?;

    match command {
        WorkloadCommand::Submit { kind, image } => {
            let response = client
                .submit(SubmitWorkloadRequest {
                    kind: kind as i32,
                    image,
                })
                .await?
                .into_inner();
            println!("{}", response.workload_id);
        }
        WorkloadCommand::Get { workload_id } => {
            let response = client
                .get(GetWorkloadRequest { workload_id })
                .await?
                .into_inner();
            print_workload(&mut io::stdout(), &response)?;
        }
        WorkloadCommand::Logs { workload_id } => {
            let response = client
                .get_logs(GetWorkloadLogsRequest { workload_id })
                .await?
                .into_inner();
            io::stdout().write_all(&response.stdout)?;
            io::stderr().write_all(&response.stderr)?;
            if response.stdout_truncated {
                writeln!(
                    io::stderr(),
                    "\nbillow-cli: stdout from workload was truncated"
                )?;
            }
            if response.stderr_truncated {
                writeln!(
                    io::stderr(),
                    "\nbillow-cli: stderr from workload was truncated"
                )?;
            }
        }
        WorkloadCommand::Start { workload_id } => {
            let response = client
                .start(StartWorkloadRequest { workload_id })
                .await?
                .into_inner();
            print_workload(&mut io::stdout(), &response)?;
        }
        WorkloadCommand::Stop { workload_id } => {
            let response = client
                .stop(StopWorkloadRequest { workload_id })
                .await?
                .into_inner();
            print_workload(&mut io::stdout(), &response)?;
        }
        WorkloadCommand::Delete { workload_id } => {
            let response = client
                .delete(DeleteWorkloadRequest { workload_id })
                .await?
                .into_inner();
            print_workload(&mut io::stdout(), &response)?;
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
        Some("workload") => parse_workload_command(args).map(Command::Workload),
        Some(command) => Err(usage_error(format!("unknown subcommand '{command}'"))),
        None => Err(usage_error("expected a subcommand: echo or workload")),
    }
}

fn parse_workload_command(
    mut args: impl Iterator<Item = String>,
) -> Result<WorkloadCommand, Error> {
    match args.next().as_deref() {
        Some("submit") => {
            let kind = parse_workload_kind(args.next().as_deref())?;
            let image = args.next().ok_or_else(|| {
                usage_error("expected an image, e.g. billow-cli workload submit once hello-world")
            })?;
            reject_extra_args(args)?;
            Ok(WorkloadCommand::Submit { kind, image })
        }
        Some("get") => {
            let workload_id = next_workload_id(&mut args, "get")?;
            reject_extra_args(args)?;
            Ok(WorkloadCommand::Get { workload_id })
        }
        Some("logs") => {
            let workload_id = next_workload_id(&mut args, "logs")?;
            reject_extra_args(args)?;
            Ok(WorkloadCommand::Logs { workload_id })
        }
        Some("start") => {
            let workload_id = next_workload_id(&mut args, "start")?;
            reject_extra_args(args)?;
            Ok(WorkloadCommand::Start { workload_id })
        }
        Some("stop") => {
            let workload_id = next_workload_id(&mut args, "stop")?;
            reject_extra_args(args)?;
            Ok(WorkloadCommand::Stop { workload_id })
        }
        Some("delete") => {
            let workload_id = next_workload_id(&mut args, "delete")?;
            reject_extra_args(args)?;
            Ok(WorkloadCommand::Delete { workload_id })
        }
        Some(command) => Err(usage_error(format!(
            "unknown workload subcommand '{command}'"
        ))),
        None => Err(usage_error(
            "expected a workload subcommand: submit, get, logs, start, stop, or delete",
        )),
    }
}

fn parse_workload_kind(kind: Option<&str>) -> Result<WorkloadKind, Error> {
    match kind {
        Some("once") => Ok(WorkloadKind::Once),
        Some("service") => Ok(WorkloadKind::Service),
        Some(kind) => Err(usage_error(format!(
            "unknown workload kind '{kind}', expected once or service"
        ))),
        None => Err(usage_error("expected a workload kind: once or service")),
    }
}

fn next_workload_id(
    args: &mut impl Iterator<Item = String>,
    command: &str,
) -> Result<String, Error> {
    args.next().ok_or_else(|| {
        usage_error(format!(
            "expected a workload id, e.g. billow-cli workload {command} <id>"
        ))
    })
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

fn print_workload(writer: &mut impl Write, response: &WorkloadResponse) -> io::Result<()> {
    writeln!(writer, "workload_id={}", response.workload_id)?;
    writeln!(writer, "kind={}", workload_kind_name(response.kind))?;
    writeln!(writer, "image={}", response.image)?;
    writeln!(
        writer,
        "desired_state={}",
        desired_state_name(response.desired_state)
    )?;
    writeln!(
        writer,
        "actual_state={}",
        actual_state_name(response.actual_state)
    )?;
    writeln!(writer, "runtime_task_id={}", response.runtime_task_id)?;
    writeln!(writer, "container_ip={}", response.container_ip)?;
    if let Some(exit_code) = response.exit_code {
        writeln!(writer, "exit_code={exit_code}")?;
    } else {
        writeln!(writer, "exit_code=")?;
    }
    writeln!(writer, "error={}", response.error)?;
    writeln!(
        writer,
        "created_at_unix_secs={}",
        response.created_at_unix_secs
    )?;
    writeln!(
        writer,
        "updated_at_unix_secs={}",
        response.updated_at_unix_secs
    )?;
    writer.flush()
}

fn workload_kind_name(kind: i32) -> String {
    match WorkloadKind::try_from(kind) {
        Ok(WorkloadKind::Once) => String::from("once"),
        Ok(WorkloadKind::Service) => String::from("service"),
        _ => String::from("unspecified"),
    }
}

fn desired_state_name(state: i32) -> String {
    match billow_api::api::DesiredState::try_from(state) {
        Ok(billow_api::api::DesiredState::Running) => String::from("running"),
        Ok(billow_api::api::DesiredState::Stopped) => String::from("stopped"),
        Ok(billow_api::api::DesiredState::Deleted) => String::from("deleted"),
        _ => String::from("unspecified"),
    }
}

fn actual_state_name(state: i32) -> String {
    match billow_api::api::ActualState::try_from(state) {
        Ok(billow_api::api::ActualState::Accepted) => String::from("accepted"),
        Ok(billow_api::api::ActualState::Creating) => String::from("creating"),
        Ok(billow_api::api::ActualState::Starting) => String::from("starting"),
        Ok(billow_api::api::ActualState::Running) => String::from("running"),
        Ok(billow_api::api::ActualState::Stopping) => String::from("stopping"),
        Ok(billow_api::api::ActualState::Stopped) => String::from("stopped"),
        Ok(billow_api::api::ActualState::Failed) => String::from("failed"),
        Ok(billow_api::api::ActualState::Deleted) => String::from("deleted"),
        _ => String::from("unspecified"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(actual_state: billow_api::api::ActualState) -> WorkloadResponse {
        WorkloadResponse {
            workload_id: String::from("workload-1"),
            kind: WorkloadKind::Service as i32,
            image: String::from("nginx"),
            desired_state: billow_api::api::DesiredState::Running as i32,
            actual_state: actual_state as i32,
            runtime_task_id: String::from("task-1"),
            container_ip: String::from("10.1.1.2"),
            exit_code: Some(0),
            error: String::new(),
            created_at_unix_secs: 10,
            updated_at_unix_secs: 20,
        }
    }

    #[test]
    fn prints_stable_workload_fields() {
        let mut output = Vec::new();
        print_workload(
            &mut output,
            &response(billow_api::api::ActualState::Running),
        )
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("workload_id=workload-1\n"));
        assert!(output.contains("kind=service\n"));
        assert!(output.contains("desired_state=running\n"));
        assert!(output.contains("actual_state=running\n"));
        assert!(output.contains("container_ip=10.1.1.2\n"));
    }

    #[test]
    fn parses_workload_submit() {
        let command = parse_workload_command(
            ["submit", "once", "hello-world"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();

        match command {
            WorkloadCommand::Submit { kind, image } => {
                assert_eq!(kind, WorkloadKind::Once);
                assert_eq!(image, "hello-world");
            }
            _ => panic!("expected submit command"),
        }
    }
}
