use billow_api::echo::EchoRequest;
use billow_api::echo::echo_service_client::EchoServiceClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let message = std::env::args()
        .nth(1)
        .expect("expected a message as the first argument");

    let agent_ip = std::env::var("BILLOW_AGENT_IP").unwrap_or_else(|_| String::from("127.0.0.1"));
    let mut client = EchoServiceClient::connect(format!("http://{agent_ip}:50051")).await?;
    let response = client.echo(EchoRequest { message }).await?;

    println!("{}", response.into_inner().message);

    Ok(())
}
