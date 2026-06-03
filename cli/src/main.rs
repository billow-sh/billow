use billow_api::api::echo_service_client::EchoServiceClient;
use billow_api::api::task_service_client::TaskServiceClient;
use billow_api::api::{EchoRequest, RunRequest};

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
            client.run(RunRequest { image }).await?;
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
